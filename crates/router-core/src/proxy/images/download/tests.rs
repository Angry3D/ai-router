use std::{
    io::Write as _,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    process::Command,
    sync::Arc,
    time::Duration,
};

use flate2::{Compression, write::GzEncoder};
use reqwest::{StatusCode, header};
use tokio::{sync::Notify, time::timeout};

use super::{
    AdmittedUrl, ImageAssetErrorKind, ImageDownloadError, MAX_DNS_ADDRESSES, MAX_URL_BYTES,
    admit_addresses,
    test_support::{AssetFixture, AssetReply, DnsAnswer, PUBLIC_ADDRESS},
};

fn assert_error(error: ImageDownloadError, kind: ImageAssetErrorKind, status: Option<StatusCode>) {
    assert_eq!(error.kind, kind);
    assert_eq!(error.upstream_status, status);
}

#[test]
fn url_admission_rejects_ambiguous_or_credential_bearing_targets() {
    for raw in [
        "",
        "https:assets.example/image.png",
        "https:@assets.example/image.png?next=https://cdn.example",
        "//assets.example/image.png",
        "http://assets.example/image.png",
        "file:///image.png",
        "data:image/png;base64,aGVsbG8=",
        "https://assets.example:8443/image.png",
        "https://user:secret@assets.example/image.png",
        "https://@assets.example/image.png",
        "https:////@assets.example/image.png",
        "https://assets.example/image.png#",
        "https://assets.example/image.png#fragment",
        " https://assets.example/image.png",
        "https://assets.example/image.png ",
        "https://assets.example/\nimage.png",
        "https://assets.example/image.png?signature=secret\u{0085}",
        "https://assets.example\\image.png",
    ] {
        assert!(AdmittedUrl::parse(raw).is_err());
    }
    let base = "https://assets.example/";
    let at_limit = format!("{base}{}", "a".repeat(MAX_URL_BYTES - base.len()));
    assert!(AdmittedUrl::parse(&at_limit).is_ok());
    assert!(AdmittedUrl::parse(&format!("{at_limit}a")).is_err());
    assert!(AdmittedUrl::parse("https://assets.example:443/image.png?signature=allowed").is_ok());
    let target = AdmittedUrl::parse("https://assets.example/image.png").expect("admitted URL");
    assert!(
        target
            .redirect("?return=https://user:secret@cdn.example/image.png")
            .is_ok()
    );
}

#[test]
fn address_admission_covers_special_ranges_and_mapped_ipv6() {
    for raw in [
        "0.0.0.0",
        "0.255.255.255",
        "10.0.0.0",
        "10.255.255.255",
        "100.64.0.0",
        "100.127.255.255",
        "127.0.0.1",
        "127.255.255.255",
        "169.254.0.0",
        "169.254.255.255",
        "172.16.0.0",
        "172.31.255.255",
        "192.0.0.0",
        "192.0.0.255",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.0.1",
        "198.18.0.0",
        "198.19.255.255",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.0",
        "239.255.255.255",
        "240.0.0.0",
        "255.255.255.255",
        "::",
        "::1",
        "::ffff:127.0.0.1",
        "::ffff:192.168.1.1",
        "::ffff:169.254.169.254",
        "64:ff9b::808:808",
        "100::1",
        "2001::1",
        "2001:1ff:ffff::1",
        "2001:db8::1",
        "2002:808:808::1",
        "3fff::1",
        "3fff:fff:ffff::1",
        "4000::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
    ] {
        assert!(
            admit_addresses(vec![SocketAddr::new(raw.parse().expect("test IP"), 443)]).is_err()
        );
    }
    for raw in [
        "1.1.1.1",
        "8.8.8.8",
        "100.63.255.255",
        "100.128.0.0",
        "172.15.255.255",
        "172.32.0.0",
        "192.0.1.1",
        "192.88.100.1",
        "198.17.255.255",
        "198.20.0.0",
        "223.255.255.255",
        "2001:200::1",
        "2001:4860:4860::8888",
        "2606:4700:4700::1111",
        "3fff:1000::1",
        "::ffff:8.8.8.8",
    ] {
        assert!(admit_addresses(vec![SocketAddr::new(raw.parse().expect("test IP"), 443)]).is_ok());
    }
    let mapped: IpAddr = "::ffff:8.8.8.8".parse().expect("mapped IP");
    assert_eq!(
        admit_addresses(vec![SocketAddr::new(mapped, 443)]).expect("public mapped IP"),
        vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 443)]
    );
    assert!(admit_addresses(vec![SocketAddr::new(PUBLIC_ADDRESS, 444)]).is_err());
    assert!(admit_addresses(Vec::new()).is_err());
    assert!(
        admit_addresses(vec![
            SocketAddr::new(PUBLIC_ADDRESS, 443);
            MAX_DNS_ADDRESSES
        ])
        .is_ok()
    );
    assert!(
        admit_addresses(vec![
            SocketAddr::new(PUBLIC_ADDRESS, 443);
            MAX_DNS_ADDRESSES + 1
        ])
        .is_err()
    );
}

#[tokio::test]
async fn nonpublic_literals_are_rejected_before_any_network_operation() {
    let fixture = AssetFixture::new(Vec::new()).await;
    for raw in [
        "https://127.0.0.1/image.png",
        "https://2130706433/image.png",
        "https://0x7f000001/image.png",
        "https://0177.0.0.1/image.png",
        "https://[::1]/image.png",
        "https://[::ffff:127.0.0.1]/image.png",
        "https://[64:ff9b::7f00:1]/image.png",
    ] {
        let error = fixture
            .downloader()
            .download(raw.to_owned(), StatusCode::ACCEPTED)
            .await
            .err()
            .expect("nonpublic target rejected");
        assert_error(
            error,
            ImageAssetErrorKind::InvalidUrl,
            Some(StatusCode::ACCEPTED),
        );
    }
    assert_eq!(fixture.connection_count(), 0);
    assert!(fixture.dns_queries().is_empty());
}

#[tokio::test]
async fn dns_answers_are_checked_in_full_before_pinning() {
    for addresses in [
        Vec::new(),
        vec![PUBLIC_ADDRESS, IpAddr::V4(Ipv4Addr::LOCALHOST)],
        vec![PUBLIC_ADDRESS; MAX_DNS_ADDRESSES + 1],
    ] {
        let fixture = AssetFixture::new(Vec::new()).await;
        fixture.set_dns_answers("assets.example", vec![DnsAnswer::addresses(addresses)]);
        let error = fixture
            .downloader()
            .download(AssetFixture::url(), StatusCode::ACCEPTED)
            .await
            .err()
            .expect("invalid DNS set rejected");
        assert_error(
            error,
            ImageAssetErrorKind::InvalidUrl,
            Some(StatusCode::ACCEPTED),
        );
        assert_eq!(fixture.connection_count(), 0);
        assert!(fixture.pinned_addresses().is_empty());
    }
}

#[tokio::test]
async fn pinned_dns_retains_normalized_host_sni_and_asset_status() {
    let fixture = AssetFixture::new(vec![AssetReply::status(
        StatusCode::CREATED,
        b"asset".to_vec(),
    )])
    .await;
    fixture.set_dns_answers(
        "xn--bcher-kva.example",
        vec![
            DnsAnswer::addresses(vec!["::ffff:93.184.216.34".parse().expect("mapped IP")]),
            DnsAnswer::addresses(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]),
        ],
    );
    let image = fixture
        .downloader()
        .download(
            "HTTPS://BÜCHER.example:443/image.png?signature=fixture".to_owned(),
            StatusCode::OK,
        )
        .await
        .expect("pinned TLS download");
    assert_eq!(image.upstream_status, StatusCode::CREATED);
    assert_eq!(image.decode().expect("identity bytes"), b"asset");
    assert_eq!(fixture.dns_queries(), ["xn--bcher-kva.example"]);
    assert_eq!(
        fixture.pinned_addresses(),
        [vec![SocketAddr::new(PUBLIC_ADDRESS, 443)]]
    );
    assert_eq!(fixture.unexpected_dns_queries(), 0);
    assert_eq!(fixture.server_names(), ["xn--bcher-kva.example"]);
    let requests = fixture.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/image.png?signature=fixture");
    assert_eq!(requests[0].headers[header::HOST], "xn--bcher-kva.example");
    assert_eq!(requests[0].headers[header::ACCEPT_ENCODING], "identity");
}

#[tokio::test]
async fn tls_rejects_a_certificate_for_another_hostname() {
    let fixture = AssetFixture::new(vec![AssetReply::ok(b"asset".to_vec())]).await;
    let error = fixture
        .downloader()
        .download(
            "https://wrong.example/image.png?signature=secret".to_owned(),
            StatusCode::OK,
        )
        .await
        .err()
        .expect("hostname mismatch rejected");
    assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    assert_eq!(fixture.connection_count(), 1);
    assert_eq!(fixture.request_count(), 0);
    assert_eq!(fixture.unexpected_dns_queries(), 0);
    assert!(!format!("{error:?}").contains("secret"));
}

#[tokio::test]
async fn every_redirect_is_fresh_without_cookies_referer_or_credentials() {
    let fixture = AssetFixture::new(vec![
        AssetReply::redirect("/next.png?signature=second")
            .header(header::SET_COOKIE, "credential=secret; Secure"),
        AssetReply::redirect("https://cdn.example/final.png?signature=third"),
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
        .expect("safe redirects");
    assert_eq!(image.decode().expect("asset bytes"), b"asset");
    assert_eq!(
        fixture.dns_queries(),
        ["assets.example", "assets.example", "cdn.example"]
    );
    assert_eq!(fixture.connection_count(), 3);
    assert_eq!(fixture.unexpected_dns_queries(), 0);
    let requests = fixture.requests();
    assert_eq!(requests[1].path, "/next.png?signature=second");
    assert_eq!(requests[2].headers[header::HOST], "cdn.example");
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
async fn all_five_redirect_statuses_are_explicitly_followed() {
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
async fn redirect_targets_cannot_downgrade_or_reach_nonpublic_addresses() {
    for location in [
        "http://cdn.example/image.png",
        "https://127.0.0.1/image.png",
        "https://[::ffff:127.0.0.1]/image.png",
        "https://cdn.example:444/image.png",
        "//user:secret@cdn.example/image.png",
        "//@cdn.example/image.png",
        "https:@cdn.example/image.png?next=https://assets.example",
        "https://internal.example/image.png",
        "/image.png#fragment",
        "file:///image.png",
    ] {
        let fixture = AssetFixture::new(vec![AssetReply::redirect(location)]).await;
        fixture.set_dns_answers(
            "internal.example",
            vec![DnsAnswer::addresses(vec![
                "10.0.0.1".parse().expect("private IP"),
            ])],
        );
        let error = fixture
            .downloader()
            .download(AssetFixture::url(), StatusCode::ACCEPTED)
            .await
            .err()
            .expect("redirect rejected");
        assert_error(
            error,
            ImageAssetErrorKind::InvalidUrl,
            Some(StatusCode::FOUND),
        );
        assert_eq!(fixture.request_count(), 1);
        assert_eq!(fixture.connection_count(), 1);
    }
}

#[tokio::test]
async fn same_host_redirects_revalidate_dns_and_reject_rebinding() {
    let fixture = AssetFixture::new(vec![AssetReply::redirect("/next.png")]).await;
    fixture.set_dns_answers(
        "assets.example",
        vec![
            DnsAnswer::addresses(vec![PUBLIC_ADDRESS]),
            DnsAnswer::addresses(vec![
                PUBLIC_ADDRESS,
                "127.0.0.1".parse().expect("private IP"),
            ]),
        ],
    );
    let error = fixture
        .downloader()
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .err()
        .expect("rebind rejected");
    assert_error(
        error,
        ImageAssetErrorKind::InvalidUrl,
        Some(StatusCode::FOUND),
    );
    assert_eq!(fixture.dns_queries(), ["assets.example", "assets.example"]);
    assert_eq!(fixture.pinned_addresses().len(), 1);
    assert_eq!(fixture.unexpected_dns_queries(), 0);
    assert_eq!(fixture.request_count(), 1);
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
            .err()
            .expect("unusable redirect");
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
        .err()
        .expect("hop limit");
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
    let fixture = AssetFixture::new(vec![
        AssetReply::status(
            StatusCode::SERVICE_UNAVAILABLE,
            b"secret-error-body".to_vec(),
        )
        .declared_length(super::MAX_COMPRESSED_PNG_BYTES + 1),
    ])
    .await;
    let error = fixture
        .downloader()
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .err()
        .expect("HTTP failure");
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
        .err()
        .expect("disconnect failure");
    assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    assert_eq!(fixture.request_count(), 1);
    assert_eq!(fixture.connection_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn dns_failures_and_both_dns_and_total_deadlines_have_no_http_status() {
    for (dns_timeout, total_timeout, fails) in [
        (Duration::from_secs(10), Duration::from_mins(10), true),
        (Duration::from_secs(10), Duration::from_mins(10), false),
        (Duration::from_secs(10), Duration::from_secs(1), false),
    ] {
        let fixture = AssetFixture::new(Vec::new()).await;
        fixture.set_dns_answers(
            "assets.example",
            vec![DnsAnswer {
                addresses: vec![PUBLIC_ADDRESS],
                delay: if fails {
                    Duration::ZERO
                } else {
                    Duration::from_mins(1)
                },
                fails,
            }],
        );
        let mut downloader = fixture.downloader();
        downloader.limits.dns_timeout = dns_timeout;
        downloader.limits.total_timeout = total_timeout;
        let error = downloader
            .download(AssetFixture::url(), StatusCode::OK)
            .await
            .err()
            .expect("DNS failure");
        assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
        assert_eq!(fixture.connection_count(), 0);
    }
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
        .err()
        .expect("TLS deadline");
    assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    assert_eq!(fixture.connection_count(), 1);
    assert_eq!(fixture.request_count(), 0);
}

#[tokio::test]
async fn total_deadline_covers_delayed_headers_and_continuously_slow_body() {
    let fixture = AssetFixture::new(vec![
        AssetReply::ok(b"asset".to_vec()).delayed_headers(Duration::from_secs(5)),
    ])
    .await;
    let mut downloader = fixture.downloader();
    downloader.limits.total_timeout = Duration::from_millis(400);
    let error = downloader
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .err()
        .expect("header deadline");
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
        .err()
        .expect("body deadline");
    assert_error(
        error,
        ImageAssetErrorKind::DownloadFailed,
        Some(StatusCode::CREATED),
    );
    assert_eq!(fixture.request_count(), 1);
}

#[tokio::test]
async fn redirects_do_not_reset_the_download_deadline() {
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
        .err()
        .expect("shared deadline");
    assert_error(error, ImageAssetErrorKind::DownloadFailed, None);
    assert_eq!(fixture.request_count(), 2);
}

#[tokio::test]
async fn wire_limit_checks_declared_and_actual_chunked_bytes() {
    for reply in [
        AssetReply::ok(vec![1; 9]),
        AssetReply::ok(vec![1; 9]).chunked(2, Duration::ZERO),
        AssetReply::ok(vec![1]).declared_length(9),
    ] {
        let fixture = AssetFixture::new(vec![reply]).await;
        let mut downloader = fixture.downloader();
        downloader.limits.body_bytes = 8;
        let error = downloader
            .download(AssetFixture::url(), StatusCode::ACCEPTED)
            .await
            .err()
            .expect("wire limit");
        assert_error(error, ImageAssetErrorKind::TooLarge, Some(StatusCode::OK));
        assert_eq!(fixture.request_count(), 1);
    }
    let fixture = AssetFixture::new(vec![
        AssetReply::ok(vec![1; 9])
            .chunked(2, Duration::ZERO)
            .header(header::CONTENT_LENGTH, "1"),
    ])
    .await;
    let mut downloader = fixture.downloader();
    downloader.limits.body_bytes = 8;
    let error = downloader
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .err()
        .expect("forged length rejected");
    assert!(matches!(
        error.kind,
        ImageAssetErrorKind::TooLarge | ImageAssetErrorKind::DownloadFailed
    ));
    assert_eq!(fixture.request_count(), 1);
}

#[tokio::test]
async fn incomplete_body_keeps_the_asset_response_status() {
    let fixture = AssetFixture::new(vec![
        AssetReply::status(StatusCode::PARTIAL_CONTENT, b"short".to_vec()).declared_length(20),
    ])
    .await;
    let error = fixture
        .downloader()
        .download(AssetFixture::url(), StatusCode::OK)
        .await
        .err()
        .expect("truncated body");
    assert_error(
        error,
        ImageAssetErrorKind::DownloadFailed,
        Some(StatusCode::PARTIAL_CONTENT),
    );
    assert_eq!(fixture.request_count(), 1);
}

fn gzip(body: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(body).expect("synthetic gzip");
    encoder.finish().expect("synthetic gzip trailer")
}

#[tokio::test]
async fn explicit_content_decoding_is_bounded_even_when_identity_was_requested() {
    let original = vec![7; 128];
    let wire = gzip(&original);
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
        assert_eq!(image.wire, wire);
        if succeeds {
            assert_eq!(image.decode().expect("decoded asset bytes"), original);
        } else {
            assert_error(
                image.decode().expect_err("decoded size limit"),
                ImageAssetErrorKind::TooLarge,
                Some(StatusCode::CREATED),
            );
        }
        assert_eq!(
            fixture.requests()[0].headers[header::ACCEPT_ENCODING],
            "identity"
        );
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
        assert_error(
            image.decode().expect_err("invalid encoding"),
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
                assert_eq!(fixture.unexpected_dns_queries(), 0);
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
