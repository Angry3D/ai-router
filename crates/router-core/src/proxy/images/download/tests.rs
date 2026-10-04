use std::{io::Write as _, process::Command, sync::Arc, time::Duration};

use super::{
    ImageAssetDownloader, ImageAssetErrorKind, ImageDownloadError,
    test_support::{AssetFixture, AssetReply},
};
use flate2::{Compression, write::GzEncoder};
use reqwest::{StatusCode, header};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
    sync::Notify,
    time::timeout,
};

fn assert_error(error: ImageDownloadError, kind: ImageAssetErrorKind, status: Option<StatusCode>) {
    assert_eq!(error.kind, kind);
    assert_eq!(error.upstream_status, status);
}
trait ResultExpectErr<T> {
    fn expect_err_no_debug(self, message: &str) -> ImageDownloadError;
}

impl<T> ResultExpectErr<T> for Result<T, ImageDownloadError> {
    fn expect_err_no_debug(self, message: &str) -> ImageDownloadError {
        match self {
            Ok(_) => panic!("{message}"),
            Err(error) => error,
        }
    }
}

#[tokio::test]
async fn arbitrary_https_target_and_custom_port_are_direct_gets() {
    let fixture = AssetFixture::new(vec![AssetReply::ok(b"asset".to_vec())]).await;
    let url = format!(
        "https://internal.example:{}/image.png?signature=synthetic#ignored",
        fixture.port()
    );
    let image = fixture
        .downloader()
        .download(url, StatusCode::ACCEPTED)
        .await
        .expect("direct GET");
    assert_eq!(image.decode().expect("asset bytes"), b"asset");
    assert_eq!(fixture.request_count(), 1);
    let request = &fixture.requests()[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/image.png?signature=synthetic");
    assert_eq!(
        request.headers[header::HOST],
        format!("internal.example:{}", fixture.port())
    );
    assert_eq!(request.headers[header::ACCEPT_ENCODING], "identity");
}

#[tokio::test]
async fn non_https_private_literal_and_custom_port_are_direct_gets() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("plain HTTP fixture");
    let address = listener.local_addr().expect("plain HTTP address");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("plain HTTP connection");
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).await.expect("plain HTTP request");
            assert!(read > 0, "plain HTTP request ended before headers");
            request.extend_from_slice(&chunk[..read]);
        }
        let request = String::from_utf8(request).expect("plain HTTP request text");
        assert!(request.starts_with("GET /image.png?signature=synthetic HTTP/1.1\r\n"));
        assert!(request.lines().any(|line| {
            line.eq_ignore_ascii_case(&format!("Host: 127.0.0.1:{}", address.port()))
        }));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nasset")
            .await
            .expect("plain HTTP response");
    });

    let image = ImageAssetDownloader::default()
        .download(
            format!(
                "http://127.0.0.1:{}/image.png?signature=synthetic",
                address.port()
            ),
            StatusCode::ACCEPTED,
        )
        .await
        .expect("direct HTTP GET");
    assert_eq!(image.decode().expect("asset bytes"), b"asset");
    server.await.expect("plain HTTP fixture task");
}

#[tokio::test]
async fn tls_hostname_validation_remains_active() {
    let fixture = AssetFixture::new(vec![AssetReply::ok(b"asset".to_vec())]).await;
    let error = fixture
        .downloader()
        .download(
            format!("https://wrong.example:{}/image.png", fixture.port()),
            StatusCode::OK,
        )
        .await
        .expect_err_no_debug("hostname mismatch");
    assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    assert_eq!(fixture.connection_count(), 1);
    assert_eq!(fixture.request_count(), 0);
}

#[tokio::test]
async fn url_parse_or_request_construction_failures_use_download_error() {
    let fixture = AssetFixture::new(Vec::new()).await;
    for raw in ["file:///image.png", "not a URL"] {
        let error = fixture
            .downloader()
            .download(raw.to_owned(), StatusCode::ACCEPTED)
            .await
            .expect_err_no_debug("unsupported target fails as a download error");
        assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    }
    assert_eq!(fixture.request_count(), 0);
}

#[tokio::test]
async fn redirects_are_explicit_and_do_not_reapply_target_admission() {
    let fixture = AssetFixture::new(vec![
        AssetReply::redirect("/next.png?signature=second")
            .header(header::SET_COOKIE, "credential=secret; Secure"),
        AssetReply::redirect("https://internal.example/final.png?signature=third"),
        AssetReply::ok(b"asset".to_vec()),
    ])
    .await;
    let image = fixture
        .downloader()
        .download(
            format!("{}?signature=first", AssetFixture::url()),
            StatusCode::OK,
        )
        .await
        .expect("safe direct redirects");
    assert_eq!(image.decode().expect("asset bytes"), b"asset");
    assert_eq!(fixture.request_count(), 3);
    let requests = fixture.requests();
    assert_eq!(requests[1].path, "/next.png?signature=second");
    assert_eq!(requests[2].headers[header::HOST], "internal.example");
    for request in requests {
        assert_eq!(request.method, "GET");
        assert_eq!(request.headers[header::ACCEPT_ENCODING], "identity");
        for forbidden in [
            "authorization",
            "proxy-authorization",
            "cookie",
            "referer",
            "x-api-key",
            "x-gateway-token",
        ] {
            assert!(!request.headers.contains_key(forbidden));
        }
    }
}

#[tokio::test]
async fn all_redirect_statuses_are_explicitly_followed() {
    for status in [301, 302, 303, 307, 308] {
        let fixture = AssetFixture::new(vec![
            AssetReply::status(
                StatusCode::from_u16(status).expect("redirect status"),
                Vec::new(),
            )
            .header(header::LOCATION, "/final.png"),
            AssetReply::ok(b"asset".to_vec()),
        ])
        .await;
        assert!(
            fixture
                .downloader()
                .download(AssetFixture::url(), StatusCode::OK)
                .await
                .is_ok()
        );
        assert_eq!(fixture.request_count(), 2);
    }
}

#[tokio::test]
async fn redirect_loops_missing_locations_and_hop_exhaustion_are_bounded() {
    for reply in [
        AssetReply::redirect("/image.png"),
        AssetReply::status(StatusCode::FOUND, Vec::new()),
    ] {
        let fixture = AssetFixture::new(vec![reply]).await;
        let error = fixture
            .downloader()
            .download(AssetFixture::url(), StatusCode::OK)
            .await
            .expect_err_no_debug("unusable redirect");
        assert_error(
            error,
            ImageAssetErrorKind::DownloadFailed,
            Some(StatusCode::FOUND),
        );
        assert_eq!(fixture.request_count(), 1);
    }
    let fixture = AssetFixture::new(
        (1..=5)
            .map(|index| AssetReply::redirect(&format!("/{index}.png")))
            .collect(),
    )
    .await;
    let error = fixture
        .downloader()
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .expect_err_no_debug("hop limit");
    assert_error(
        error,
        ImageAssetErrorKind::DownloadFailed,
        Some(StatusCode::FOUND),
    );
    assert_eq!(fixture.request_count(), 4);
    assert_eq!(fixture.requests()[3].path, "/3.png");
}

#[tokio::test]
async fn download_status_and_disconnect_failures_do_not_retry_or_read_error_bodies() {
    let fixture = AssetFixture::new(vec![AssetReply::status(
        StatusCode::SERVICE_UNAVAILABLE,
        b"secret-error-body".to_vec(),
    )])
    .await;
    let error = fixture
        .downloader()
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .expect_err_no_debug("HTTP failure");
    assert_error(
        error,
        ImageAssetErrorKind::DownloadFailed,
        Some(StatusCode::SERVICE_UNAVAILABLE),
    );
    assert_eq!(fixture.request_count(), 1);
    assert!(!format!("{error:?}").contains("secret-error-body"));

    let fixture = AssetFixture::new(vec![AssetReply::disconnect()]).await;
    let error = fixture
        .downloader()
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .expect_err_no_debug("disconnect failure");
    assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    assert_eq!(fixture.request_count(), 1);
    assert_eq!(fixture.connection_count(), 1);
}

#[tokio::test]
async fn connect_deadline_covers_a_stalled_tls_handshake() {
    let fixture = AssetFixture::new(Vec::new()).await;
    fixture.delay_handshake(Duration::from_secs(5));
    let mut downloader = fixture.downloader();
    downloader.limits.connect_timeout = Duration::from_millis(150);
    downloader.limits.total_timeout = Duration::from_secs(3);
    let error = downloader
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .expect_err_no_debug("TLS deadline");
    assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    assert_eq!(fixture.connection_count(), 1);
    assert_eq!(fixture.request_count(), 0);
}

#[tokio::test]
async fn total_deadline_covers_headers_body_and_redirects() {
    let fixture = AssetFixture::new(vec![
        AssetReply::ok(b"asset".to_vec()).delayed_headers(Duration::from_secs(5)),
    ])
    .await;
    let mut downloader = fixture.downloader();
    downloader.limits.total_timeout = Duration::from_millis(400);
    let error = downloader
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .expect_err_no_debug("header deadline");
    assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    assert_eq!(fixture.request_count(), 1);

    let fixture = AssetFixture::new(vec![
        AssetReply::status(StatusCode::CREATED, vec![1; 100]).chunked(1, Duration::from_millis(40)),
    ])
    .await;
    let mut downloader = fixture.downloader();
    downloader.limits.total_timeout = Duration::from_millis(400);
    let error = downloader
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .expect_err_no_debug("body deadline");
    assert_error(
        error,
        ImageAssetErrorKind::DownloadFailed,
        Some(StatusCode::CREATED),
    );
    assert_eq!(fixture.request_count(), 1);

    let fixture = AssetFixture::new(vec![
        AssetReply::redirect("/next.png").delayed_headers(Duration::from_millis(250)),
        AssetReply::ok(b"asset".to_vec()).delayed_headers(Duration::from_millis(250)),
    ])
    .await;
    let mut downloader = fixture.downloader();
    downloader.limits.total_timeout = Duration::from_millis(400);
    let error = downloader
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .expect_err_no_debug("shared redirect deadline");
    assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    assert_eq!(fixture.request_count(), 2);
}

#[tokio::test]
async fn wire_limit_checks_declared_and_actual_bytes() {
    for reply in [
        AssetReply::ok(vec![1; 9]),
        AssetReply::ok(vec![1; 9]).chunked(2, Duration::ZERO),
        AssetReply::ok(vec![1]).declared_length(9),
        AssetReply::ok(vec![1; 9])
            .chunked(2, Duration::ZERO)
            .header(header::CONTENT_LENGTH, "1"),
    ] {
        let fixture = AssetFixture::new(vec![reply]).await;
        let mut downloader = fixture.downloader();
        downloader.limits.body_bytes = 8;
        let error = downloader
            .download(AssetFixture::url(), StatusCode::ACCEPTED)
            .await
            .expect_err_no_debug("wire limit");
        assert_error(error, ImageAssetErrorKind::TooLarge, Some(StatusCode::OK));
        assert_eq!(fixture.request_count(), 1);
    }
}

#[tokio::test]
async fn incomplete_body_keeps_asset_status_and_decoding_stays_bounded() {
    let fixture = AssetFixture::new(vec![
        AssetReply::status(StatusCode::PARTIAL_CONTENT, b"short".to_vec()).declared_length(20),
    ])
    .await;
    let error = fixture
        .downloader()
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .expect_err_no_debug("truncated body");
    assert_error(
        error,
        ImageAssetErrorKind::DownloadFailed,
        Some(StatusCode::PARTIAL_CONTENT),
    );

    let original = vec![7; 128];
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&original).expect("synthetic gzip");
    let wire = encoder.finish().expect("synthetic gzip trailer");
    let fixture = AssetFixture::new(vec![
        AssetReply::status(StatusCode::CREATED, wire.clone())
            .header(header::CONTENT_ENCODING, "gzip"),
    ])
    .await;
    let image = fixture
        .downloader()
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .expect("bounded wire");
    assert_eq!(image.wire, wire);
    assert_eq!(image.decode().expect("decoded asset bytes"), original);
    assert_eq!(
        fixture.requests()[0].headers[header::ACCEPT_ENCODING],
        "identity"
    );
    for (limit, succeeds) in [(256, true), (64, false)] {
        assert!(wire.len() <= limit);
        let fixture = AssetFixture::new(vec![
            AssetReply::status(StatusCode::CREATED, wire.clone())
                .header(header::CONTENT_ENCODING, "gzip"),
        ])
        .await;
        let mut downloader = fixture.downloader();
        downloader.limits.body_bytes = limit;
        let image = downloader
            .download(AssetFixture::url(), StatusCode::OK)
            .await
            .expect("bounded wire");
        if succeeds {
            assert_eq!(image.decode().expect("decoded asset bytes"), original);
        } else {
            let error = image.decode().expect_err_no_debug("decoded size limit");
            assert_error(
                error,
                ImageAssetErrorKind::TooLarge,
                Some(StatusCode::CREATED),
            );
        }
    }

    for (encoding, body) in [
        ("compress", b"unsupported".to_vec()),
        ("gzip", b"broken".to_vec()),
    ] {
        let fixture = AssetFixture::new(vec![
            AssetReply::status(StatusCode::CREATED, body)
                .header(header::CONTENT_ENCODING, encoding),
        ])
        .await;
        let image = fixture
            .downloader()
            .download(AssetFixture::url(), StatusCode::OK)
            .await
            .expect("wire bytes");
        let error = image.decode().expect_err_no_debug("invalid encoding");
        assert_error(
            error,
            ImageAssetErrorKind::DownloadFailed,
            Some(StatusCode::CREATED),
        );
    }
}

#[tokio::test]
async fn cancelling_a_download_closes_the_body_without_an_extra_request() {
    let gate = Arc::new(Notify::new());
    let fixture = AssetFixture::new(vec![AssetReply::gated(b"asset".to_vec(), gate)]).await;
    let downloader = fixture.downloader();
    let url = AssetFixture::url();
    let download = tokio::spawn(async move { downloader.download(url, StatusCode::OK).await });
    fixture.wait_for_requests(1).await;
    download.abort();
    assert!(matches!(download.await, Err(error) if error.is_cancelled()));
    timeout(Duration::from_secs(3), async {
        while fixture.closed_connection_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled connection closes");
    assert_eq!(fixture.request_count(), 1);
    assert_eq!(fixture.connection_count(), 1);
}

#[test]
fn asset_download_ignores_proxy_environment() {
    const CHILD_MARKER: &str = "AI_ROUTER_ASSET_PROXY_TEST_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        tokio::runtime::Runtime::new()
            .expect("fixture runtime")
            .block_on(async {
                let fixture = AssetFixture::new(vec![AssetReply::ok(b"asset".to_vec())]).await;
                assert!(
                    fixture
                        .downloader()
                        .download(AssetFixture::url(), StatusCode::OK)
                        .await
                        .is_ok()
                );
                assert_eq!(fixture.request_count(), 1);
            });
        return;
    }
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "proxy::images::download::tests::asset_download_ignores_proxy_environment",
            "--test-threads=1",
        ])
        .env(CHILD_MARKER, "1")
        .env("NO_PROXY", "")
        .env("no_proxy", "");
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env(name, "http://127.0.0.1:1");
    }
    let output = command.output().expect("isolated proxy-environment test");
    assert!(output.status.success(), "proxy-environment child failed");
    let stdout = std::str::from_utf8(&output.stdout).expect("test output");
    assert!(stdout.contains("1 passed"));
}
