//! Hidden-`WebView` synchronization of the official pricing table.
//!
//! The manual "同步官网" action renders
//! <https://developers.openai.com/api/docs/pricing/> in a hidden, isolated
//! `WebView` that runs `fixtures/pricing-capture-script.js` at document start.
//! The script reads the rendered tables and reports one payload; Rust validates
//! it (`router_core::pricing_capture`), atomically replaces the local pricing
//! table, and hot-swaps the effective catalog.
//!
//! Handoff channel: the script reports by navigating to
//! `airouter-pricing-capture://v1/<base64url payload>`. `on_navigation` is a
//! generic policy callback — wry consults it for every navigation action,
//! including schemes the engine cannot load — so the unknown scheme is
//! intercepted, cancelled, and never reaches the network. The documented
//! fallback (polling `WebviewWindow::title()`) was rejected because it needs
//! two moving parts (a title budget plus a poll interval) for the same signal.
//!
//! Isolation: the window is created from Rust only, is invisible, unfocusable,
//! unlisted, uses an ephemeral data store, denies new windows and downloads,
//! and is deliberately covered by no capability, so the remote page receives no
//! command and no local data. Tauri still injects its IPC bootstrap
//! (`window.__TAURI_INTERNALS__`) into every webview, but the page origin is
//! remote and the window matches no capability, so the runtime ACL rejects every
//! command from it. Every navigation except the capture target itself is
//! cancelled.
//!
//! QA builds may point the capture at a loopback fixture with
//! `AI_ROUTER_QA_PRICING_URL`; an override that is not an `http(s)` loopback
//! page is ignored with an error and the production page is used instead.

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::time::Duration;

use router_core::{
    app_api::{PricingTableDto, PricingTableStatusDto},
    pricing::CatalogProvider,
    pricing_capture::{
        CAPTURE_SCHEME, CaptureError, CaptureHint, decode_capture_navigation, parse_capture_payload,
    },
    pricing_local::{LocalPricingLoad, LocalPricingStore},
    state::{AppRuntimeState, StateArea},
};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::{
    mpsc::{UnboundedSender, unbounded_channel},
    watch,
};
use url::Url;

/// Label of the hidden synchronization window; at most one may exist.
const SYNC_WINDOW_LABEL: &str = "pricing-sync";
/// Title of the hidden window, only ever seen in a platform window list.
const SYNC_WINDOW_TITLE: &str = "AI Router 定价同步";
/// Environment variable that points the capture at a QA loopback fixture.
const QA_PRICING_URL_ENV: &str = "AI_ROUTER_QA_PRICING_URL";
/// Total budget of one synchronization run, including the page load.
const SYNC_BUDGET: Duration = Duration::from_secs(20);
/// The extraction script, kept as pure DOM JavaScript so tests can run it.
const CAPTURE_SCRIPT: &str = include_str!("../../fixtures/pricing-capture-script.js");

/// Where one synchronization run renders the pricing page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureTarget {
    /// The fixed production pricing page.
    Production(Url),
    /// The loopback fixture an accepted QA override names.
    Override(Url),
    /// An override that must be ignored; the production page is used instead.
    RejectedOverride(Url),
}

impl CaptureTarget {
    /// The page a run must render.
    #[must_use]
    pub const fn url(&self) -> &Url {
        match self {
            Self::Production(url) | Self::Override(url) | Self::RejectedOverride(url) => url,
        }
    }
}

/// What the hidden window must do with one navigation request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NavigationDecision {
    /// The capture target itself, so the page may load.
    Allow,
    /// Cancel the navigation and capture the reported payload.
    Capture(Vec<u8>),
    /// Cancel the navigation and fail the run with a stable reason.
    Reject(CaptureError),
    /// Cancel the navigation: the page may not reach anything else.
    Cancel,
}

/// Resolves the page one capture run renders.
///
/// The production page is the default. A QA override replaces it only when it
/// parses as an `http(s)` loopback page, so a stray environment variable can
/// never make the hidden webview render an arbitrary remote page.
#[must_use]
pub fn resolve_capture_url(production: Url, override_url: Option<&str>) -> CaptureTarget {
    let Some(candidate) = override_url else {
        return CaptureTarget::Production(production);
    };
    match Url::parse(candidate) {
        Ok(url) if is_loopback_page(&url) => CaptureTarget::Override(url),
        Ok(_) | Err(_) => CaptureTarget::RejectedOverride(production),
    }
}

/// What one navigation evaluation asks the hidden window to do.
#[must_use]
pub fn navigation_decision(target: &Url, url: &Url) -> NavigationDecision {
    if url.scheme() == CAPTURE_SCHEME {
        return match decode_capture_navigation(url) {
            Ok(payload) => NavigationDecision::Capture(payload),
            Err(error) => NavigationDecision::Reject(error),
        };
    }
    if same_page(target, url) {
        NavigationDecision::Allow
    } else {
        NavigationDecision::Cancel
    }
}

/// Whether a URL may replace the production capture target in QA builds.
fn is_loopback_page(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// Whether a navigation stays on the exact page the run asked for.
fn same_page(target: &Url, url: &Url) -> bool {
    url.scheme() == target.scheme()
        && url.host() == target.host()
        && url.port_or_known_default() == target.port_or_known_default()
        && normalized_path(url) == normalized_path(target)
}

fn normalized_path(url: &Url) -> &str {
    url.path().strip_suffix('/').unwrap_or(url.path())
}

/// Transient state of the manual synchronization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum SyncStatus {
    Idle,
    Syncing,
    Error,
}

impl SyncStatus {
    fn load(status: &AtomicU8) -> Self {
        match status.load(Ordering::Acquire) {
            1 => Self::Syncing,
            2 => Self::Error,
            _ => Self::Idle,
        }
    }

    fn store(self, status: &AtomicU8) {
        status.store(self as u8, Ordering::Release);
    }

    const fn dto(self) -> PricingTableStatusDto {
        match self {
            Self::Idle => PricingTableStatusDto::Idle,
            Self::Syncing => PricingTableStatusDto::Syncing,
            Self::Error => PricingTableStatusDto::Error,
        }
    }
}

/// What the hidden window reported through the navigation policy.
enum CaptureSignal {
    Captured(Vec<u8>),
    Rejected(CaptureError),
}

/// One in-flight synchronization that concurrent callers may join.
struct SyncRun {
    result: watch::Sender<Option<PricingTableDto>>,
}

impl SyncRun {
    fn new() -> Self {
        let (result, _receiver) = watch::channel(None);
        Self { result }
    }

    /// Publishes the run's outcome so every caller that joined reads it.
    ///
    /// [`watch::Sender::send`] drops the value when the channel has no receiver
    /// yet, and a caller that took this run's handle just before the leader
    /// cleared the in-flight slot only subscribes afterwards; it would then wait
    /// for a write that never comes. `send_replace` keeps the value instead.
    fn finish(&self, snapshot: PricingTableDto) {
        self.result.send_replace(Some(snapshot));
    }

    /// Waits for the run to report; `None` when the run vanished first.
    async fn joined(&self) -> Option<PricingTableDto> {
        let mut receiver = self.result.subscribe();
        loop {
            let reported = receiver.borrow().as_ref().cloned();
            if let Some(snapshot) = reported {
                return Some(snapshot);
            }
            receiver.changed().await.ok()?;
        }
    }
}

/// Closes the hidden window on every exit path, including task cancellation.
struct CaptureWindowGuard(Option<WebviewWindow>);

impl Drop for CaptureWindowGuard {
    fn drop(&mut self) {
        if let Some(window) = self.0.take() {
            let _ = window.destroy();
        }
    }
}

/// Owns the single-flight manual synchronization of the local pricing table.
pub struct PricingSyncCoordinator {
    store: LocalPricingStore,
    pricing: CatalogProvider,
    runtime_state: Arc<AppRuntimeState>,
    status: AtomicU8,
    inflight: tokio::sync::Mutex<Option<Arc<SyncRun>>>,
    production: Url,
    override_allowed: bool,
}

impl PricingSyncCoordinator {
    /// Builds the coordinator around the installed local table and catalog.
    ///
    /// # Panics
    ///
    /// Panics when the embedded pricing source URL is not a valid URL.
    #[must_use]
    pub fn new(
        store: LocalPricingStore,
        pricing: CatalogProvider,
        runtime_state: Arc<AppRuntimeState>,
        override_allowed: bool,
    ) -> Self {
        Self {
            store,
            pricing,
            runtime_state,
            status: AtomicU8::new(SyncStatus::Idle as u8),
            inflight: tokio::sync::Mutex::new(None),
            production: Url::parse(crate::PRICING_SOURCE_URL)
                .expect("the embedded pricing source URL is valid"),
            override_allowed,
        }
    }

    /// Projects the read-only pricing table the settings view renders.
    #[must_use]
    pub fn snapshot(&self) -> PricingTableDto {
        let state = self.pricing.state();
        PricingTableDto {
            rows: state.catalog.rows().into_iter().map(Into::into).collect(),
            synced_at_ms: state.synced_at_ms,
            source_url: state.source_url.clone(),
            local_state: state.local.into(),
            status: SyncStatus::load(&self.status).dto(),
        }
    }

    /// Runs one manual synchronization, joining an in-flight run if there is one.
    ///
    /// Concurrent callers share a single hidden window and receive the snapshot
    /// that run produced, so a repeated click never starts two captures.
    pub async fn sync(&self, app: &AppHandle) -> PricingTableDto {
        self.join(self.capture(app)).await
    }

    /// Shares one capture between every caller that arrives while it runs.
    async fn join(&self, capture: impl Future<Output = PricingTableDto>) -> PricingTableDto {
        let existing = {
            let mut inflight = self.inflight.lock().await;
            if let Some(existing) = inflight.as_ref() {
                Arc::clone(existing)
            } else {
                let run = Arc::new(SyncRun::new());
                *inflight = Some(Arc::clone(&run));
                drop(inflight);
                return self.lead(capture, &run).await;
            }
        };
        // A joining caller never renders the page itself.
        drop(capture);
        existing.joined().await.unwrap_or_else(|| self.snapshot())
    }

    /// Runs the capture and always reports its outcome to the shared run.
    async fn lead(
        &self,
        capture: impl Future<Output = PricingTableDto>,
        run: &Arc<SyncRun>,
    ) -> PricingTableDto {
        let snapshot = capture.await;
        {
            let mut inflight = self.inflight.lock().await;
            if inflight
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, run))
            {
                *inflight = None;
            }
        }
        run.finish(snapshot.clone());
        snapshot
    }

    /// Parses one captured payload and installs it as the local pricing table.
    ///
    /// The table is written atomically before it becomes effective, so a crash
    /// never leaves a half-written override. `StateArea::PricingTable` is
    /// published after the replace so every open window refetches the new rows.
    ///
    /// # Errors
    ///
    /// Returns the contract error of a rejected payload, or
    /// [`CaptureError::Store`] when the local table could not be published; the
    /// previously installed table stays in effect either way.
    pub fn apply_capture(
        &self,
        payload: &[u8],
        synced_at_ms: i64,
        source_url: &str,
    ) -> Result<Vec<CaptureHint>, CaptureError> {
        let outcome = parse_capture_payload(payload, synced_at_ms, source_url)?;
        self.store.publish(&outcome.table).map_err(|error| {
            log::warn!(
                target: "ai_router::pricing",
                "code=pricing_sync_store_failed error={error}"
            );
            CaptureError::Store
        })?;
        self.pricing
            .install(LocalPricingLoad::Loaded(outcome.table));
        self.runtime_state
            .publish_background_change(vec![StateArea::PricingTable]);
        Ok(outcome.hints)
    }

    /// Renders the capture target and reports the resulting pricing table.
    async fn capture(&self, app: &AppHandle) -> PricingTableDto {
        let target = self.resolve_target();
        if matches!(target, CaptureTarget::RejectedOverride(_)) {
            log::error!(
                target: "ai_router::pricing",
                "code=pricing_sync_override_rejected"
            );
        }
        let url = target.url().clone();
        SyncStatus::Syncing.store(&self.status);
        let payload = match self.render(app, &url).await {
            Ok(payload) => payload,
            Err(error) => return self.report_failure(&url, &error),
        };
        match self.apply_capture(&payload, crate::runtime::now_millis(), url.as_str()) {
            Ok(hints) => {
                for hint in &hints {
                    log::warn!(
                        target: "ai_router::pricing",
                        "code=pricing_sync_ratio_hint {hint}"
                    );
                }
                SyncStatus::Idle.store(&self.status);
            }
            Err(error) => return self.report_failure(&url, &error),
        }
        self.snapshot()
    }

    /// Reports a failed run in the settings snapshot without losing the table.
    fn report_failure(&self, source: &Url, error: &CaptureError) -> PricingTableDto {
        log::warn!(
            target: "ai_router::pricing",
            "code=pricing_sync_failed error={error} source={source}"
        );
        SyncStatus::Error.store(&self.status);
        self.snapshot()
    }

    /// Resolves the page this run renders, honouring the QA loopback override.
    fn resolve_target(&self) -> CaptureTarget {
        let override_url = self
            .override_allowed
            .then(|| std::env::var(QA_PRICING_URL_ENV).ok())
            .flatten();
        resolve_capture_url(self.production.clone(), override_url.as_deref())
    }

    /// Renders one URL in the hidden window and waits for the script's report.
    async fn render(&self, app: &AppHandle, url: &Url) -> Result<Vec<u8>, CaptureError> {
        // A leaked window from an aborted run must never block the next one.
        if let Some(existing) = app.get_webview_window(SYNC_WINDOW_LABEL) {
            let _ = existing.destroy();
        }
        let (signals, mut receiver) = unbounded_channel();
        let window = build_capture_window(app, url, signals).map_err(|error| {
            log::warn!(
                target: "ai_router::pricing",
                "code=pricing_sync_window_failed error={error}"
            );
            CaptureError::Window
        })?;
        // The guard closes the window however this function ends.
        let _guard = CaptureWindowGuard(Some(window));
        match tokio::time::timeout(SYNC_BUDGET, receiver.recv()).await {
            Ok(Some(CaptureSignal::Captured(payload))) => Ok(payload),
            Ok(Some(CaptureSignal::Rejected(error))) => Err(error),
            Ok(None) => Err(CaptureError::Closed),
            Err(_) => Err(CaptureError::Timeout),
        }
    }
}

/// Builds the hidden, isolated window that renders the capture target.
fn build_capture_window(
    app: &AppHandle,
    target: &Url,
    signals: UnboundedSender<CaptureSignal>,
) -> tauri::Result<WebviewWindow> {
    let navigation_target = target.clone();
    WebviewWindowBuilder::new(app, SYNC_WINDOW_LABEL, WebviewUrl::External(target.clone()))
        .title(SYNC_WINDOW_TITLE)
        .visible(false)
        .focused(false)
        .resizable(false)
        .skip_taskbar(true)
        .incognito(true)
        .disable_drag_drop_handler()
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_download(|_, _| false)
        .initialization_script(CAPTURE_SCRIPT)
        .on_navigation(
            move |url| match navigation_decision(&navigation_target, url) {
                NavigationDecision::Allow => true,
                NavigationDecision::Capture(payload) => {
                    let _ = signals.send(CaptureSignal::Captured(payload));
                    false
                }
                NavigationDecision::Reject(error) => {
                    let _ = signals.send(CaptureSignal::Rejected(error));
                    false
                }
                NavigationDecision::Cancel => false,
            },
        )
        .build()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::sync::Mutex;

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use router_core::app_api::{
        PricingBandDto, PricingLocalStateDto, PricingRowSourceDto, PricingTableStatusDto,
    };
    use router_core::pricing::{CostStatus, UsageObservation};
    use router_core::state::{StateChangedEventDto, StateEventError, StateEventSink};
    use tempfile::TempDir;

    use super::*;

    const PRODUCTION: &str = crate::PRICING_SOURCE_URL;
    const SYNCED_AT_MS: i64 = 1_790_000_000_000;
    const PAGE: &str = include_str!("../../fixtures/pricing-capture-page.html");
    const PAYLOAD: &[u8] = include_bytes!("../../fixtures/pricing-capture-payload.json");

    #[derive(Default)]
    struct RecordingEventSink(Mutex<Vec<StateChangedEventDto>>);

    impl StateEventSink for RecordingEventSink {
        fn publish(&self, event: &StateChangedEventDto) -> Result<(), StateEventError> {
            self.0.lock().expect("event sink lock").push(event.clone());
            Ok(())
        }
    }

    struct Fixture {
        directory: TempDir,
        coordinator: PricingSyncCoordinator,
        pricing: CatalogProvider,
        events: Arc<RecordingEventSink>,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = TempDir::new().expect("app data fixture");
            let pricing = CatalogProvider::baseline();
            let events = Arc::new(RecordingEventSink::default());
            let coordinator = PricingSyncCoordinator::new(
                LocalPricingStore::new(directory.path().to_path_buf()),
                pricing.clone(),
                Arc::new(AppRuntimeState::new(events.clone())),
                true,
            );
            Self {
                directory,
                coordinator,
                pricing,
                events,
            }
        }

        fn store(&self) -> LocalPricingStore {
            LocalPricingStore::new(self.directory.path().to_path_buf())
        }

        fn published_areas(&self) -> Vec<StateArea> {
            self.events
                .0
                .lock()
                .expect("event sink lock")
                .iter()
                .flat_map(|event| event.areas.clone())
                .collect()
        }
    }

    fn production() -> Url {
        Url::parse(PRODUCTION).expect("the production pricing URL parses")
    }

    /// Serves the capture fixture over loopback, once.
    fn serve_fixture() -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port");
        let port = listener.local_addr().expect("the bound address").port();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming().take(1) {
                let Ok(mut stream) = stream else { return };
                // Drain the request so the peer never sees a reset.
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
                    PAGE.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
            }
        });
        (format!("http://127.0.0.1:{port}/api/docs/pricing/"), handle)
    }

    /// Reads one loopback page with a minimal HTTP/1.1 request.
    fn fetch(url: &Url) -> String {
        let host = url.host_str().expect("the fixture URL has a host");
        let port = url.port().expect("the fixture URL has a port");
        let mut stream = TcpStream::connect((host, port)).expect("the fixture server accepts");
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n",
            url.path()
        );
        stream
            .write_all(request.as_bytes())
            .expect("the request is sent");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("the response is read");
        response
            .split_once("\r\n\r\n")
            .expect("the response has a body")
            .1
            .to_owned()
    }

    #[test]
    fn qa_override_is_accepted_only_for_http_loopback_pages() {
        for candidate in [
            "http://127.0.0.1:8080/api/docs/pricing/",
            "https://localhost:8443/api/docs/pricing/",
            "http://[::1]:9000/pricing",
        ] {
            let target = resolve_capture_url(production(), Some(candidate));
            assert!(
                matches!(target, CaptureTarget::Override(_)),
                "{candidate} must be accepted: {target:?}"
            );
        }

        for candidate in [
            PRODUCTION,
            "http://192.168.0.10/pricing",
            "http://127.0.0.1.evil.test/pricing",
            "file:///tmp/pricing.html",
            "javascript:alert(1)",
            "not a url",
        ] {
            match resolve_capture_url(production(), Some(candidate)) {
                CaptureTarget::RejectedOverride(url) => {
                    assert_eq!(url.as_str(), PRODUCTION, "{candidate} fell back");
                }
                other => panic!("{candidate} must be ignored: {other:?}"),
            }
        }

        assert_eq!(
            resolve_capture_url(production(), None),
            CaptureTarget::Production(production())
        );
    }

    #[test]
    fn navigation_policy_allows_only_the_capture_target_and_the_capture_scheme() {
        let target = production();
        for allowed in [
            "https://developers.openai.com/api/docs/pricing/",
            "https://developers.openai.com/api/docs/pricing",
            "https://developers.openai.com/api/docs/pricing/?lang=en",
            "https://developers.openai.com/api/docs/pricing/#models",
        ] {
            let url = Url::parse(allowed).expect("the same page parses");
            assert_eq!(
                navigation_decision(&target, &url),
                NavigationDecision::Allow
            );
        }
        for cancelled in [
            "https://developers.openai.com/api/docs/guides/prompt-caching/",
            "https://example.com/",
            "http://developers.openai.com/api/docs/pricing/",
            "data:text/html,<h1>challenge</h1>",
            "airouter-pricing-capture://v2/AAAA",
        ] {
            let url = Url::parse(cancelled).expect("the foreign URL parses");
            let decision = navigation_decision(&target, &url);
            if cancelled.starts_with("airouter-pricing-capture") {
                assert_eq!(
                    decision,
                    NavigationDecision::Reject(CaptureError::NotCapture)
                );
            } else {
                assert_eq!(decision, NavigationDecision::Cancel, "{cancelled}");
            }
        }

        let encoded = URL_SAFE_NO_PAD.encode(b"{\"ok\":false}");
        let link = Url::parse(&format!("airouter-pricing-capture://v1/{encoded}"))
            .expect("the capture link parses");
        assert_eq!(
            navigation_decision(&target, &link),
            NavigationDecision::Capture(b"{\"ok\":false}".to_vec())
        );

        let oversized = Url::parse(&format!(
            "airouter-pricing-capture://v1/{}",
            "A".repeat(router_core::pricing_capture::MAX_CAPTURE_PAYLOAD_BYTES + 1)
        ))
        .expect("the oversized link parses");
        assert_eq!(
            navigation_decision(&target, &oversized),
            NavigationDecision::Reject(CaptureError::TooLarge)
        );
    }

    #[test]
    fn the_loopback_fixture_synchronizes_the_local_table_end_to_end() {
        let fixture = Fixture::new();
        let (base, server) = serve_fixture();
        let target = resolve_capture_url(production(), Some(&base));
        let CaptureTarget::Override(url) = target else {
            panic!("a loopback override must be accepted");
        };
        assert_eq!(url.as_str(), base);

        // The fixture really is the page the extraction script reads: it renders
        // the segmented controls, keeps the unselected panes hidden, and names
        // every model the reported payload bills.
        let page = fetch(&url);
        for marker in [
            "class=\"content-switcher-root\"",
            "role=\"radio\"",
            ">Fast mode<",
            "hidden",
        ] {
            assert!(page.contains(marker), "the fixture page renders {marker}");
        }
        let reported: serde_json::Value =
            serde_json::from_slice(PAYLOAD).expect("the capture payload parses");
        for tier in ["standard", "priority"] {
            for model in reported[tier].as_array().expect("the tier is an array") {
                let id = model["id"].as_str().expect("the model id is a string");
                assert!(page.contains(id), "the fixture page renders {id}");
            }
        }

        let hints = fixture
            .coordinator
            .apply_capture(PAYLOAD, SYNCED_AT_MS, url.as_str())
            .expect("the captured payload applies");
        assert!(hints.is_empty(), "{hints:?}");
        assert!(matches!(
            fixture.store().load(),
            LocalPricingLoad::Loaded(_)
        ));

        let snapshot = fixture.coordinator.snapshot();
        assert_eq!(snapshot.status, PricingTableStatusDto::Idle);
        assert_eq!(snapshot.local_state, PricingLocalStateDto::Loaded);
        assert_eq!(snapshot.synced_at_ms, Some(SYNCED_AT_MS));
        assert_eq!(snapshot.source_url.as_deref(), Some(url.as_str()));
        // A family the page bills under another id keeps pricing with the
        // bundled baseline: the captured `chat-latest` row never reaches the
        // local table.
        assert!(
            snapshot
                .rows
                .iter()
                .all(|row| row.model_id != "chat-latest"),
            "only GPT models synchronize; other families stay bundled"
        );
        let official: Vec<_> = snapshot
            .rows
            .iter()
            .filter(|row| row.source == PricingRowSourceDto::Official)
            .collect();
        assert_eq!(official.len(), 8);
        assert!(official.iter().all(|row| row.model_id.starts_with("gpt-")));
        assert!(official.iter().any(|row| {
            row.model_id == "gpt-6-astra"
                && row.band == PricingBandDto::Short
                && row.input_micro_usd == 10_000_000
        }));
        let split = snapshot
            .rows
            .iter()
            .position(|row| row.source == PricingRowSourceDto::Bundled)
            .expect("the bundled models stay available");
        assert!(
            snapshot.rows[..split]
                .iter()
                .all(|row| row.source == PricingRowSourceDto::Official)
                && snapshot.rows[split..]
                    .iter()
                    .all(|row| row.source == PricingRowSourceDto::Bundled),
            "synchronized rows come first, the bundled baseline follows"
        );

        // New requests are priced with the synchronized catalog.
        let priced = fixture.pricing.price(&UsageObservation {
            requested_model: Some("gpt-6-astra"),
            actual_model: Some("gpt-6-astra"),
            forwarded_service_tier: None,
            actual_service_tier: None,
            input_tokens: Some(272_000),
            output_tokens: Some(1_000),
            total_tokens: Some(273_000),
            cached_input_tokens: Some(0),
            cache_write_input_tokens: Some(0),
            possible_model_work: true,
        });
        assert_eq!(priced.status, CostStatus::Exact);
        assert_eq!(
            priced.catalog_version.as_deref(),
            Some("openai-standard-synced-2026-09-21")
        );
        assert_eq!(priced.amount_pico_usd, Some(2_770_000_000_000));

        assert_eq!(fixture.published_areas(), vec![StateArea::PricingTable]);
        server.join().expect("the fixture server stops");
    }

    #[test]
    fn a_rejected_capture_keeps_the_previous_table_in_effect() {
        let fixture = Fixture::new();
        fixture
            .coordinator
            .apply_capture(PAYLOAD, SYNCED_AT_MS, PRODUCTION)
            .expect("the first capture applies");

        let mut broken: serde_json::Value =
            serde_json::from_slice(PAYLOAD).expect("the capture payload parses");
        broken["thresholds"] = serde_json::json!(["300K"]);
        let broken = serde_json::to_vec(&broken).expect("the rejected payload still serializes");
        assert_eq!(
            fixture
                .coordinator
                .apply_capture(&broken, SYNCED_AT_MS + 1, PRODUCTION),
            Err(CaptureError::Threshold)
        );

        let snapshot = fixture.coordinator.snapshot();
        assert_eq!(snapshot.local_state, PricingLocalStateDto::Loaded);
        assert_eq!(
            snapshot.synced_at_ms,
            Some(SYNCED_AT_MS),
            "the first capture stays effective"
        );
        assert!(snapshot.rows.iter().any(|row| {
            row.source == PricingRowSourceDto::Official && row.input_micro_usd == 10_000_000
        }));
        assert_eq!(
            fixture.published_areas(),
            vec![StateArea::PricingTable],
            "a rejected capture publishes nothing"
        );
        match fixture.store().load() {
            LocalPricingLoad::Loaded(table) => {
                assert_eq!(table.synced_at_ms, SYNCED_AT_MS);
            }
            other => panic!("expected the first capture on disk, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn concurrent_runs_share_one_capture_and_one_snapshot() {
        let fixture = Fixture::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let leader = async {
            entered.notify_one();
            release.notified().await;
            fixture
                .coordinator
                .apply_capture(PAYLOAD, SYNCED_AT_MS, PRODUCTION)
                .expect("the shared capture applies");
            fixture.coordinator.snapshot()
        };
        let releaser = tokio::spawn({
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            async move {
                entered.notified().await;
                release.notify_waiters();
            }
        });

        let (first, second) = tokio::join!(
            fixture.coordinator.join(leader),
            fixture
                .coordinator
                .join(async { panic!("a joined caller must not capture again") }),
        );

        assert_eq!(first, second, "both callers share the run's snapshot");
        assert_eq!(first.status, PricingTableStatusDto::Idle);
        assert_eq!(first.local_state, PricingLocalStateDto::Loaded);
        assert_eq!(first.synced_at_ms, Some(SYNCED_AT_MS));
        assert_eq!(
            fixture.published_areas(),
            vec![StateArea::PricingTable],
            "the shared capture publishes once"
        );
        releaser.await.expect("the releaser stops");

        // The next caller starts a fresh run instead of joining the finished one.
        let third = fixture
            .coordinator
            .join(async { fixture.coordinator.snapshot() })
            .await;
        assert_eq!(third, first);
        assert_eq!(fixture.published_areas(), vec![StateArea::PricingTable]);
    }

    #[tokio::test]
    async fn a_caller_that_joins_as_the_leader_finishes_still_reads_the_snapshot() {
        // The leader clears the in-flight slot before it publishes the outcome,
        // so a joining caller can take the run handle first and subscribe only
        // afterwards. It must read the stored snapshot instead of waiting for a
        // second write that never comes.
        let fixture = Fixture::new();
        let expected = fixture.coordinator.snapshot();
        let run = SyncRun::new();

        run.finish(expected.clone());

        let joined = tokio::time::timeout(std::time::Duration::from_secs(5), run.joined())
            .await
            .expect("a late subscriber must not wait for another write");
        assert_eq!(joined, Some(expected.clone()));
        assert_eq!(run.joined().await, Some(expected));
    }
}
