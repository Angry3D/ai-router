use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};

use arc_swap::ArcSwap;
use reqwest::{Client, ClientBuilder, Proxy, Url};

use crate::domain::OutboundProxyConfig;

struct OutboundProxySnapshot {
    revision: u64,
    endpoint: Option<Arc<Url>>,
}

#[derive(Clone)]
pub struct OutboundProxyTransport {
    snapshot: Arc<ArcSwap<OutboundProxySnapshot>>,
}

impl Default for OutboundProxyTransport {
    fn default() -> Self {
        Self {
            snapshot: Arc::new(ArcSwap::from_pointee(OutboundProxySnapshot {
                revision: 0,
                endpoint: None,
            })),
        }
    }
}

impl OutboundProxyTransport {
    #[must_use]
    pub fn endpoint(&self) -> Option<Arc<Url>> {
        self.snapshot.load().endpoint.clone()
    }

    pub fn set_endpoint(&self, endpoint: Option<Url>) {
        let revision = self.snapshot.load().revision.wrapping_add(1);
        self.snapshot.store(Arc::new(OutboundProxySnapshot {
            revision,
            endpoint: endpoint.map(Arc::new),
        }));
    }

    /// Applies validated durable settings without rebuilding any client.
    ///
    /// # Errors
    ///
    /// Returns a URL parse error if a value that bypassed the domain boundary
    /// reaches the transport.
    pub fn apply(&self, config: &OutboundProxyConfig) -> Result<(), url::ParseError> {
        let endpoint = Self::endpoint_from_config(config)?;
        self.set_endpoint(endpoint);
        Ok(())
    }

    /// Prepares a transport endpoint before a durable settings write.
    ///
    /// # Errors
    ///
    /// Returns a URL parse error if a value that bypassed the domain boundary
    /// reaches the transport.
    pub fn endpoint_from_config(
        config: &OutboundProxyConfig,
    ) -> Result<Option<Url>, url::ParseError> {
        if config.enabled() {
            config.url().map(|url| Url::parse(url.as_str())).transpose()
        } else {
            Ok(None)
        }
    }

    pub fn configure_current_client(&self, builder: ClientBuilder) -> ClientBuilder {
        configure_client(builder, self.snapshot.load().endpoint.clone())
    }
}

fn configure_client(builder: ClientBuilder, endpoint: Option<Arc<Url>>) -> ClientBuilder {
    builder.proxy(Proxy::custom(move |target| {
        if is_loopback_target(target) {
            None
        } else {
            endpoint.as_deref().cloned()
        }
    }))
}

struct VersionedClient {
    revision: u64,
    client: Client,
}

#[derive(Clone)]
pub struct OutboundHttpClient {
    transport: OutboundProxyTransport,
    builder: Arc<dyn Fn() -> ClientBuilder + Send + Sync>,
    current: Arc<ArcSwap<VersionedClient>>,
    refresh_gate: Arc<Mutex<()>>,
}

impl OutboundHttpClient {
    /// Creates a client handle that replaces its connection pool after each
    /// proxy configuration revision.
    ///
    /// # Errors
    ///
    /// Returns a Reqwest client construction error.
    pub fn new<F>(transport: OutboundProxyTransport, builder: F) -> Result<Self, reqwest::Error>
    where
        F: Fn() -> ClientBuilder + Send + Sync + 'static,
    {
        let builder = Arc::new(builder);
        let snapshot = transport.snapshot.load_full();
        let client = configure_client(builder(), snapshot.endpoint.clone()).build()?;
        Ok(Self {
            transport,
            builder,
            current: Arc::new(ArcSwap::from_pointee(VersionedClient {
                revision: snapshot.revision,
                client,
            })),
            refresh_gate: Arc::new(Mutex::new(())),
        })
    }

    /// Returns a client bound to the latest proxy revision.
    ///
    /// # Errors
    ///
    /// Returns a Reqwest client construction error while replacing the pool.
    pub fn client(&self) -> Result<Client, reqwest::Error> {
        let snapshot = self.transport.snapshot.load_full();
        let current = self.current.load_full();
        if current.revision == snapshot.revision {
            return Ok(current.client.clone());
        }
        drop(current);

        let _refresh = self
            .refresh_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = self.transport.snapshot.load_full();
        let current = self.current.load_full();
        if current.revision == snapshot.revision {
            return Ok(current.client.clone());
        }
        let client = configure_client((self.builder)(), snapshot.endpoint.clone()).build()?;
        self.current.store(Arc::new(VersionedClient {
            revision: snapshot.revision,
            client: client.clone(),
        }));
        Ok(client)
    }
}

fn is_loopback_target(target: &Url) -> bool {
    let Some(host) = target.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return true;
    }
    let ip_literal = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    ip_literal
        .parse::<IpAddr>()
        .is_ok_and(|address| match address {
            IpAddr::V4(address) => address.is_loopback(),
            IpAddr::V6(address) => {
                address.is_loopback() || address.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
            }
        })
}

#[cfg(test)]
mod tests {
    use std::{net::Ipv4Addr, time::Duration};

    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpListener,
    };

    use super::*;

    async fn response_server(
        body: &'static str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept fixture request");
            let mut request = vec![0_u8; 4_096];
            let count = stream
                .read(&mut request)
                .await
                .expect("read fixture request");
            request.truncate(count);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write fixture response");
            request
        });
        (address, task)
    }

    fn client(transport: &OutboundProxyTransport) -> OutboundHttpClient {
        OutboundHttpClient::new(transport.clone(), reqwest::Client::builder)
            .expect("build fixture client")
    }

    #[test]
    fn loopback_detection_covers_named_ipv4_ipv6_and_mapped_targets() {
        for target in [
            "http://localhost/resource",
            "http://LOCALHOST./resource",
            "http://service.localhost/resource",
            "http://127.0.0.42/resource",
            "http://[::1]/resource",
            "http://[::ffff:127.0.0.1]/resource",
        ] {
            assert!(is_loopback_target(&Url::parse(target).expect("target URL")));
        }
        assert!(!is_loopback_target(
            &Url::parse("https://example.com/resource").expect("target URL")
        ));
    }

    #[test]
    fn supported_proxy_schemes_build_without_exposing_environment_proxies() {
        for endpoint in [
            "http://127.0.0.1:7890",
            "https://127.0.0.1:7890",
            "socks5://127.0.0.1:7890",
            "socks5h://127.0.0.1:7890",
        ] {
            let transport = OutboundProxyTransport::default();
            transport.set_endpoint(Some(Url::parse(endpoint).expect("proxy URL")));
            let _client = client(&transport);
        }
    }

    #[tokio::test]
    async fn enabled_proxy_intercepts_external_requests_and_hot_updates() {
        let transport = OutboundProxyTransport::default();
        let (direct_address, direct_request) = response_server("direct").await;
        let client = OutboundHttpClient::new(transport.clone(), move || {
            reqwest::Client::builder().resolve("upstream.invalid", direct_address)
        })
        .expect("build versioned client");

        let (first_address, first_request) = response_server("first").await;
        transport.set_endpoint(Some(
            Url::parse(&format!("http://{first_address}")).expect("first proxy URL"),
        ));
        let first = client
            .client()
            .expect("current first client")
            .get("http://upstream.invalid/one")
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .expect("first proxied response")
            .text()
            .await
            .expect("first response body");
        assert_eq!(first, "first");
        let first_request = first_request.await.expect("first proxy task");
        assert!(first_request.starts_with(b"GET http://upstream.invalid/one HTTP/1.1"));

        let (second_address, second_request) = response_server("second").await;
        transport.set_endpoint(Some(
            Url::parse(&format!("http://{second_address}")).expect("second proxy URL"),
        ));
        let second = client
            .client()
            .expect("current second client")
            .get("http://upstream.invalid/two")
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .expect("second proxied response")
            .text()
            .await
            .expect("second response body");
        assert_eq!(second, "second");
        let second_request = second_request.await.expect("second proxy task");
        assert!(second_request.starts_with(b"GET http://upstream.invalid/two HTTP/1.1"));

        transport.set_endpoint(None);
        let direct = client
            .client()
            .expect("current direct client")
            .get("http://upstream.invalid/direct")
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .expect("direct response")
            .text()
            .await
            .expect("direct response body");
        assert_eq!(direct, "direct");
        let direct_request = direct_request.await.expect("direct target task");
        assert!(direct_request.starts_with(b"GET /direct HTTP/1.1"));
    }

    #[tokio::test]
    async fn loopback_requests_bypass_an_enabled_proxy() {
        let transport = OutboundProxyTransport::default();
        let (proxy_address, proxy_request) = response_server("proxy").await;
        transport.set_endpoint(Some(
            Url::parse(&format!("http://{proxy_address}")).expect("proxy URL"),
        ));

        let (target_address, target_request) = response_server("direct").await;
        let response = client(&transport)
            .client()
            .expect("current direct client")
            .get(format!("http://{target_address}/loopback"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .expect("direct response")
            .text()
            .await
            .expect("direct response body");
        assert_eq!(response, "direct");
        let target_request = target_request.await.expect("target task");
        assert!(target_request.starts_with(b"GET /loopback HTTP/1.1"));

        proxy_request.abort();
    }
}
