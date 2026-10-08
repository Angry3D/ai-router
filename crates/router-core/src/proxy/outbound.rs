use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};

use arc_swap::ArcSwap;
use reqwest::{Client, ClientBuilder, Proxy, Url};

use crate::domain::{OutboundProxyConfig, OutboundProxyUrl};

/// Static manual settings supplied by desktop composition, never host discovery.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct SystemProxySettings {
    pub http: Option<Url>,
    pub https: Option<Url>,
    pub socks: Option<Url>,
    pub bypass: Vec<String>,
    pub exclude_simple_hostnames: bool,
    pub automatic_enabled: bool,
}

impl std::fmt::Debug for SystemProxySettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SystemProxySettings")
            .field("http_enabled", &self.http.is_some())
            .field("https_enabled", &self.https.is_some())
            .field("socks_enabled", &self.socks.is_some())
            .field("bypass_count", &self.bypass.len())
            .field("exclude_simple_hostnames", &self.exclude_simple_hostnames)
            .field("automatic_enabled", &self.automatic_enabled)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SystemProxyError {
    #[error(
        "System proxy settings could not be read. Review system settings or use Custom proxy mode."
    )]
    ReadFailed,
    #[error("System proxy settings are invalid. Review system settings or use Custom proxy mode.")]
    InvalidSettings,
    #[error(
        "Automatic proxy routing (PAC/WPAD) is unsupported for this destination. Configure a manual system proxy or use Custom proxy mode."
    )]
    AutomaticProxyUnsupported,
}

impl SystemProxyError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ReadFailed => "system_proxy_read_failed",
            Self::InvalidSettings => "system_proxy_invalid",
            Self::AutomaticProxyUnsupported => "system_proxy_automatic_unsupported",
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::ReadFailed => {
                "System proxy settings could not be read. Review system settings or use Custom proxy mode."
            }
            Self::InvalidSettings => {
                "System proxy settings are invalid. Review system settings or use Custom proxy mode."
            }
            Self::AutomaticProxyUnsupported => {
                "Automatic proxy routing (PAC/WPAD) is unsupported for this destination. Configure a manual system proxy or use Custom proxy mode."
            }
        }
    }

    /// Recovers only our typed redirect error, never transport error text.
    #[must_use]
    pub fn from_reqwest(error: &reqwest::Error) -> Option<Self> {
        let mut source: &(dyn std::error::Error + 'static) = error;
        loop {
            if let Some(error) = source.downcast_ref::<Self>() {
                return Some(*error);
            }
            source = source.source()?;
        }
    }
}

#[derive(thiserror::Error)]
pub enum OutboundClientError {
    #[error(transparent)]
    SystemProxy(#[from] SystemProxyError),
    #[error("The outbound HTTP client could not be prepared.")]
    Client(#[from] reqwest::Error),
    #[error("The outbound request URL is invalid.")]
    InvalidTarget(#[from] url::ParseError),
}

impl std::fmt::Debug for OutboundClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

struct SystemProxyPolicy {
    settings: SystemProxySettings,
    bypass: Vec<BypassRule>,
}

impl SystemProxyPolicy {
    fn new(settings: SystemProxySettings) -> Result<Self, SystemProxyError> {
        for (endpoint, socks) in [
            (&settings.http, false),
            (&settings.https, false),
            (&settings.socks, true),
        ] {
            if let Some(endpoint) = endpoint {
                let scheme_valid = if socks {
                    matches!(endpoint.scheme(), "socks5" | "socks5h")
                } else {
                    matches!(endpoint.scheme(), "http" | "https")
                };
                if !scheme_valid || OutboundProxyUrl::parse(endpoint.as_str()).is_err() {
                    return Err(SystemProxyError::InvalidSettings);
                }
            }
        }
        let bypass = settings
            .bypass
            .iter()
            .filter_map(|rule| BypassRule::parse(rule))
            .collect();
        Ok(Self { settings, bypass })
    }

    fn endpoint(&self, target: &Url) -> Option<&Url> {
        match target.scheme() {
            "http" => self.settings.http.as_ref(),
            "https" => self.settings.https.as_ref(),
            _ => None,
        }
        .or(self.settings.socks.as_ref())
    }

    fn bypasses(&self, target: &Url) -> bool {
        let Some(host) = target.host_str() else {
            return false;
        };
        let host = host.trim_end_matches('.');
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        // CFNetwork's simple-host exclusion is dot-based, including IPv6.
        (self.settings.exclude_simple_hostnames && !host.contains('.'))
            || self.bypass.iter().any(|rule| rule.matches(host))
    }
}

enum BypassRule {
    Address(IpAddr),
    Subnet { network: u32, mask: u32 },
    Host(String),
}

impl BypassRule {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim_end_matches('.');
        if let Ok(address) = value.parse::<IpAddr>() {
            return Some(Self::Address(address));
        }
        if let Some((network, prefix)) = value.split_once('/') {
            // macOS static exceptions support IPv4 CIDR, not IPv6 CIDR or
            // dotted netmasks. Unrecognized exceptions are inert, not bypasses.
            let network = network.parse::<std::net::Ipv4Addr>().ok()?;
            let prefix = prefix.parse::<u32>().ok().filter(|prefix| *prefix <= 32)?;
            let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
            return Some(Self::Subnet {
                network: u32::from(network) & mask,
                mask,
            });
        }
        if value.is_empty() || value == "*" || value.contains(['[', ']', ':']) {
            return None;
        }
        let pattern = if value.starts_with('.') {
            format!("*{value}")
        } else {
            value.to_owned()
        };
        if pattern
            .split('.')
            .any(|label| label.is_empty() || (label.contains('*') && label != "*"))
        {
            return None;
        }
        Some(Self::Host(pattern))
    }

    fn matches(&self, host: &str) -> bool {
        match self {
            Self::Address(address) => host.parse::<IpAddr>().is_ok_and(|host| host == *address),
            Self::Subnet { network, mask } => host
                .parse::<std::net::Ipv4Addr>()
                .is_ok_and(|address| u32::from(address) & mask == *network),
            Self::Host(pattern) => host_pattern_matches(pattern, host),
        }
    }
}

fn host_pattern_matches(pattern: &str, host: &str) -> bool {
    let (label, remaining_pattern) = pattern
        .split_once('.')
        .map_or((pattern, None), |(label, rest)| (label, Some(rest)));
    let (host_label, remaining_host) = host
        .split_once('.')
        .map_or((host, None), |(label, rest)| (label, Some(rest)));
    if label == "*" {
        if host_label.is_empty() {
            return false;
        }
        let Some(remaining_pattern) = remaining_pattern else {
            return true;
        };
        let mut suffix = remaining_host;
        while let Some(rest) = suffix {
            if host_pattern_matches(remaining_pattern, rest) {
                return true;
            }
            suffix = rest.split_once('.').map(|(_, rest)| rest);
        }
        false
    } else if label.eq_ignore_ascii_case(host_label) {
        match (remaining_pattern, remaining_host) {
            (Some(pattern), Some(host)) => host_pattern_matches(pattern, host),
            (None, None) => true,
            _ => false,
        }
    } else {
        false
    }
}

struct OutboundProxySnapshot {
    revision: u64,
    endpoint: Option<Arc<Url>>,
    system: Result<Arc<SystemProxyPolicy>, SystemProxyError>,
}

/// One immutable routing decision context, including its redirect guards.
#[derive(Clone)]
pub struct OutboundProxyPolicy {
    snapshot: Arc<OutboundProxySnapshot>,
}

#[derive(Clone)]
pub struct OutboundProxyTransport {
    snapshot: Arc<ArcSwap<OutboundProxySnapshot>>,
    update_gate: Arc<Mutex<()>>,
}

impl Default for OutboundProxyTransport {
    fn default() -> Self {
        Self {
            snapshot: Arc::new(ArcSwap::from_pointee(OutboundProxySnapshot {
                revision: 0,
                endpoint: None,
                system: Ok(Arc::new(SystemProxyPolicy {
                    settings: SystemProxySettings::default(),
                    bypass: Vec::new(),
                })),
            })),
            update_gate: Arc::new(Mutex::new(())),
        }
    }
}

impl OutboundProxyTransport {
    #[must_use]
    pub fn endpoint(&self) -> Option<Arc<Url>> {
        self.snapshot.load().endpoint.clone()
    }

    pub fn set_endpoint(&self, endpoint: Option<Url>) {
        let _update = self
            .update_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.snapshot.load_full();
        if current.endpoint.as_deref() == endpoint.as_ref() {
            return;
        }
        self.snapshot.store(Arc::new(OutboundProxySnapshot {
            revision: current.revision.wrapping_add(1),
            endpoint: endpoint.map(Arc::new),
            system: current.system.clone(),
        }));
    }

    /// Publishes a native settings read, retaining failures as fail-closed state.
    pub fn set_system_settings(&self, settings: Result<SystemProxySettings, SystemProxyError>) {
        let _update = self
            .update_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.snapshot.load_full();
        let unchanged = match (&current.system, &settings) {
            (Ok(previous), Ok(next)) => previous.settings == *next,
            (Err(previous), Err(next)) => previous == next,
            _ => false,
        };
        if unchanged {
            return;
        }
        let system = settings.and_then(SystemProxyPolicy::new).map(Arc::new);
        if matches!((&current.system, &system), (Err(previous), Err(next)) if previous == next) {
            return;
        }
        self.snapshot.store(Arc::new(OutboundProxySnapshot {
            revision: current.revision.wrapping_add(1),
            endpoint: current.endpoint.clone(),
            system,
        }));
    }

    #[must_use]
    pub fn policy(&self) -> OutboundProxyPolicy {
        OutboundProxyPolicy {
            snapshot: self.snapshot.load_full(),
        }
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
}

impl OutboundProxyPolicy {
    /// Checks the initial destination before handing a client to a consumer.
    ///
    /// # Errors
    /// Returns a bounded settings error, never permission for direct fallback.
    pub fn validate_target(&self, target: &Url) -> Result<(), SystemProxyError> {
        if is_loopback_target(target) || self.snapshot.endpoint.is_some() {
            return Ok(());
        }
        let system = self.snapshot.system.as_ref().map_err(|error| *error)?;
        if system.bypasses(target)
            || system.endpoint(target).is_some()
            || !system.settings.automatic_enabled
        {
            Ok(())
        } else {
            Err(SystemProxyError::AutomaticProxyUnsupported)
        }
    }

    /// Configures explicit routing and wraps the consumer's redirect policy.
    /// The caller MUST validate each initial URL with this same snapshot before
    /// sending; the proxy callback cannot express application-policy errors.
    pub fn configure_client(
        &self,
        builder: ClientBuilder,
        redirects: reqwest::redirect::Policy,
    ) -> ClientBuilder {
        let proxy_policy = self.clone();
        let redirect_policy = self.clone();
        builder
            .no_proxy()
            .proxy(Proxy::custom(move |target| {
                proxy_policy.endpoint(target).cloned()
            }))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if let Err(error) = redirect_policy.validate_target(attempt.url()) {
                    return attempt.error(error);
                }
                redirects.redirect(attempt)
            }))
    }

    // Admission is separate from routing: errors are rejected before send or
    // redirect, not translated into Proxy::custom's direct-route `None`.
    fn endpoint(&self, target: &Url) -> Option<&Url> {
        if is_loopback_target(target) {
            return None;
        }
        if let Some(endpoint) = self.snapshot.endpoint.as_deref() {
            return Some(endpoint);
        }
        if let Ok(system) = &self.snapshot.system
            && !system.bypasses(target)
        {
            return system.endpoint(target);
        }
        None
    }
}

struct VersionedClient {
    policy: OutboundProxyPolicy,
    client: Client,
}

#[derive(Clone)]
pub struct OutboundHttpClient {
    transport: OutboundProxyTransport,
    builder: Arc<dyn Fn() -> ClientBuilder + Send + Sync>,
    redirects: Arc<dyn Fn() -> reqwest::redirect::Policy + Send + Sync>,
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
        Self::new_with_redirect_policy(transport, builder, reqwest::redirect::Policy::default)
    }

    /// Creates a pooled client while preserving the consumer's redirect policy.
    ///
    /// # Errors
    /// Returns a Reqwest client construction error.
    pub fn new_with_redirect_policy<F, R>(
        transport: OutboundProxyTransport,
        builder: F,
        redirects: R,
    ) -> Result<Self, reqwest::Error>
    where
        F: Fn() -> ClientBuilder + Send + Sync + 'static,
        R: Fn() -> reqwest::redirect::Policy + Send + Sync + 'static,
    {
        let builder = Arc::new(builder);
        let redirects = Arc::new(redirects);
        let policy = transport.policy();
        let client = policy.configure_client(builder(), redirects()).build()?;
        Ok(Self {
            transport,
            builder,
            redirects,
            current: Arc::new(ArcSwap::from_pointee(VersionedClient { policy, client })),
            refresh_gate: Arc::new(Mutex::new(())),
        })
    }

    /// Returns a client bound to the latest revision after target admission.
    ///
    /// # Errors
    /// Returns a bounded system-policy or client-construction error.
    pub fn client_for(&self, target: &Url) -> Result<Client, OutboundClientError> {
        let snapshot = self.transport.snapshot.load_full();
        let current = self.current.load_full();
        if current.policy.snapshot.revision == snapshot.revision {
            current.policy.validate_target(target)?;
            return Ok(current.client.clone());
        }
        drop(current);

        let _refresh = self
            .refresh_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let policy = self.transport.policy();
        policy.validate_target(target)?;
        let current = self.current.load_full();
        if current.policy.snapshot.revision == policy.snapshot.revision {
            return Ok(current.client.clone());
        }
        let client = policy
            .configure_client((self.builder)(), (self.redirects)())
            .build()?;
        self.current.store(Arc::new(VersionedClient {
            policy,
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
            .client_for(&Url::parse("http://upstream.invalid/one").expect("target"))
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
            .client_for(&Url::parse("http://upstream.invalid/two").expect("target"))
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
            .client_for(&Url::parse("http://upstream.invalid/direct").expect("target"))
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
            .client_for(&Url::parse(&format!("http://{target_address}/loopback")).expect("target"))
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

    #[test]
    fn manual_protocol_precedence_and_custom_override_ignore_automatic_rules() {
        let transport = OutboundProxyTransport::default();
        let http = Url::parse("http://127.0.0.1:8101").expect("HTTP proxy");
        let https = Url::parse("http://127.0.0.1:8102").expect("HTTPS proxy");
        let socks = Url::parse("socks5h://127.0.0.1:8103").expect("SOCKS proxy");
        transport.set_system_settings(Ok(SystemProxySettings {
            http: Some(http.clone()),
            https: Some(https.clone()),
            socks: Some(socks.clone()),
            automatic_enabled: true,
            ..SystemProxySettings::default()
        }));
        for (target, endpoint) in [
            ("http://upstream.invalid", &http),
            ("https://upstream.invalid", &https),
        ] {
            let target = Url::parse(target).expect("target");
            assert_eq!(transport.policy().validate_target(&target), Ok(()));
            assert_eq!(transport.policy().endpoint(&target), Some(endpoint));
        }
        transport.set_system_settings(Ok(SystemProxySettings {
            socks: Some(socks.clone()),
            automatic_enabled: true,
            ..SystemProxySettings::default()
        }));
        let target = Url::parse("https://upstream.invalid").expect("target");
        assert_eq!(transport.policy().endpoint(&target), Some(&socks));
        transport.set_system_settings(Err(SystemProxyError::ReadFailed));
        transport.set_endpoint(Some(http.clone()));
        assert_eq!(transport.policy().validate_target(&target), Ok(()));
        assert_eq!(transport.policy().endpoint(&target), Some(&http));
        transport.set_endpoint(None);
        assert_eq!(
            transport.policy().validate_target(&target),
            Err(SystemProxyError::ReadFailed)
        );
        assert_eq!(
            transport
                .policy()
                .validate_target(&Url::parse("http://[::1]").expect("loopback")),
            Ok(())
        );
    }

    #[test]
    fn system_exceptions_match_native_label_and_subnet_semantics() {
        for (rule, target, bypasses) in [
            ("example.invalid", "example.invalid", true),
            ("example.invalid", "api.example.invalid", false),
            (".example.invalid", "example.invalid", false),
            (".example.invalid", "a.b.example.invalid", true),
            ("*.example.invalid", "example.invalid", false),
            ("*.example.invalid", "a.b.example.invalid", true),
            ("*example.invalid", "example.invalid", false),
            ("*", "printer", false),
            ("api.*.invalid", "api.a.b.invalid", true),
            ("API.EXAMPLE.INVALID.", "api.example.invalid.", true),
            ("<local>", "printer", false),
            ("192.0.2.0/24", "192.0.2.9", true),
            ("192.0.2.0/24", "192.0.3.9", false),
            ("192.0.2.0/255.255.255.0", "192.0.2.9", false),
            ("192.0.2.*", "192.0.2.9", true),
            ("2001:db8::9", "[2001:db8::9]", true),
            ("[2001:db8::9]", "[2001:db8::9]", false),
            ("2001:db8::/32", "[2001:db8::9]", false),
        ] {
            let transport = OutboundProxyTransport::default();
            transport.set_system_settings(Ok(SystemProxySettings {
                bypass: vec![rule.to_owned()],
                automatic_enabled: true,
                ..SystemProxySettings::default()
            }));
            let target = Url::parse(&format!("https://{target}/")).expect("target");
            assert_eq!(
                transport.policy().validate_target(&target).is_ok(),
                bypasses,
                "rule {rule}"
            );
        }
        let transport = OutboundProxyTransport::default();
        transport.set_system_settings(Ok(SystemProxySettings {
            exclude_simple_hostnames: true,
            automatic_enabled: true,
            ..SystemProxySettings::default()
        }));
        for (host, allowed) in [
            ("printer", true),
            ("printer.local", false),
            ("192.0.2.9", false),
            ("[2001:db8::9]", true),
        ] {
            assert_eq!(
                transport
                    .policy()
                    .validate_target(&Url::parse(&format!("https://{host}")).expect("target"))
                    .is_ok(),
                allowed
            );
        }
    }

    #[tokio::test]
    async fn read_invalid_and_automatic_only_states_fail_without_direct_traffic() {
        let direct = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("direct fixture");
        let address = direct.local_addr().expect("direct address");
        let transport = OutboundProxyTransport::default();
        let client = OutboundHttpClient::new(transport.clone(), move || {
            Client::builder().resolve("upstream.invalid", address)
        })
        .expect("client");
        let target = Url::parse("http://upstream.invalid").expect("target");
        for (state, expected) in [
            (
                Err(SystemProxyError::ReadFailed),
                SystemProxyError::ReadFailed,
            ),
            (
                Ok(SystemProxySettings {
                    automatic_enabled: true,
                    ..SystemProxySettings::default()
                }),
                SystemProxyError::AutomaticProxyUnsupported,
            ),
            (
                Ok(SystemProxySettings {
                    http: Some(
                        Url::parse("http://secret:password@127.0.0.1:80").expect("malformed proxy"),
                    ),
                    ..SystemProxySettings::default()
                }),
                SystemProxyError::InvalidSettings,
            ),
        ] {
            transport.set_system_settings(state);
            assert!(
                matches!(client.client_for(&target), Err(OutboundClientError::SystemProxy(error)) if error == expected)
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(50), direct.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn failed_selected_proxy_never_falls_back_to_resolved_direct_target() {
        let direct = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("direct fixture");
        let unavailable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("unused port");
        let proxy_address = unavailable.local_addr().expect("unused address");
        drop(unavailable);
        let address = direct.local_addr().expect("direct address");
        let transport = OutboundProxyTransport::default();
        transport.set_system_settings(Ok(SystemProxySettings {
            http: Some(Url::parse(&format!("http://{proxy_address}")).expect("proxy")),
            ..SystemProxySettings::default()
        }));
        let client = OutboundHttpClient::new(transport, move || {
            Client::builder().resolve("upstream.invalid", address)
        })
        .expect("client");
        let target = Url::parse("http://upstream.invalid").expect("target");
        assert!(
            client
                .client_for(&target)
                .expect("manual route")
                .get(target)
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), direct.accept())
                .await
                .is_err()
        );
    }

    async fn redirect_server(
        location: String,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("redirect fixture");
        let address = listener.local_addr().expect("redirect address");
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("redirect request");
            let mut request = [0_u8; 4096];
            let mut read = 0;
            while !request[..read].ends_with(b"\r\n\r\n") {
                assert!(read < request.len(), "bounded redirect request headers");
                let count = socket
                    .read(&mut request[read..])
                    .await
                    .expect("read request");
                assert!(count > 0, "complete redirect request headers");
                read += count;
            }
            socket.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.expect("redirect response");
        });
        (address, task)
    }

    #[tokio::test]
    async fn loopback_redirect_cannot_escape_unsupported_system_policy() {
        let direct = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("direct fixture");
        let direct_address = direct.local_addr().expect("direct address");
        for state in [
            Err(SystemProxyError::ReadFailed),
            Ok(SystemProxySettings {
                automatic_enabled: true,
                ..SystemProxySettings::default()
            }),
        ] {
            let (address, task) = redirect_server("http://external.invalid/asset".to_owned()).await;
            let transport = OutboundProxyTransport::default();
            transport.set_system_settings(state);
            let client = OutboundHttpClient::new(transport, move || {
                Client::builder().resolve("external.invalid", direct_address)
            })
            .expect("client");
            let target = Url::parse(&format!("http://{address}/start")).expect("target");
            let error = client
                .client_for(&target)
                .expect("loopback allowed")
                .get(target)
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .expect_err("redirect blocked");
            assert!(SystemProxyError::from_reqwest(&error).is_some());
            task.await.expect("redirect task");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(50), direct.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn redirects_reselect_proxy_or_explicit_bypass_with_automatic_enabled() {
        let (direct_address, direct_request) = response_server("bypassed").await;
        let (proxy_address, proxy_task) =
            redirect_server("http://bypass.invalid/final".to_owned()).await;
        let transport = OutboundProxyTransport::default();
        transport.set_system_settings(Ok(SystemProxySettings {
            http: Some(Url::parse(&format!("http://{proxy_address}")).expect("proxy")),
            bypass: vec!["bypass.invalid".to_owned()],
            automatic_enabled: true,
            ..SystemProxySettings::default()
        }));
        let client = OutboundHttpClient::new(transport, move || {
            Client::builder().resolve("bypass.invalid", direct_address)
        })
        .expect("client");
        let target = Url::parse("http://proxied.invalid/start").expect("target");
        let body = client
            .client_for(&target)
            .expect("manual route")
            .get(target)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .expect("redirect response")
            .text()
            .await
            .expect("body");
        assert_eq!(body, "bypassed");
        assert!(
            direct_request
                .await
                .expect("direct request")
                .starts_with(b"GET /final HTTP/1.1")
        );
        proxy_task.await.expect("proxy task");
    }

    #[tokio::test]
    async fn captured_client_retains_revision_while_new_operations_adopt_changes() {
        let transport = OutboundProxyTransport::default();
        let client = client(&transport);
        let target = Url::parse("http://upstream.invalid/resource").expect("target");
        let (old_address, old_request) = response_server("old").await;
        transport.set_system_settings(Ok(SystemProxySettings {
            http: Some(Url::parse(&format!("http://{old_address}")).expect("old proxy")),
            ..SystemProxySettings::default()
        }));
        let captured = client.client_for(&target).expect("captured client");
        transport.set_system_settings(Ok(SystemProxySettings {
            automatic_enabled: true,
            ..SystemProxySettings::default()
        }));
        assert!(matches!(
            client.client_for(&target),
            Err(OutboundClientError::SystemProxy(
                SystemProxyError::AutomaticProxyUnsupported
            ))
        ));
        assert_eq!(
            captured
                .get(target.clone())
                .send()
                .await
                .expect("old snapshot request")
                .text()
                .await
                .expect("old body"),
            "old"
        );
        old_request.await.expect("old request");
        let (new_address, new_request) = response_server("new").await;
        transport.set_system_settings(Ok(SystemProxySettings {
            http: Some(Url::parse(&format!("http://{new_address}")).expect("new proxy")),
            automatic_enabled: true,
            ..SystemProxySettings::default()
        }));
        assert_eq!(
            client
                .client_for(&target)
                .expect("new client")
                .get(target)
                .send()
                .await
                .expect("new request")
                .text()
                .await
                .expect("new body"),
            "new"
        );
        new_request.await.expect("new request task");
    }

    #[tokio::test]
    async fn system_socks_routes_external_requests_without_local_dns() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("SOCKS fixture");
        let address = listener.local_addr().expect("SOCKS address");
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("SOCKS connection");
            let mut greeting = [0_u8; 2];
            socket
                .read_exact(&mut greeting)
                .await
                .expect("SOCKS greeting");
            assert_eq!(greeting[0], 5);
            let mut methods = vec![0_u8; usize::from(greeting[1])];
            socket
                .read_exact(&mut methods)
                .await
                .expect("SOCKS methods");
            socket.write_all(&[5, 0]).await.expect("SOCKS accepted");
            let mut command = [0_u8; 5];
            socket
                .read_exact(&mut command)
                .await
                .expect("SOCKS command");
            assert_eq!(&command[..4], &[5, 1, 0, 3]);
            let mut destination = vec![0_u8; usize::from(command[4]) + 2];
            socket
                .read_exact(&mut destination)
                .await
                .expect("SOCKS destination");
            assert_eq!(&destination[..destination.len() - 2], b"upstream.invalid");
            socket
                .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                .await
                .expect("SOCKS connected");
            let mut request = [0_u8; 4096];
            let read = socket.read(&mut request).await.expect("HTTP request");
            assert!(request[..read].starts_with(b"GET /asset HTTP/1.1"));
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nsocks",
                )
                .await
                .expect("HTTP response");
        });
        let transport = OutboundProxyTransport::default();
        transport.set_system_settings(Ok(SystemProxySettings {
            socks: Some(Url::parse(&format!("socks5h://{address}")).expect("SOCKS proxy")),
            automatic_enabled: true,
            ..SystemProxySettings::default()
        }));
        let target = Url::parse("http://upstream.invalid/asset").expect("target");
        let response = client(&transport)
            .client_for(&target)
            .expect("SOCKS client")
            .get(target)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .expect("SOCKS response");
        assert_eq!(response.text().await.expect("SOCKS body"), "socks");
        task.await.expect("SOCKS fixture task");
    }
}
