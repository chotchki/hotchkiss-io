//! DL.4 — check an external http(s) link.
//!
//! Behind an `ExternalChecker` trait (mirrors greylist's `CrawlerDns`) so the scan
//! is generic over it and tests inject a deterministic stub instead of hitting the
//! network — the default suite stays offline. `ReqwestChecker` is the real impl.

use std::time::Duration;

use super::class::CheckClass;

/// The result of one external check: verdict + HTTP status (if any) + a short human
/// note for the admin table.
#[derive(Debug, Clone)]
pub struct CheckOutcome {
    pub class: CheckClass,
    pub status: Option<u16>,
    pub detail: String,
}

/// Check one external URL. Generic seam so the scan can run offline in tests.
pub trait ExternalChecker: Send + Sync {
    fn check(&self, url: &str) -> impl std::future::Future<Output = CheckOutcome> + Send;
}

/// The identifying User-Agent — a webmaster who sees it in their logs knows exactly
/// what it is + where it came from. Version tracks the crate.
pub fn user_agent() -> String {
    format!(
        "hotchkiss.io-linkcheck/{} (+https://hotchkiss.io)",
        env!("CARGO_PKG_VERSION")
    )
}

/// Map an HTTP status to a class. After the HEAD→GET fallback + redirect-follow,
/// only 404/410 are confidently DEAD; other 4xx (401/403/405/451/999…) are BLOCKED
/// (the site rejects our request but the link likely works in a browser); 429 + 5xx
/// are TRANSIENT; 2xx/3xx are OK.
pub fn classify_status(status: u16) -> CheckClass {
    match status {
        200..=399 => CheckClass::Ok,
        404 | 410 => CheckClass::Dead,
        429 => CheckClass::Transient,
        500..=599 => CheckClass::Transient,
        // 999 is LinkedIn's anti-bot "request denied" — a block, not a server error.
        999 => CheckClass::Blocked,
        400..=499 => CheckClass::Blocked,
        _ => CheckClass::Transient,
    }
}

/// The real reqwest-backed checker. Holds ONE reused `Client` (built with an
/// identifying UA, explicit timeouts, redirects followed).
pub struct ReqwestChecker {
    client: reqwest::Client,
}

impl ReqwestChecker {
    /// Build the checker's client. Redirects are followed (a 301→200 is healthy),
    /// timeouts bound a slow host, TLS is rustls (matches the rest of the app).
    pub fn new() -> anyhow::Result<Self> {
        let client = reqwest::ClientBuilder::new()
            .use_rustls_tls()
            .user_agent(user_agent())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;
        Ok(Self { client })
    }

    async fn send(&self, method: reqwest::Method, url: &str) -> Result<u16, reqwest::Error> {
        let mut req = self.client.request(method.clone(), url);
        if method == reqwest::Method::GET {
            // Ask for one byte — we only want the status, not the body.
            req = req.header(reqwest::header::RANGE, "bytes=0-0");
        }
        Ok(req.send().await?.status().as_u16())
    }
}

impl ExternalChecker for ReqwestChecker {
    async fn check(&self, url: &str) -> CheckOutcome {
        // HEAD first (cheap); GET fallback if HEAD isn't a clean 2xx/3xx (many
        // servers 405 or lie about HEAD).
        if let Ok(status) = self.send(reqwest::Method::HEAD, url).await
            && (200..400).contains(&status)
        {
            return outcome_from_status(status);
        }
        match self.send(reqwest::Method::GET, url).await {
            Ok(status) => outcome_from_status(status),
            Err(e) => classify_error(&e),
        }
    }
}

fn outcome_from_status(status: u16) -> CheckOutcome {
    CheckOutcome {
        class: classify_status(status),
        status: Some(status),
        detail: format!("HTTP {status}"),
    }
}

/// Map a reqwest transport error (no HTTP status reached) to a class. A timeout is
/// TRANSIENT; a connect failure (DNS no-such-host OR connection refused) is DEAD —
/// the host/service isn't there. Confirm-before-alarm (3 consecutive Dead days)
/// absorbs the rare transient connect blip, so leaning Dead here is safe.
fn classify_error(e: &reqwest::Error) -> CheckOutcome {
    if e.is_timeout() {
        CheckOutcome {
            class: CheckClass::Transient,
            status: None,
            detail: "timeout".into(),
        }
    } else if e.is_connect() {
        CheckOutcome {
            class: CheckClass::Dead,
            status: None,
            detail: "connection failed (DNS or refused)".into(),
        }
    } else {
        let msg: String = e.to_string().chars().take(120).collect();
        CheckOutcome {
            class: CheckClass::Transient,
            status: None,
            detail: msg,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_buckets() {
        assert_eq!(classify_status(200), CheckClass::Ok);
        assert_eq!(classify_status(301), CheckClass::Ok);
        assert_eq!(classify_status(399), CheckClass::Ok);
        assert_eq!(classify_status(404), CheckClass::Dead);
        assert_eq!(classify_status(410), CheckClass::Dead);
        assert_eq!(classify_status(429), CheckClass::Transient);
        assert_eq!(classify_status(500), CheckClass::Transient);
        assert_eq!(classify_status(503), CheckClass::Transient);
        // Other 4xx = blocked (site rejects us, likely works in a browser).
        assert_eq!(classify_status(401), CheckClass::Blocked);
        assert_eq!(classify_status(403), CheckClass::Blocked);
        assert_eq!(classify_status(405), CheckClass::Blocked);
        assert_eq!(classify_status(451), CheckClass::Blocked);
        assert_eq!(classify_status(999), CheckClass::Blocked);
    }

    #[test]
    fn ua_carries_name_and_version() {
        let ua = user_agent();
        assert!(ua.starts_with("hotchkiss.io-linkcheck/"));
        assert!(ua.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn reqwest_checker_builds() {
        // The client config is valid (UA + timeouts + redirect policy compile+build).
        assert!(ReqwestChecker::new().is_ok());
    }
}
