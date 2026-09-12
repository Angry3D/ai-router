use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use reqwest::{
    ClientBuilder, StatusCode,
    dns::{Name, Resolve, Resolving},
    header::{self, HeaderMap, HeaderName, HeaderValue},
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{self, ServerConfig, pki_types::PrivatePkcs8KeyDer},
    server::TlsStream,
};
use url::Host;

use super::{AdmittedUrl, DownloadLimits, ImageAssetDownloader};

pub(super) const PUBLIC_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
const ASSET_URL: &str = "https://assets.example/image.png";

// Generated for each fixture in memory. Neither test private keys nor a TLS
// verification bypass are embedded in the source or the production downloader.
pub(in crate::proxy) struct AssetFixture {
    network: Arc<TestNetwork>,
    server: JoinHandle<()>,
}

impl AssetFixture {
    pub(in crate::proxy) async fn new(replies: Vec<AssetReply>) -> Self {
        let rcgen::CertifiedKey { cert, key_pair } = rcgen::generate_simple_self_signed(vec![
            "assets.example".to_owned(),
            "cdn.example".to_owned(),
            "xn--bcher-kva.example".to_owned(),
        ])
        .expect("generate synthetic TLS certificate");
        let root = reqwest::Certificate::from_der(cert.der().as_ref())
            .expect("load synthetic TLS certificate");
        let server_config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("TLS versions")
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.der().clone()],
                    PrivatePkcs8KeyDer::from(key_pair.serialize_der()).into(),
                )
                .expect("TLS server certificate");
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local TLS fixture");
        let local = listener.local_addr().expect("local fixture address");
        let state = Arc::new(FixtureState {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
            request_seen: Notify::new(),
            connections: AtomicUsize::new(0),
            closed_connections: AtomicUsize::new(0),
            server_names: Mutex::new(Vec::new()),
            handshake_delay: Mutex::new(Duration::ZERO),
        });
        let network = Arc::new(TestNetwork {
            local,
            root,
            state: Arc::clone(&state),
            answers: Mutex::new(BTreeMap::new()),
            dns_queries: Mutex::new(Vec::new()),
            pinned: Mutex::new(Vec::new()),
            unexpected_dns: Arc::new(AtomicUsize::new(0)),
        });
        let server = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        state.connections.fetch_add(1, Ordering::SeqCst);
                        connections.spawn(serve_connection(
                            socket,
                            acceptor.clone(),
                            Arc::clone(&state),
                        ));
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Self { network, server }
    }

    pub(in crate::proxy) fn url() -> String {
        ASSET_URL.to_owned()
    }

    pub(in crate::proxy) fn downloader(&self) -> ImageAssetDownloader {
        ImageAssetDownloader {
            limits: DownloadLimits::default(),
            network: Some(Arc::clone(&self.network)),
        }
    }

    pub(in crate::proxy) fn request_count(&self) -> usize {
        self.network
            .state
            .requests
            .lock()
            .expect("fixture requests")
            .len()
    }

    pub(in crate::proxy) fn requests(&self) -> Vec<CapturedAssetRequest> {
        self.network
            .state
            .requests
            .lock()
            .expect("fixture requests")
            .clone()
    }

    pub(in crate::proxy) async fn wait_for_requests(&self, count: usize) {
        timeout(Duration::from_secs(5), async {
            loop {
                let notified = self.network.state.request_seen.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.request_count() >= count {
                    return;
                }
                notified.await;
            }
        })
        .await
        .expect("fixture request deadline");
    }

    pub(super) fn set_dns_answers(&self, host: &str, answers: Vec<DnsAnswer>) {
        self.network
            .answers
            .lock()
            .expect("DNS answers")
            .insert(host.to_owned(), answers.into());
    }

    pub(super) fn dns_queries(&self) -> Vec<String> {
        self.network
            .dns_queries
            .lock()
            .expect("DNS queries")
            .clone()
    }

    pub(super) fn pinned_addresses(&self) -> Vec<Vec<SocketAddr>> {
        self.network
            .pinned
            .lock()
            .expect("pinned addresses")
            .clone()
    }

    pub(super) fn unexpected_dns_queries(&self) -> usize {
        self.network.unexpected_dns.load(Ordering::SeqCst)
    }

    pub(super) fn connection_count(&self) -> usize {
        self.network.state.connections.load(Ordering::SeqCst)
    }

    pub(super) fn closed_connection_count(&self) -> usize {
        self.network.state.closed_connections.load(Ordering::SeqCst)
    }

    pub(super) fn server_names(&self) -> Vec<String> {
        self.network
            .state
            .server_names
            .lock()
            .expect("TLS server names")
            .clone()
    }

    pub(super) fn delay_handshake(&self, delay: Duration) {
        *self
            .network
            .state
            .handshake_delay
            .lock()
            .expect("handshake delay") = delay;
    }
}

impl Drop for AssetFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

// Deliberately no Debug: captured paths can contain synthetic signed queries.
#[derive(Clone)]
pub(in crate::proxy) struct CapturedAssetRequest {
    pub(in crate::proxy) method: String,
    pub(in crate::proxy) path: String,
    pub(in crate::proxy) headers: HeaderMap,
}

pub(in crate::proxy) struct AssetReply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
    header_delay: Duration,
    chunk_bytes: Option<usize>,
    chunk_delay: Duration,
    declared_length: Option<usize>,
    disconnect_before_headers: bool,
    gate: Option<Arc<Notify>>,
}

impl AssetReply {
    pub(in crate::proxy) fn ok(body: Vec<u8>) -> Self {
        Self::status(StatusCode::OK, body)
    }

    pub(in crate::proxy) fn status(status: StatusCode, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body,
            header_delay: Duration::ZERO,
            chunk_bytes: None,
            chunk_delay: Duration::ZERO,
            declared_length: None,
            disconnect_before_headers: false,
            gate: None,
        }
    }

    pub(in crate::proxy) fn redirect(location: &str) -> Self {
        Self::status(StatusCode::FOUND, Vec::new()).header(header::LOCATION, location)
    }

    pub(in crate::proxy) fn gated(body: Vec<u8>, gate: Arc<Notify>) -> Self {
        let mut reply = Self::ok(body);
        reply.gate = Some(gate);
        reply
    }

    pub(super) fn header(mut self, name: HeaderName, value: &str) -> Self {
        self.headers
            .insert(name, HeaderValue::from_str(value).expect("fixture header"));
        self
    }

    pub(super) fn delayed_headers(mut self, delay: Duration) -> Self {
        self.header_delay = delay;
        self
    }

    pub(super) fn chunked(mut self, chunk_bytes: usize, delay: Duration) -> Self {
        assert!(chunk_bytes > 0);
        self.chunk_bytes = Some(chunk_bytes);
        self.chunk_delay = delay;
        self
    }

    pub(super) fn declared_length(mut self, length: usize) -> Self {
        self.declared_length = Some(length);
        self
    }

    pub(super) fn disconnect() -> Self {
        let mut reply = Self::ok(Vec::new());
        reply.disconnect_before_headers = true;
        reply
    }
}

#[derive(Clone)]
pub(super) struct DnsAnswer {
    pub(super) addresses: Vec<IpAddr>,
    pub(super) delay: Duration,
    pub(super) fails: bool,
}

impl DnsAnswer {
    pub(super) fn addresses(addresses: Vec<IpAddr>) -> Self {
        Self {
            addresses,
            delay: Duration::ZERO,
            fails: false,
        }
    }
}

struct FixtureState {
    replies: Mutex<VecDeque<AssetReply>>,
    requests: Mutex<Vec<CapturedAssetRequest>>,
    request_seen: Notify,
    connections: AtomicUsize,
    closed_connections: AtomicUsize,
    server_names: Mutex<Vec<String>>,
    handshake_delay: Mutex<Duration>,
}

pub(super) struct TestNetwork {
    local: SocketAddr,
    root: reqwest::Certificate,
    state: Arc<FixtureState>,
    answers: Mutex<BTreeMap<String, VecDeque<DnsAnswer>>>,
    dns_queries: Mutex<Vec<String>>,
    pinned: Mutex<Vec<Vec<SocketAddr>>>,
    unexpected_dns: Arc<AtomicUsize>,
}

impl TestNetwork {
    pub(super) async fn resolve(&self, domain: &str) -> Result<Vec<SocketAddr>, ()> {
        self.dns_queries
            .lock()
            .expect("DNS queries")
            .push(domain.to_owned());
        let answer = {
            let mut answers = self.answers.lock().expect("DNS answers");
            answers.get_mut(domain).map_or_else(
                || DnsAnswer::addresses(vec![PUBLIC_ADDRESS]),
                |answers| {
                    if answers.len() > 1 {
                        answers.pop_front().expect("queued DNS answer")
                    } else {
                        answers.front().expect("configured DNS answer").clone()
                    }
                },
            )
        };
        sleep(answer.delay).await;
        if answer.fails {
            Err(())
        } else {
            Ok(answer
                .addresses
                .into_iter()
                .map(|address| SocketAddr::new(address, 443))
                .collect())
        }
    }

    pub(super) fn connect_addresses(
        &self,
        target: &AdmittedUrl,
        addresses: &[SocketAddr],
    ) -> Vec<SocketAddr> {
        assert!(matches!(target.0.host(), Some(Host::Domain(_))));
        self.pinned
            .lock()
            .expect("pinned addresses")
            .push(addresses.to_vec());
        vec![self.local]
    }

    pub(super) fn configure_client(&self, builder: ClientBuilder) -> ClientBuilder {
        builder
            .tls_certs_only([self.root.clone()])
            .dns_resolver(Arc::new(RejectUnpinnedDns(Arc::clone(
                &self.unexpected_dns,
            ))))
    }
}

struct RejectUnpinnedDns(Arc<AtomicUsize>);

impl Resolve for RejectUnpinnedDns {
    fn resolve(&self, _name: Name) -> Resolving {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(io::Error::other("unpinned fixture DNS lookup").into()) })
    }
}

struct ConnectionGuard(Arc<FixtureState>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.closed_connections.fetch_add(1, Ordering::SeqCst);
    }
}

async fn serve_connection(socket: TcpStream, acceptor: TlsAcceptor, state: Arc<FixtureState>) {
    let _guard = ConnectionGuard(Arc::clone(&state));
    let delay = *state.handshake_delay.lock().expect("handshake delay");
    sleep(delay).await;
    let Ok(Ok(mut stream)) = timeout(Duration::from_secs(5), acceptor.accept(socket)).await else {
        return;
    };
    if let Some(server_name) = stream.get_ref().1.server_name() {
        state
            .server_names
            .lock()
            .expect("TLS server names")
            .push(server_name.to_owned());
    }
    let Ok(Ok(request)) = timeout(Duration::from_secs(5), read_request(&mut stream)).await else {
        return;
    };
    state
        .requests
        .lock()
        .expect("fixture requests")
        .push(request);
    state.request_seen.notify_waiters();
    let reply = state
        .replies
        .lock()
        .expect("fixture replies")
        .pop_front()
        .unwrap_or_else(|| AssetReply::status(StatusCode::SERVICE_UNAVAILABLE, Vec::new()));
    let _ = write_reply(&mut stream, reply).await;
}

async fn read_request(stream: &mut TlsStream<TcpStream>) -> io::Result<CapturedAssetRequest> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await?;
        if read == 0 || bytes.len().saturating_add(read) > 64 * 1024 {
            return Err(io::Error::other("invalid fixture request"));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_owned();
    let path = request_line.next().unwrap_or_default().to_owned();
    let mut headers = HeaderMap::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("invalid fixture header"))?;
        headers.append(
            HeaderName::from_bytes(name.as_bytes()).map_err(io::Error::other)?,
            HeaderValue::from_str(value.trim()).map_err(io::Error::other)?,
        );
    }
    Ok(CapturedAssetRequest {
        method,
        path,
        headers,
    })
}

async fn write_reply(stream: &mut TlsStream<TcpStream>, reply: AssetReply) -> io::Result<()> {
    if reply.disconnect_before_headers {
        return Ok(());
    }
    sleep(reply.header_delay).await;
    let mut head = format!(
        "HTTP/1.1 {} Fixture\r\nConnection: close\r\n",
        reply.status.as_u16()
    );
    for (name, value) in &reply.headers {
        head.push_str(name.as_str());
        head.push_str(": ");
        head.push_str(value.to_str().expect("fixture response header"));
        head.push_str("\r\n");
    }
    if reply.chunk_bytes.is_some() {
        head.push_str("Transfer-Encoding: chunked\r\n");
    } else {
        write!(
            head,
            "Content-Length: {}\r\n",
            reply.declared_length.unwrap_or(reply.body.len())
        )
        .expect("fixture response length");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await?;
    if let Some(gate) = reply.gate {
        let mut probe = [0_u8; 1];
        tokio::select! {
            () = gate.notified() => {}
            _ = stream.read(&mut probe) => return Ok(()),
        }
    }
    if let Some(chunk_bytes) = reply.chunk_bytes {
        for chunk in reply.body.chunks(chunk_bytes) {
            sleep(reply.chunk_delay).await;
            stream
                .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
                .await?;
            stream.write_all(chunk).await?;
            stream.write_all(b"\r\n").await?;
            stream.flush().await?;
        }
        stream.write_all(b"0\r\n\r\n").await?;
    } else {
        stream.write_all(&reply.body).await?;
    }
    stream.shutdown().await
}
