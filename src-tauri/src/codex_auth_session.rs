//! Hidden-`WebView` fetch of the `ChatGPT` session for Codex credential export.
//!
//! Plain HTTP clients are Cloudflare-blocked, so the session body must be
//! requested by a browser engine. This module reads the browser safe-storage
//! key from the macOS Keychain, injects the two decrypted session cookies into
//! an ephemeral, invisible `WebView`, loads the session endpoint, and hands the
//! JSON body back to the domain layer.
//!
//! Isolation mirrors `pricing_sync.rs`: the window is created from Rust only, is
//! invisible, unfocusable, unlisted, uses an incognito data store, denies new
//! windows and downloads, and matches no capability, so the remote page receives
//! no command and no local data. Injected cookies are deleted when the run ends.
//!
//! Handoff channel: the initialization script reports the fetched body by
//! navigating to `airouter-codex-auth://body/<base64url payload>`. The unknown
//! scheme is intercepted by `on_navigation`, cancelled, and never reaches the
//! network.
//!
//! QA builds may point the fetch at a loopback fixture with
//! `AI_ROUTER_QA_CODEX_AUTH_URL`; an override that is not an `http(s)` loopback
//! page is ignored with an error and the production endpoint is used instead.

use std::time::Duration;

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use router_core::codex_auth::{CodexAuthError, SessionCookie};
use tauri::{
    AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    webview::{Cookie, cookie::SameSite},
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use url::Url;
use zeroize::Zeroizing;

/// Label of the hidden session window; at most one may exist.
const SESSION_WINDOW_LABEL: &str = "codex-auth-session";
/// Title of the hidden window, only ever seen in a platform window list.
const SESSION_WINDOW_TITLE: &str = "AI Router Codex 凭证";
/// Environment variable that points the fetch at a QA loopback fixture.
const QA_SESSION_URL_ENV: &str = "AI_ROUTER_QA_CODEX_AUTH_URL";
/// The production session endpoint.
const PRODUCTION_SESSION_URL: &str = "https://chatgpt.com/api/auth/session";
/// Handoff scheme the initialization script navigates to.
const HANDOFF_SCHEME: &str = "airouter-codex-auth";
/// Handoff host that carries the body.
const HANDOFF_HOST: &str = "body";
/// Total budget of one session fetch, including the page load.
const SESSION_BUDGET: Duration = Duration::from_secs(25);
/// Maximum accepted session body; generous for the session JSON and small
/// enough that the handoff URL stays far below any platform limit.
const MAX_SESSION_BODY_BYTES: usize = 256 * 1024;

/// Where one session fetch loads its endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionTarget {
    /// The production endpoint.
    Production(Url),
    /// A QA loopback fixture that replaced the production endpoint.
    Override(Url),
    /// An override was provided but rejected; the production endpoint is used.
    RejectedOverride(Url),
}

impl SessionTarget {
    /// The endpoint this run loads.
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
    /// Keep loading the session endpoint.
    Allow,
    /// Intercept the handoff and report the decoded body.
    Capture(String),
    /// Cancel any other navigation.
    Cancel,
}

/// Resolves the endpoint one fetch run loads.
///
/// The production endpoint is the default. A QA override replaces it only when
/// it parses as an `http(s)` loopback page, so a stray environment variable can
/// never make the hidden webview render an arbitrary remote page.
#[must_use]
pub fn resolve_session_url(production: Url, override_url: Option<&str>) -> SessionTarget {
    let Some(candidate) = override_url.and_then(|value| Url::parse(value).ok()) else {
        return SessionTarget::Production(production);
    };
    if is_loopback_page(&candidate) {
        SessionTarget::Override(candidate)
    } else {
        SessionTarget::RejectedOverride(production)
    }
}

/// Whether a URL may replace the production endpoint in QA builds.
fn is_loopback_page(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// What one navigation evaluation asks the hidden window to do.
#[must_use]
pub fn navigation_decision(target: &Url, url: &Url) -> NavigationDecision {
    if url.scheme() == HANDOFF_SCHEME {
        return match decode_handoff(url) {
            Some(body) => NavigationDecision::Capture(body),
            None => NavigationDecision::Cancel,
        };
    }
    if same_page(target, url) {
        NavigationDecision::Allow
    } else {
        NavigationDecision::Cancel
    }
}

fn decode_handoff(url: &Url) -> Option<String> {
    if url.host_str() != Some(HANDOFF_HOST) {
        return None;
    }
    let payload = url.path().strip_prefix('/')?;
    if payload.is_empty() {
        return None;
    }
    // The script emits unpadded base64url. Standard base64 — what a naive
    // `btoa` produces, `=` padding included — is accepted too, so a page-side
    // encoding change can never silently discard a fetched session.
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| STANDARD.decode(payload))
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// Rejects a reported body beyond the accepted bound.
///
/// The handoff travels through a navigation URL, so an oversized body fails the
/// run closed instead of being materialised into a path the platform may refuse.
fn accept_session_body(body: Zeroizing<String>) -> Result<Zeroizing<String>, CodexAuthError> {
    if body.len() > MAX_SESSION_BODY_BYTES {
        return Err(CodexAuthError::SessionFetchFailed);
    }
    Ok(body)
}

/// Whether a navigation stays on the exact endpoint the run asked for.
fn same_page(target: &Url, url: &Url) -> bool {
    url.scheme() == target.scheme()
        && url.host_str() == target.host_str()
        && url.port_or_known_default() == target.port_or_known_default()
        && normalized_path(url) == normalized_path(target)
}

fn normalized_path(url: &Url) -> &str {
    url.path().strip_suffix('/').unwrap_or(url.path())
}

/// One run's outcome reported through the navigation policy.
enum SessionSignal {
    Captured(Zeroizing<String>),
    Failed,
}

/// Closes the hidden window on every exit path, including task cancellation.
struct SessionWindowGuard(Option<WebviewWindow>);

impl Drop for SessionWindowGuard {
    fn drop(&mut self) {
        if let Some(window) = self.0.take() {
            let _ = window.destroy();
        }
    }
}

/// Fetches the `ChatGPT` session body through hidden browser engines.
pub struct CodexAuthSessionFetcher {
    production: Url,
    override_allowed: bool,
}

impl CodexAuthSessionFetcher {
    /// Builds the fetcher; the QA override is honoured only when allowed.
    ///
    /// # Panics
    ///
    /// Panics when the embedded production endpoint is not a valid URL.
    #[must_use]
    pub fn new(override_allowed: bool) -> Self {
        Self {
            production: Url::parse(PRODUCTION_SESSION_URL)
                .expect("the embedded session endpoint URL is valid"),
            override_allowed,
        }
    }

    /// Loads the session endpoint with the injected cookies and returns the body.
    ///
    /// # Errors
    ///
    /// Returns [`CodexAuthError::SessionFetchFailed`] when the hidden window
    /// cannot be created, the navigation fails, or the run times out; cookies
    /// are always deleted and the window always closed.
    pub async fn fetch(
        &self,
        app: &AppHandle,
        cookies: &[SessionCookie],
    ) -> Result<Zeroizing<String>, CodexAuthError> {
        let target = self.resolve_target();
        let url = target.url().clone();
        // A leaked window from an aborted run must never block the next one.
        if let Some(existing) = app.get_webview_window(SESSION_WINDOW_LABEL) {
            let _ = existing.destroy();
        }
        let (signals, mut receiver) = unbounded_channel();
        let window = build_session_window(app, &url, signals)
            .map_err(|_| CodexAuthError::SessionFetchFailed)?;
        let _guard = SessionWindowGuard(Some(window.clone()));
        if inject_cookies(&window, cookies).is_err() {
            return Err(CodexAuthError::SessionFetchFailed);
        }
        if window.navigate(url).is_err() {
            return Err(CodexAuthError::SessionFetchFailed);
        }
        let result = match tokio::time::timeout(SESSION_BUDGET, receiver.recv()).await {
            Ok(Some(SessionSignal::Captured(body))) => accept_session_body(body),
            Ok(Some(SessionSignal::Failed) | None) | Err(_) => {
                Err(CodexAuthError::SessionFetchFailed)
            }
        };
        clear_cookies(&window, cookies);
        result
    }

    /// Resolves the endpoint this run loads, honouring the QA loopback override.
    fn resolve_target(&self) -> SessionTarget {
        let override_url = self
            .override_allowed
            .then(|| std::env::var(QA_SESSION_URL_ENV).ok())
            .flatten();
        resolve_session_url(self.production.clone(), override_url.as_deref())
    }
}

fn build_session_window(
    app: &AppHandle,
    target: &Url,
    signals: UnboundedSender<SessionSignal>,
) -> tauri::Result<WebviewWindow> {
    let navigation_target = target.clone();
    let script = session_script(target);
    WebviewWindowBuilder::new(
        app,
        SESSION_WINDOW_LABEL,
        WebviewUrl::External(blank_page()),
    )
    .title(SESSION_WINDOW_TITLE)
    .visible(false)
    .focused(false)
    .resizable(false)
    .skip_taskbar(true)
    .incognito(true)
    .disable_drag_drop_handler()
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .on_download(|_, _| false)
    .initialization_script(script)
    .on_navigation(
        move |url| match navigation_decision(&navigation_target, url) {
            NavigationDecision::Allow => true,
            NavigationDecision::Capture(body) => {
                let _ = signals.send(SessionSignal::Captured(Zeroizing::new(body)));
                false
            }
            NavigationDecision::Cancel => {
                if url.scheme() == HANDOFF_SCHEME {
                    let _ = signals.send(SessionSignal::Failed);
                }
                false
            }
        },
    )
    .build()
}

fn blank_page() -> Url {
    Url::parse("about:blank").expect("about:blank is a valid URL")
}

fn session_script(target: &Url) -> String {
    let endpoint = serde_json::to_string(target.as_str()).unwrap_or_else(|_| "\"\"".to_owned());
    format!(
        r#"(function () {{
  var endpoint = {endpoint};
  var done = false;
  function report(text) {{
    if (done) {{ return; }}
    done = true;
    // An oversized body becomes an empty report: the handoff URL stays small
    // and Rust fails the run closed instead of building a huge path.
    if (text.length > {MAX_SESSION_BODY_BYTES}) {{ text = ""; }}
    try {{
      var bytes = new TextEncoder().encode(text);
      var binary = "";
      for (var i = 0; i < bytes.length; i++) {{ binary += String.fromCharCode(bytes[i]); }}
      window.location.href = "{HANDOFF_SCHEME}://{HANDOFF_HOST}/" + btoa(binary).split("+").join("-").split("/").join("_").split("=").join("");
    }} catch (error) {{}}
  }}
  try {{
    if (location.origin !== new URL(endpoint).origin) {{ return; }}
  }} catch (error) {{ return; }}
  fetch(endpoint, {{ credentials: "include", cache: "no-store" }})
    .then(function (response) {{ return response.text(); }})
    .then(report)
    .catch(function () {{ report(""); }});
}})();"#,
    )
}

fn inject_cookies(window: &WebviewWindow, cookies: &[SessionCookie]) -> tauri::Result<()> {
    for cookie in cookies {
        let Ok(value) = std::str::from_utf8(cookie.value.as_slice()) else {
            continue;
        };
        window.set_cookie(session_cookie(&cookie.name, value, &cookie.host_key))?;
    }
    Ok(())
}

fn clear_cookies(window: &WebviewWindow, cookies: &[SessionCookie]) {
    for cookie in cookies {
        let Ok(value) = std::str::from_utf8(cookie.value.as_slice()) else {
            continue;
        };
        let _ = window.delete_cookie(session_cookie(&cookie.name, value, &cookie.host_key));
    }
}

fn session_cookie(name: &str, value: &str, host_key: &str) -> Cookie<'static> {
    Cookie::build((name.to_owned(), value.to_owned()))
        .domain(host_key.to_owned())
        .path("/")
        .secure(true)
        .http_only(true)
        .same_site(SameSite::Lax)
        .build()
}

/// Reads the browser safe-storage key from the macOS Keychain.
///
/// # Errors
///
/// Returns [`CodexAuthError::KeychainDenied`] when the item is absent or the
/// user denies the authorization prompt. The returned key is zeroized on drop.
#[cfg(target_os = "macos")]
pub fn read_safe_storage_key(
    service: &str,
    account: &str,
) -> Result<Zeroizing<Vec<u8>>, CodexAuthError> {
    security_framework::passwords::get_generic_password(service, account)
        .map(Zeroizing::new)
        .map_err(|_| CodexAuthError::KeychainDenied)
}

/// Reads the browser safe-storage key from the Keychain.
///
/// # Errors
///
/// Always returns [`CodexAuthError::KeychainDenied`] on unsupported platforms.
#[cfg(not(target_os = "macos"))]
pub fn read_safe_storage_key(
    _service: &str,
    _account: &str,
) -> Result<Zeroizing<Vec<u8>>, CodexAuthError> {
    Err(CodexAuthError::KeychainDenied)
}

/// The endpoint body's raw text is classified by the domain layer.
#[cfg(test)]
mod tests {
    use super::{
        MAX_SESSION_BODY_BYTES, NavigationDecision, SessionTarget, accept_session_body,
        is_loopback_page, navigation_decision, resolve_session_url, same_page,
    };
    use router_core::codex_auth::CodexAuthError;
    use url::Url;
    use zeroize::Zeroizing;

    fn production() -> Url {
        Url::parse("https://chatgpt.com/api/auth/session").expect("production")
    }

    #[test]
    fn override_is_limited_to_loopback_pages() {
        assert!(matches!(
            resolve_session_url(production(), Some("http://127.0.0.1:8080/session")),
            SessionTarget::Override(_)
        ));
        assert!(matches!(
            resolve_session_url(production(), Some("https://chatgpt.com/api/auth/session")),
            SessionTarget::RejectedOverride(_)
        ));
        assert!(matches!(
            resolve_session_url(production(), Some("file:///tmp/session.json")),
            SessionTarget::RejectedOverride(_)
        ));
        assert!(matches!(
            resolve_session_url(production(), None),
            SessionTarget::Production(_)
        ));
    }

    #[test]
    fn loopback_detection_accepts_local_hosts_only() {
        assert!(is_loopback_page(
            &Url::parse("http://127.0.0.1:9000/x").expect("url")
        ));
        assert!(is_loopback_page(
            &Url::parse("http://localhost/x").expect("url")
        ));
        assert!(!is_loopback_page(
            &Url::parse("http://example.com/x").expect("url")
        ));
        assert!(!is_loopback_page(
            &Url::parse("https://chatgpt.com/").expect("url")
        ));
    }

    #[test]
    fn navigation_allows_only_the_target_and_the_handoff() {
        let target = production();
        assert_eq!(
            navigation_decision(&target, &target),
            NavigationDecision::Allow
        );
        assert_eq!(
            navigation_decision(&target, &Url::parse("https://example.com/").expect("url")),
            NavigationDecision::Cancel
        );
        assert_eq!(
            navigation_decision(
                &target,
                &Url::parse("airouter-codex-auth://body/aGVsbG8").expect("url")
            ),
            NavigationDecision::Capture("hello".to_owned())
        );
        assert_eq!(
            navigation_decision(
                &target,
                &Url::parse("airouter-codex-auth://body/").expect("url")
            ),
            NavigationDecision::Cancel
        );
    }

    #[test]
    fn handoff_accepts_the_standard_base64_a_naive_page_emits() {
        // A page that reaches for `btoa` emits the standard alphabet with `=`
        // padding; the payload must still decode, or every run fails closed
        // without a session.
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let target = production();
        let body = r#"{"accessToken":"a","sessionToken":"b","expires":"2027-01-15T08:00:00Z","user":{"id":"u"},"account":{"id":"acc"}}"#;
        let payload = STANDARD.encode(body);
        assert!(
            payload.contains('=') || payload.contains('+') || payload.contains('/'),
            "the fixture must not be URL-safe base64: {payload}"
        );
        let url = Url::parse(&format!("airouter-codex-auth://body/{payload}")).expect("url");
        assert_eq!(
            navigation_decision(&target, &url),
            NavigationDecision::Capture(body.to_owned())
        );
    }

    #[test]
    fn same_page_ignores_a_trailing_slash_and_port_default() {
        let target = production();
        assert!(same_page(
            &target,
            &Url::parse("https://chatgpt.com:443/api/auth/session/").expect("url")
        ));
        assert!(!same_page(
            &target,
            &Url::parse("https://chatgpt.com/api/auth/session/extra").expect("url")
        ));
    }

    #[test]
    fn oversized_session_bodies_fail_the_run_closed() {
        let small = accept_session_body(Zeroizing::new("{}".to_owned())).expect("small body");
        assert_eq!(small.as_str(), "{}");

        let at_limit = Zeroizing::new("x".repeat(MAX_SESSION_BODY_BYTES));
        assert!(accept_session_body(at_limit).is_ok());

        let over_limit = Zeroizing::new("x".repeat(MAX_SESSION_BODY_BYTES + 1));
        assert!(matches!(
            accept_session_body(over_limit),
            Err(CodexAuthError::SessionFetchFailed)
        ));
    }
}
