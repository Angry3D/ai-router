use std::io::Cursor;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use router_core::proxy::SystemProxySettings;
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

const VERSION: &str = "99.0.0";
const PACKAGE: &[u8] = b"synthetic signed updater download; never installed";

fn signing_fixture() -> (String, String) {
    // New test-only key material stays in memory, never in env, files, or logs.
    let keypair = minisign::KeyPair::generate_unencrypted_keypair().expect("ephemeral keypair");
    let signature = minisign::sign(
        Some(&keypair.pk),
        &keypair.sk,
        Cursor::new(PACKAGE),
        Some("timestamp:1700000000\tfile:AI.Router.app.tar.gz\tversion:99.0.0"),
        Some("synthetic local updater fixture"),
    )
    .expect("synthetic signature");
    (
        STANDARD.encode(keypair.pk.to_box().expect("public key box").to_string()),
        STANDARD.encode(signature.to_string()),
    )
}

fn metadata(signature: &str, download_url: &Url) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "version": VERSION,
        "platforms": {
            "darwin-aarch64": {
                "url": download_url,
                "signature": signature
            }
        }
    }))
    .expect("metadata")
}

fn ok_response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len(),
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn redirect_response(target: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .into_bytes()
}

async fn server(responses: Vec<Vec<u8>>) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let endpoint = Url::parse(&format!(
        "http://{}",
        listener.local_addr().expect("address")
    ))
    .expect("endpoint");
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().await.expect("request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await.expect("request bytes");
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() < 8192);
                }
                stream.write_all(&response).await.expect("response");
                requests.push(String::from_utf8(request).expect("request text"));
            }
            requests
        })
        .await
        .expect("bounded loopback fixture")
    });
    (endpoint, task)
}

fn updater(
    slot: Arc<ArcSwap<OutboundProxyPolicy>>,
    endpoint: Url,
    public_key: &str,
) -> (tauri::App<MockRuntime>, tauri_plugin_updater::Updater) {
    let mut context = mock_context(noop_assets());
    context.config_mut().identifier = "com.relax.airouter.test".to_owned();
    context.config_mut().plugins.0.insert(
        "updater".to_owned(),
        serde_json::json!({
            "pubkey": public_key,
            "requireSignedVersion": true,
            "dangerousInsecureTransportProtocol": true,
        }),
    );
    let app = mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .expect("mock desktop app");
    let updater = app
        .updater_builder()
        .target("darwin-aarch64")
        .endpoints(vec![endpoint])
        .expect("synthetic endpoint")
        .configure_client(move |builder| configure_updater_client(&slot, builder))
        .build()
        .expect("updater");
    (app, updater)
}

#[tokio::test]
async fn plugin_checks_then_downloads_cached_signed_update_using_changed_policy() {
    let (public_key, signature) = signing_fixture();
    let archive = Url::parse("http://archive.invalid/AI.Router.app.tar.gz").expect("archive URL");
    let endpoint = Url::parse("http://updates.invalid/latest.json").expect("metadata URL");
    let (metadata_proxy, metadata_requests) =
        server(vec![ok_response(&metadata(&signature, &archive))]).await;
    let transport = OutboundProxyTransport::default();
    transport.set_system_settings(Ok(SystemProxySettings {
        http: Some(metadata_proxy),
        automatic_enabled: true,
        ..SystemProxySettings::default()
    }));
    let slot = Arc::new(ArcSwap::from_pointee(transport.policy()));
    prepare_update_policy(&transport, &slot, std::slice::from_ref(&endpoint))
        .expect("check preflight");
    let (_app, updater) = updater(Arc::clone(&slot), endpoint, &public_key);
    let update = updater
        .check()
        .await
        .expect("real plugin check")
        .expect("available update");
    assert_eq!(update.version, VERSION);
    assert_eq!(update.download_url, archive);
    let metadata_requests = metadata_requests.await.expect("metadata capture");
    assert_eq!(metadata_requests.len(), 1);
    assert!(metadata_requests[0].starts_with("GET http://updates.invalid/latest.json HTTP/1.1"));

    // The first server is now gone. Rechecking or retaining its policy cannot succeed.
    let (archive_proxy, archive_requests) = server(vec![
        redirect_response("http://cdn.invalid/package"),
        ok_response(PACKAGE),
    ])
    .await;
    transport.set_system_settings(Ok(SystemProxySettings {
        http: Some(archive_proxy),
        automatic_enabled: true,
        ..SystemProxySettings::default()
    }));
    prepare_update_policy(
        &transport,
        &slot,
        std::slice::from_ref(&update.download_url),
    )
    .expect("cached download preflight");
    let downloaded = update
        .download(|_, _| {}, || {})
        .await
        .expect("verified plugin download");
    assert_eq!(downloaded, PACKAGE);
    let archive_requests = archive_requests.await.expect("archive captures");
    assert_eq!(archive_requests.len(), 2);
    assert!(
        archive_requests[0].starts_with("GET http://archive.invalid/AI.Router.app.tar.gz HTTP/1.1")
    );
    assert!(archive_requests[1].starts_with("GET http://cdn.invalid/package HTTP/1.1"));

    // A newly unsupported policy must reject this cached Update before any send.
    transport.set_system_settings(Ok(SystemProxySettings {
        automatic_enabled: true,
        ..SystemProxySettings::default()
    }));
    assert_eq!(
        prepare_update_policy(
            &transport,
            &slot,
            std::slice::from_ref(&update.download_url)
        )
        .expect_err("automatic-only cached download")
        .code,
        "system_proxy_automatic_unsupported",
    );

    // A later valid policy still cannot authorize bytes that fail the retained signature.
    let (invalid_proxy, invalid_requests) = server(vec![ok_response(b"tampered package")]).await;
    transport.set_system_settings(Ok(SystemProxySettings {
        http: Some(invalid_proxy),
        ..SystemProxySettings::default()
    }));
    prepare_update_policy(
        &transport,
        &slot,
        std::slice::from_ref(&update.download_url),
    )
    .expect("invalid package preflight");
    let failure = update
        .download(|_, _| {}, || {})
        .await
        .expect_err("plugin must reject bad signature");
    assert_eq!(map_install_error(&failure).code, "update_signature_invalid");
    assert_eq!(
        invalid_requests
            .await
            .expect("invalid package capture")
            .len(),
        1
    );
    // No test calls install/download_and_install or any app lifecycle action.
}

#[tokio::test]
async fn plugin_guards_loopback_metadata_and_cached_package_redirects() {
    let (public_key, signature) = signing_fixture();
    let transport = OutboundProxyTransport::default();
    transport.set_system_settings(Ok(SystemProxySettings {
        automatic_enabled: true,
        ..SystemProxySettings::default()
    }));
    let slot = Arc::new(ArcSwap::from_pointee(transport.policy()));
    let (endpoint, metadata_requests) = server(vec![redirect_response(
        "http://blocked.invalid/latest.json",
    )])
    .await;
    prepare_update_policy(&transport, &slot, std::slice::from_ref(&endpoint))
        .expect("loopback admitted");
    let (_app, checker) = updater(Arc::clone(&slot), endpoint, &public_key);
    let Err(failure) = checker.check().await else {
        panic!("metadata redirect must fail before external send");
    };
    assert_eq!(
        map_check_error(&failure).code,
        "system_proxy_automatic_unsupported"
    );
    assert_eq!(
        metadata_requests
            .await
            .expect("metadata redirect capture")
            .len(),
        1
    );

    let (archive, archive_requests) =
        server(vec![redirect_response("http://blocked.invalid/package")]).await;
    let (endpoint, metadata_requests) =
        server(vec![ok_response(&metadata(&signature, &archive))]).await;
    prepare_update_policy(&transport, &slot, std::slice::from_ref(&endpoint))
        .expect("loopback metadata");
    let (_app, checker) = updater(Arc::clone(&slot), endpoint, &public_key);
    let update = checker
        .check()
        .await
        .expect("loopback check")
        .expect("update");
    prepare_update_policy(
        &transport,
        &slot,
        std::slice::from_ref(&update.download_url),
    )
    .expect("loopback archive");
    let failure = update
        .download(|_, _| {}, || {})
        .await
        .expect_err("archive redirect must fail");
    assert_eq!(
        map_install_error(&failure).code,
        "system_proxy_automatic_unsupported"
    );
    assert_eq!(metadata_requests.await.expect("metadata capture").len(), 1);
    assert_eq!(
        archive_requests
            .await
            .expect("archive redirect capture")
            .len(),
        1
    );
}
