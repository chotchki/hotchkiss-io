use anyhow::Result;
use sqlx::{
    prelude::FromRow,
    query, query_as,
    types::chrono::{DateTime, Utc},
    SqliteExecutor, SqlitePool,
};

/// A persisted request observation, projected for the "recent requests" view.
/// `ts` is stamped by SQLite (`CURRENT_TIMESTAMP`, UTC) on insert; `ip` /
/// `user_agent` are best-effort and may be null. (`id` and `referer` are also
/// columns on the table — `referer` is recorded but not surfaced here yet.)
/// `duration_ms` is SERVER-handler processing time (CQ.1), nullable: legacy
/// pre-CQ rows and beta-scrubbed rows carry NULL.
#[derive(Clone, Debug, FromRow)]
pub struct RequestLogDao {
    pub ts: DateTime<Utc>,
    pub method: String,
    pub path: String,
    pub status: i64,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub duration_ms: Option<i64>,
}

/// A request as observed by the logging middleware, before it gets an id / timestamp.
/// `duration_ms` is always measured on the live path (server-handler time); it lands
/// in the nullable column, so only legacy/beta-scrubbed rows are NULL.
#[derive(Clone, Debug)]
pub struct NewRequestLog {
    pub method: String,
    pub path: String,
    pub status: i64,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub referer: Option<String>,
    pub duration_ms: i64,
    /// Stored bot classification (CR.2), computed at write via `is_bot(user_agent)`.
    pub is_bot: bool,
}

#[derive(Clone, Debug)]
pub struct PathCount {
    pub path: String,
    pub count: i64,
}

#[derive(Clone, Debug)]
pub struct DayCount {
    pub day: String,
    pub count: i64,
}

#[derive(Clone, Debug)]
pub struct UserAgentCount {
    pub user_agent: Option<String>,
    pub count: i64,
}

#[derive(Clone, Debug)]
pub struct RefererCount {
    pub referer: String,
    pub count: i64,
}

/// Total / human / bot request counts over a window — the always-visible 3-chip
/// (CQ.2). `humans + bots == all` by construction (every row classifies as exactly
/// one, once backfilled). Directional: `is_bot` is a spoofable-User-Agent heuristic.
#[derive(Clone, Debug)]
pub struct AudienceCounts {
    pub all: i64,
    pub humans: i64,
    pub bots: i64,
}

/// Audience filter for the analytics dashboard (CQ.2): All (factual, the default),
/// Humans, or Bots — maps to the stored `is_bot` column (CR.2). Directional not
/// authoritative: `is_bot` keys off the spoofable User-Agent, so this never governs a
/// primary number on its own (the 3-chip shows all three at once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
    All,
    Humans,
    Bots,
}

impl Audience {
    /// The bound `is_bot` filter value (CR.2): `None` for All (the `?N IS NULL OR
    /// is_bot = ?N` predicate then matches every row, so the `ts` index still bounds
    /// the scan), `Some(0)` for Humans, `Some(1)` for Bots.
    pub fn as_bot_filter(self) -> Option<i64> {
        match self {
            Audience::All => None,
            Audience::Humans => Some(0),
            Audience::Bots => Some(1),
        }
    }

    /// The `?audience=` URL token + active-chip key.
    pub fn as_tag(self) -> &'static str {
        match self {
            Audience::All => "all",
            Audience::Humans => "humans",
            Audience::Bots => "bots",
        }
    }

    /// Parse `?audience=` — anything unrecognized falls back to All. NEVER errors: a
    /// bad param must not 500 the (GET-public-gated) admin dashboard.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("humans") => Audience::Humans,
            Some("bots") => Audience::Bots,
            _ => Audience::All,
        }
    }
}

/// The single-source bot/scanner classifier (CR.2): a UA is a bot if it's absent/empty
/// or contains any known bot/crawler/library/scanner/headless marker. Used at write
/// (the middleware stamps `is_bot`), by the startup backfill, and by the admin recompute
/// — so the ruleset stays RETUNABLE despite being stored (edit the list, run the
/// recompute). Ports the substrings the retired `request_log_view.ua_class` used.
/// DIRECTIONAL: the User-Agent is trivially spoofable, so this never governs a primary
/// number on its own (the 3-chip shows All/Humans/Bots at once).
pub fn is_bot(user_agent: Option<&str>) -> bool {
    let Some(ua) = user_agent else {
        return true; // no UA is overwhelmingly automated
    };
    if ua.is_empty() {
        return true;
    }
    let ua = ua.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "bot", "crawl", "spider", "slurp", "curl", "wget", "python", "go-http", "java/",
        "okhttp", "httpx", "axios", "node-fetch", "headless", "phantomjs", "scrapy",
        "masscan", "zgrab", "nmap", "semrush", "ahrefs", "mj12", "dotbot",
        "facebookexternalhit", "feedfetcher",
    ];
    MARKERS.iter().any(|m| ua.contains(m))
}

/// A per-IP distinct-404-path count at/above this flags a likely SCANNER (a client
/// walking a wordlist of dead paths). It's a BADGE + reuse signal (CQ.3), NOT a hard
/// filter on the volume-sorted leaderboard — a high-volume 200-only scraper should
/// still surface. The deferred IP-blocklist phase reuses this threshold as its
/// selection floor. Floored at 2 conceptually: 1 distinct 404 is just a dead link.
pub const SCAN_DISTINCT_404_THRESHOLD: i64 = 5;

/// Status-code breakdown over a window — the FACTUAL axis (CQ.3), a single row of
/// conditional counts. 403 + 404 are split OUT of `s4xx` on purpose: 403 = blocked,
/// 404 = the scanner-probe signal. Every status lands in exactly one bucket (1xx and
/// any status outside 200–599 are not counted — the app never emits them).
#[derive(Clone, Debug)]
pub struct StatusBucketCounts {
    pub s2xx: i64,
    pub s3xx: i64,
    pub s403: i64,
    pub s404: i64,
    /// 4xx OTHER than 403/404.
    pub s4xx: i64,
    pub s5xx: i64,
}

/// A per-IP activity row for the "who's-scanning-me" leaderboard (CQ.3). `distinct_404`
/// is the scan fingerprint (one client, many dead paths). This is ALSO the reuse seam
/// for the deferred IP-blocklist phase — hence the window-cutoff (not days) param.
#[derive(Clone, Debug)]
pub struct NoisyIp {
    pub ip: String,
    pub total: i64,
    pub distinct_paths: i64,
    pub distinct_404: i64,
    pub errors: i64,
}

impl NoisyIp {
    /// True if this IP tripped the scanner heuristic (many distinct dead paths).
    pub fn is_scanner(&self) -> bool {
        self.distinct_404 >= SCAN_DISTINCT_404_THRESHOLD
    }
}

/// One (path, status) pair + its hit count for a single IP's drill-down (CQ.4). The
/// header stats (total / distinct paths / distinct-404 wordlist / status mix) are
/// derived from these rows in Rust — no separate summary query.
#[derive(Clone, Debug)]
pub struct IpPathStatus {
    pub path: String,
    pub status: i64,
    pub count: i64,
}

/// One timed request for latency analysis (CQ.6): the raw path + its server-handler
/// duration. Percentiles are computed Rust-side (SQLite has no percentile fn), so this
/// pulls the whole windowed sample set — see the SPEC latency deferral (a SQL histogram
/// is the cheap next step when this gets slow).
#[derive(Clone, Debug)]
pub struct LatencySample {
    pub path: String,
    pub duration_ms: i64,
}

fn window(days: i64) -> String {
    // SQLite datetime modifier — `datetime('now', '-7 days')`. Still used by
    // `prune_before` (a DELETE); the analytics reads moved to `Window` (CT.1).
    format!("-{} days", days.max(0))
}

/// A concrete UTC time window `[from, to)` for the analytics queries, as SQLite
/// datetime strings (`YYYY-MM-DD HH:MM:SS` — how `ts` is stored: SQLite
/// `CURRENT_TIMESTAMP`, UTC). Both the fixed presets (7/30/90d) AND the custom
/// from/to picker (Phase CT) funnel through this: every windowed query filters
/// `ts >= from AND ts < to`. A preset is `[now - N days, FAR_FUTURE)` — unbounded
/// above, identical to the old `datetime('now','-N days')` behavior; a custom
/// range clamps either or both ends. Concrete strings (computed once in the
/// handler) keep every query on the same `ts` index range-scan, and the
/// fixed-width format makes the string compare a chronological compare.
pub struct Window {
    pub from: String,
    pub to: String,
}

/// Open-ended sentinels. The `ts` format is fixed-width, so these sort
/// lexicographically before/after any real timestamp.
const WINDOW_FAR_PAST: &str = "0000-01-01 00:00:00";
const WINDOW_FAR_FUTURE: &str = "9999-12-31 23:59:59";

impl Window {
    fn fmt(d: DateTime<Utc>) -> String {
        d.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    /// A preset lookback: `[now - days, unbounded)`. Computed via timestamp math
    /// (`sqlx::types::chrono` doesn't re-export the `TimeDelta`/`Duration` delta type,
    /// only the datetime types), which needs only `DateTime`/`Utc`.
    pub fn last_days(days: i64) -> Self {
        let now = Utc::now();
        let cutoff_secs = now.timestamp() - days.max(0) * 86_400;
        let from = DateTime::<Utc>::from_timestamp(cutoff_secs, 0).unwrap_or(now);
        Self {
            from: Self::fmt(from),
            to: WINDOW_FAR_FUTURE.to_string(),
        }
    }

    /// A custom range. Either bound may be open: a `None` `from` means "from the
    /// beginning", a `None` `to` means "through now and beyond" — so a from-only
    /// range is "since this instant" (the post-deploy p95 case, CT).
    pub fn custom(from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> Self {
        Self {
            from: from.map(Self::fmt).unwrap_or_else(|| WINDOW_FAR_PAST.to_string()),
            to: to.map(Self::fmt).unwrap_or_else(|| WINDOW_FAR_FUTURE.to_string()),
        }
    }
}

impl RequestLogDao {
    pub async fn insert(executor: impl SqliteExecutor<'_>, new: &NewRequestLog) -> Result<()> {
        query!(
            r#"
            INSERT INTO request_log (method, path, status, ip, user_agent, referer, duration_ms, is_bot)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            new.method,
            new.path,
            new.status,
            new.ip,
            new.user_agent,
            new.referer,
            new.duration_ms,
            new.is_bot,
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    pub async fn recent(
        executor: impl SqliteExecutor<'_>,
        limit: i64,
    ) -> Result<Vec<RequestLogDao>> {
        Ok(query_as!(
            RequestLogDao,
            r#"
            SELECT
                ts as "ts: DateTime<Utc>",
                method,
                path,
                status,
                ip,
                user_agent,
                duration_ms
            FROM request_log
            ORDER BY id DESC
            LIMIT ?1
            "#,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    pub async fn count_since(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        audience: Audience,
    ) -> Result<i64> {
        let bot = audience.as_bot_filter();
        Ok(query!(
            r#"
            SELECT COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2 AND (?3 IS NULL OR is_bot = ?3)
            "#,
            w.from,
            w.to,
            bot
        )
        .fetch_one(executor)
        .await?
        .count)
    }

    pub async fn distinct_ip_count(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        audience: Audience,
    ) -> Result<i64> {
        let bot = audience.as_bot_filter();
        Ok(query!(
            r#"
            SELECT COUNT(DISTINCT ip) as "count!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2 AND ip IS NOT NULL AND (?3 IS NULL OR is_bot = ?3)
            "#,
            w.from,
            w.to,
            bot
        )
        .fetch_one(executor)
        .await?
        .count)
    }

    /// Total / human / bot counts over the window in one pass — the always-visible
    /// 3-chip (CQ.2, on the stored `is_bot` since CR.2). `COUNT(CASE …)` never returns
    /// NULL (0 for an empty window), so the `!` annotations hold. `humans + bots == all`
    /// once the backfill has stamped every row (a NULL is_bot counts as neither).
    pub async fn audience_counts(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
    ) -> Result<AudienceCounts> {
        Ok(query_as!(
            AudienceCounts,
            r#"
            SELECT
                COUNT(*) as "all!: i64",
                COUNT(CASE WHEN is_bot = 0 THEN 1 END) as "humans!: i64",
                COUNT(CASE WHEN is_bot = 1 THEN 1 END) as "bots!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2
            "#,
            w.from,
            w.to
        )
        .fetch_one(executor)
        .await?)
    }

    pub async fn count_by_user_agent(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        limit: i64,
    ) -> Result<Vec<UserAgentCount>> {
        Ok(query_as!(
            UserAgentCount,
            r#"
            SELECT user_agent, COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2
            GROUP BY user_agent
            ORDER BY COUNT(*) DESC, user_agent ASC
            LIMIT ?3
            "#,
            w.from,
            w.to,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    /// ALL distinct non-null referers over the window (CQ.5), UNBOUNDED — the caller
    /// (`web::util::referer::group_referers`) does host-grouping + classification in
    /// Rust, and an accurate "N polluting referers hidden" needs the full count=1
    /// IP-literal/malformed tail a LIMIT would drop. HONEST caveat: referrer spam is
    /// high-cardinality BY DESIGN, so this fetch is the phase's growth driver — the fix
    /// at that trigger is a stored+indexed `referer_host` column, NOT a LIMIT band-aid
    /// (SPEC deferrals). Replaces the shipped `count_by_referer` (which grouped by full
    /// URL and self-filtered with a substring `LIKE` that mis-swallowed lookalikes).
    pub async fn referer_urls_since(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
    ) -> Result<Vec<RefererCount>> {
        Ok(query_as!(
            RefererCount,
            r#"
            SELECT referer as "referer!: String", COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2 AND referer IS NOT NULL
            GROUP BY referer
            ORDER BY COUNT(*) DESC, referer ASC
            "#,
            w.from,
            w.to
        )
        .fetch_all(executor)
        .await?)
    }

    /// Count of requests with NO referer (direct / stripped) over the window (CQ.5) —
    /// the "direct" number, core to the sources picture.
    pub async fn direct_referer_count(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
    ) -> Result<i64> {
        Ok(query!(
            r#"SELECT COUNT(*) as "count!: i64" FROM request_log WHERE ts >= ?1 AND ts < ?2 AND referer IS NULL"#,
            w.from,
            w.to
        )
        .fetch_one(executor)
        .await?
        .count)
    }

    pub async fn count_by_day(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        audience: Audience,
    ) -> Result<Vec<DayCount>> {
        let bot = audience.as_bot_filter();
        Ok(query_as!(
            DayCount,
            r#"
            SELECT substr(ts, 1, 10) as "day!: String", COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2 AND (?3 IS NULL OR is_bot = ?3)
            GROUP BY substr(ts, 1, 10)
            ORDER BY substr(ts, 1, 10) ASC
            "#,
            w.from,
            w.to,
            bot
        )
        .fetch_all(executor)
        .await?)
    }

    /// Unique visitors (distinct IP) per day over the window. NULL-ip rows are
    /// excluded — they can't be attributed to a visitor.
    pub async fn distinct_ip_by_day(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        audience: Audience,
    ) -> Result<Vec<DayCount>> {
        let bot = audience.as_bot_filter();
        Ok(query_as!(
            DayCount,
            r#"
            SELECT substr(ts, 1, 10) as "day!: String", COUNT(DISTINCT ip) as "count!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2 AND ip IS NOT NULL AND (?3 IS NULL OR is_bot = ?3)
            GROUP BY substr(ts, 1, 10)
            ORDER BY substr(ts, 1, 10) ASC
            "#,
            w.from,
            w.to,
            bot
        )
        .fetch_all(executor)
        .await?)
    }

    /// Top paths over the window, excluding static assets + well-known files so
    /// real routes rank. `max_status` is an exclusive ceiling on the HTTP status:
    /// pass 400 for successful-only ("Content"), or a high value (e.g. 10000) to
    /// include 4xx/5xx — the bot/scanner probes ("All"). The static exclusion is
    /// a plain prefix/exact set — amend it here if a new static prefix shows up.
    pub async fn count_by_content_path(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        audience: Audience,
        max_status: i64,
        limit: i64,
    ) -> Result<Vec<PathCount>> {
        let bot = audience.as_bot_filter();
        Ok(query_as!(
            PathCount,
            r#"
            SELECT path, COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2
              AND status < ?3
              AND (?4 IS NULL OR is_bot = ?4)
              AND path NOT LIKE '/styles%'
              AND path NOT LIKE '/vendor%'
              AND path NOT LIKE '/scripts%'
              AND path NOT LIKE '/images%'
              AND path NOT LIKE '/attachments%'
              AND path NOT LIKE '/diagram%'
              AND path NOT IN ('/favicon.ico', '/manifest.webmanifest', '/robots.txt', '/apple-touch-icon.png')
            GROUP BY path
            ORDER BY COUNT(*) DESC, path ASC
            LIMIT ?5
            "#,
            w.from,
            w.to,
            max_status,
            bot,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    /// Status-code breakdown over the window (CQ.3) — one row, conditional counts.
    /// Audience-filtered like the headline. COUNT(CASE …) never returns NULL, so the
    /// `!` annotations hold.
    pub async fn count_by_status_bucket(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        audience: Audience,
    ) -> Result<StatusBucketCounts> {
        let bot = audience.as_bot_filter();
        Ok(query_as!(
            StatusBucketCounts,
            r#"
            SELECT
                COUNT(CASE WHEN status BETWEEN 200 AND 299 THEN 1 END) as "s2xx!: i64",
                COUNT(CASE WHEN status BETWEEN 300 AND 399 THEN 1 END) as "s3xx!: i64",
                COUNT(CASE WHEN status = 403 THEN 1 END) as "s403!: i64",
                COUNT(CASE WHEN status = 404 THEN 1 END) as "s404!: i64",
                COUNT(CASE WHEN status BETWEEN 400 AND 499 AND status NOT IN (403, 404) THEN 1 END) as "s4xx!: i64",
                COUNT(CASE WHEN status BETWEEN 500 AND 599 THEN 1 END) as "s5xx!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2 AND (?3 IS NULL OR is_bot = ?3)
            "#,
            w.from,
            w.to,
            bot
        )
        .fetch_one(executor)
        .await?)
    }

    /// Per-IP "who's-scanning-me" leaderboard (CQ.3), sorted by VOLUME so a
    /// high-volume 200-only scraper surfaces too — the scanner badge
    /// (`distinct_404 >= SCAN_DISTINCT_404_THRESHOLD`) is computed in Rust, not the
    /// primary rank. **NOT audience-filtered** (raw `request_log`): a UA-spoofing
    /// scanner claiming to be a browser is exactly what this must still catch.
    ///
    /// `WHERE ip IS NOT NULL` is NON-NEGOTIABLE — a NULL in a GROUP BY is its own
    /// bucket and would poison the leaderboard. Takes the same `Window` as every other
    /// windowed read (CT.1), so the deferred IP-blocklist phase still reuses this fn for
    /// its "N 404s in M MINUTES" variant — it just passes `Window::custom(Some(now -
    /// 15min), None)` and a `min_distinct_404` of `SCAN_DISTINCT_404_THRESHOLD` (the
    /// dashboard leaves it at 0 to see everyone).
    pub async fn noisy_ips(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        min_distinct_404: i64,
        limit: i64,
    ) -> Result<Vec<NoisyIp>> {
        Ok(query_as!(
            NoisyIp,
            r#"
            SELECT
                ip as "ip!: String",
                COUNT(*) as "total!: i64",
                COUNT(DISTINCT path) as "distinct_paths!: i64",
                COUNT(DISTINCT CASE WHEN status = 404 THEN path END) as "distinct_404!: i64",
                COUNT(CASE WHEN status >= 400 THEN 1 END) as "errors!: i64"
            FROM request_log
            WHERE ip IS NOT NULL AND ts >= ?1 AND ts < ?2
            GROUP BY ip
            HAVING COUNT(DISTINCT CASE WHEN status = 404 THEN path END) >= ?3
            ORDER BY COUNT(*) DESC, ip ASC
            LIMIT ?4
            "#,
            w.from,
            w.to,
            min_distinct_404,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    /// Paths that NEVER succeeded over the window (CQ.3) — the scanner-signature list.
    /// `HAVING SUM(status < 400) = 0` = no request to this path ever returned < 400.
    /// Labeled honestly: this includes 403-only / 5xx-only probes, not just 404s.
    pub async fn never_succeeded_paths(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        limit: i64,
    ) -> Result<Vec<PathCount>> {
        Ok(query_as!(
            PathCount,
            r#"
            SELECT path, COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2
            GROUP BY path
            HAVING SUM(CASE WHEN status < 400 THEN 1 ELSE 0 END) = 0
            ORDER BY COUNT(*) DESC, path ASC
            LIMIT ?3
            "#,
            w.from,
            w.to,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    /// Every (path, status) pair + count for ONE ip over the window (CQ.4). The
    /// drill-down derives the header (total / distinct paths / distinct-404 wordlist /
    /// status mix) from these in Rust. Exact `ip = ?` match (raw, not the view).
    pub async fn ip_path_status(
        executor: impl SqliteExecutor<'_>,
        ip: &str,
        w: &Window,
    ) -> Result<Vec<IpPathStatus>> {
        Ok(query_as!(
            IpPathStatus,
            r#"
            SELECT path, status, COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ip = ?1 AND ts >= ?2 AND ts < ?3
            GROUP BY path, status
            ORDER BY COUNT(*) DESC, path ASC
            "#,
            ip,
            w.from,
            w.to
        )
        .fetch_all(executor)
        .await?)
    }

    /// User-Agent breakdown for ONE ip over the window (CQ.4) — a rotating-UA client
    /// (many UAs from one IP) is itself a bot tell.
    pub async fn ip_user_agents(
        executor: impl SqliteExecutor<'_>,
        ip: &str,
        w: &Window,
    ) -> Result<Vec<UserAgentCount>> {
        Ok(query_as!(
            UserAgentCount,
            r#"
            SELECT user_agent, COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ip = ?1 AND ts >= ?2 AND ts < ?3
            GROUP BY user_agent
            ORDER BY COUNT(*) DESC, user_agent ASC
            "#,
            ip,
            w.from,
            w.to
        )
        .fetch_all(executor)
        .await?)
    }

    /// The most recent raw requests from ONE ip (CQ.4), newest first.
    pub async fn ip_recent(
        executor: impl SqliteExecutor<'_>,
        ip: &str,
        limit: i64,
    ) -> Result<Vec<RequestLogDao>> {
        Ok(query_as!(
            RequestLogDao,
            r#"
            SELECT
                ts as "ts: DateTime<Utc>",
                method,
                path,
                status,
                ip,
                user_agent,
                duration_ms
            FROM request_log
            WHERE ip = ?1
            ORDER BY id DESC
            LIMIT ?2
            "#,
            ip,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    /// Every timed request over the window (CQ.6): (raw path, duration_ms) where
    /// `duration_ms IS NOT NULL` (legacy/beta-scrubbed rows have none). The exclusion
    /// set drops the truly-static embedded assets (in-memory, ~0ms, pure noise) but
    /// deliberately KEEPS `/diagram` (d2 subprocess) + `/media` (external-drive I/O) —
    /// those are the HIGHEST-value latency targets, unlike `count_by_content_path`
    /// which folds them out. Unbounded: percentiles need the full sample.
    pub async fn latency_samples(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
    ) -> Result<Vec<LatencySample>> {
        Ok(query_as!(
            LatencySample,
            r#"
            SELECT path, duration_ms as "duration_ms!: i64"
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2
              AND duration_ms IS NOT NULL
              AND path NOT LIKE '/styles%'
              AND path NOT LIKE '/vendor%'
              AND path NOT LIKE '/scripts%'
              AND path NOT LIKE '/images%'
              AND path NOT IN ('/favicon.ico', '/manifest.webmanifest', '/robots.txt', '/apple-touch-icon.png')
            "#,
            w.from,
            w.to
        )
        .fetch_all(executor)
        .await?)
    }

    /// The slowest individual requests over the window (CQ.6), newest-of-the-worst
    /// first. Raw un-normalized path (so an exact slow URL is visible). Excludes NULL
    /// durations (they'd sort last under DESC anyway, but be explicit).
    pub async fn slowest_requests(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
        limit: i64,
    ) -> Result<Vec<RequestLogDao>> {
        Ok(query_as!(
            RequestLogDao,
            r#"
            SELECT
                ts as "ts: DateTime<Utc>",
                method,
                path,
                status,
                ip,
                user_agent,
                duration_ms
            FROM request_log
            WHERE ts >= ?1 AND ts < ?2 AND duration_ms IS NOT NULL
            ORDER BY duration_ms DESC, id DESC
            LIMIT ?3
            "#,
            w.from,
            w.to,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    /// Recompute the stored `is_bot` for existing rows via the single-source `is_bot()`
    /// classifier (CR.2). `only_missing = true` is the idempotent startup BACKFILL (rows
    /// where `is_bot IS NULL`); `false` is the admin RECOMPUTE (every row — e.g. after
    /// retuning the ruleset). Returns the rows updated. Efficient: classifies each
    /// DISTINCT user_agent ONCE in Rust, then one bulk `UPDATE … WHERE user_agent = ?`
    /// per value (indexed by `idx_request_log_user_agent`). A NULL UA is its own bulk
    /// update. (A pathological UA-randomizing scanner inflates the distinct count → many
    /// small updates; acceptable for a background/admin op at personal-site scale.)
    pub async fn reclassify_bots(pool: &SqlitePool, only_missing: bool) -> Result<u64> {
        let uas: Vec<Option<String>> = if only_missing {
            query!("SELECT DISTINCT user_agent FROM request_log WHERE is_bot IS NULL")
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(|r| r.user_agent)
                .collect()
        } else {
            query!("SELECT DISTINCT user_agent FROM request_log")
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(|r| r.user_agent)
                .collect()
        };

        let mut updated = 0u64;
        for ua in uas {
            let bot = i64::from(is_bot(ua.as_deref()));
            let res = match (ua.as_deref(), only_missing) {
                (Some(u), true) => {
                    query!(
                        "UPDATE request_log SET is_bot = ?1 WHERE user_agent = ?2 AND is_bot IS NULL",
                        bot, u
                    ).execute(pool).await?
                }
                (Some(u), false) => {
                    query!("UPDATE request_log SET is_bot = ?1 WHERE user_agent = ?2", bot, u)
                        .execute(pool).await?
                }
                (None, true) => {
                    query!(
                        "UPDATE request_log SET is_bot = ?1 WHERE user_agent IS NULL AND is_bot IS NULL",
                        bot
                    ).execute(pool).await?
                }
                (None, false) => {
                    query!("UPDATE request_log SET is_bot = ?1 WHERE user_agent IS NULL", bot)
                        .execute(pool).await?
                }
            };
            updated += res.rows_affected();
        }
        Ok(updated)
    }

    /// Delete rows older than `retain_days`. Returns the number removed.
    pub async fn prune_before(executor: impl SqliteExecutor<'_>, retain_days: i64) -> Result<u64> {
        let w = window(retain_days);
        Ok(query!(
            r#"DELETE FROM request_log WHERE ts < datetime('now', ?1)"#,
            w
        )
        .execute(executor)
        .await?
        .rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    fn entry(path: &str, status: i64, ip: Option<&str>, ua: Option<&str>) -> NewRequestLog {
        NewRequestLog {
            method: "GET".to_string(),
            path: path.to_string(),
            status,
            ip: ip.map(String::from),
            user_agent: ua.map(String::from),
            referer: None,
            duration_ms: 0,
            is_bot: is_bot(ua),
        }
    }

    async fn seed(pool: &SqlitePool) -> Result<()> {
        for e in [
            entry("/pages/Resume", 200, Some("1.2.3.4"), Some("curl/8")),
            entry("/pages/Resume", 200, Some("1.2.3.4"), Some("curl/8")),
            entry("/pages/Resume", 200, Some("5.6.7.8"), Some("Mozilla/5")),
            entry("/login", 200, Some("5.6.7.8"), Some("Mozilla/5")),
            entry("/wp-admin", 404, None, None),
        ] {
            RequestLogDao::insert(pool, &e).await?;
        }
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn insert_and_recent(pool: SqlitePool) -> Result<()> {
        seed(&pool).await?;
        let recent = RequestLogDao::recent(&pool, 3).await?;
        assert_eq!(recent.len(), 3);
        // most recent first
        assert_eq!(recent[0].path, "/wp-admin");
        assert_eq!(recent[0].status, 404);
        assert!(recent[0].ip.is_none());
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn duration_ms_roundtrips(pool: SqlitePool) -> Result<()> {
        // A live-logged request carries its measured server-handler time.
        let mut logged = entry("/", 200, None, None);
        logged.duration_ms = 42;
        RequestLogDao::insert(&pool, &logged).await?;
        // A legacy/beta-scrubbed row predates the column — NULL duration.
        query!("INSERT INTO request_log (method, path, status) VALUES ('GET', '/legacy', 200)")
            .execute(&pool)
            .await?;

        let recent = RequestLogDao::recent(&pool, 10).await?;
        let got = |p: &str| recent.iter().find(|r| r.path == p).unwrap().duration_ms;
        assert_eq!(got("/"), Some(42), "measured duration round-trips");
        assert_eq!(got("/legacy"), None, "legacy NULL stays None, not 0");
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn aggregates(pool: SqlitePool) -> Result<()> {
        seed(&pool).await?;

        assert_eq!(RequestLogDao::count_since(&pool, &Window::last_days(1), Audience::All).await?, 5);
        assert_eq!(RequestLogDao::distinct_ip_count(&pool, &Window::last_days(1), Audience::All).await?, 2);

        let by_path = RequestLogDao::count_by_content_path(&pool, &Window::last_days(1), Audience::All, 400, 10).await?;
        assert_eq!(by_path[0].path, "/pages/Resume");
        assert_eq!(by_path[0].count, 3);

        let by_ua = RequestLogDao::count_by_user_agent(&pool, &Window::last_days(1), 10).await?;
        // curl/8 and Mozilla/5 each appear; curl/8 has 2, Mozilla/5 has 2, plus one NULL
        assert!(by_ua.iter().any(|u| u.user_agent.as_deref() == Some("curl/8") && u.count == 2));

        let by_day = RequestLogDao::count_by_day(&pool, &Window::last_days(1), Audience::All).await?;
        assert_eq!(by_day.len(), 1); // all seeded "now"
        assert_eq!(by_day[0].count, 5);

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn window_bounds_exclude_outside_range(pool: SqlitePool) -> Result<()> {
        // A recent row (now) and an old row (100 days ago, via raw ts).
        RequestLogDao::insert(&pool, &entry("/recent", 200, Some("1.1.1.1"), None)).await?;
        query!("INSERT INTO request_log (ts, method, path, status) VALUES (datetime('now','-100 days'),'GET','/old',200)")
            .execute(&pool)
            .await?;

        let now = Utc::now();
        let at = |days_ago: i64| DateTime::<Utc>::from_timestamp(now.timestamp() - days_ago * 86_400, 0);

        // Preset is unbounded above: last 1 day → only the recent row.
        assert_eq!(
            RequestLogDao::count_since(&pool, &Window::last_days(1), Audience::All).await?,
            1,
            "last_days(1) sees only the recent row"
        );
        // A CLOSED window straddling only the OLD row: [-150d, -50d) — the UPPER bound
        // excludes 'now', the new capability this whole phase turns on.
        assert_eq!(
            RequestLogDao::count_since(&pool, &Window::custom(at(150), at(50)), Audience::All).await?,
            1,
            "only the -100d row is inside [-150d, -50d); the upper bound drops the recent one"
        );
        // A window entirely in the past (to before everything) → empty.
        assert_eq!(
            RequestLogDao::count_since(&pool, &Window::custom(None, at(200)), Audience::All).await?,
            0,
            "nothing precedes 200 days ago"
        );
        // from-only (to open) → "since this instant" through now = the recent row.
        assert_eq!(
            RequestLogDao::count_since(&pool, &Window::custom(at(1), None), Audience::All).await?,
            1,
            "from-only is 'since this instant', to = open"
        );
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn prune(pool: SqlitePool) -> Result<()> {
        seed(&pool).await?;
        // an old row
        query!(
            "INSERT INTO request_log (ts, method, path, status) VALUES (datetime('now', '-100 days'), 'GET', '/old', 200)"
        )
        .execute(&pool)
        .await?;

        assert_eq!(RequestLogDao::count_since(&pool, &Window::last_days(365), Audience::All).await?, 6);
        let removed = RequestLogDao::prune_before(&pool, 90).await?;
        assert_eq!(removed, 1);
        assert_eq!(RequestLogDao::count_since(&pool, &Window::last_days(365), Audience::All).await?, 5);

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn content_path_excludes_static(pool: SqlitePool) -> Result<()> {
        for e in [
            entry("/pages/Resume", 200, Some("1.1.1.1"), None),
            entry("/pages/Resume", 200, Some("1.1.1.1"), None),
            entry("/blog/hello", 200, Some("2.2.2.2"), None),
            entry("/styles/main.css", 200, Some("1.1.1.1"), None),
            entry("/vendor/htmx/htmx.js", 200, Some("1.1.1.1"), None),
            entry("/diagram/abc123", 200, Some("1.1.1.1"), None),
            entry("/favicon.ico", 200, Some("1.1.1.1"), None),
            entry("/cgi-bin/luci", 404, Some("9.9.9.9"), None), // bot probe, 404
        ] {
            RequestLogDao::insert(&pool, &e).await?;
        }

        let top = RequestLogDao::count_by_content_path(&pool, &Window::last_days(1), Audience::All, 400, 25).await?;
        let paths: Vec<&str> = top.iter().map(|p| p.path.as_str()).collect();

        assert!(paths.contains(&"/pages/Resume"), "content page must rank: {paths:?}");
        assert!(paths.contains(&"/blog/hello"));
        assert!(
            !paths.iter().any(|p| {
                p.starts_with("/styles")
                    || p.starts_with("/vendor")
                    || p.starts_with("/diagram")
                    || *p == "/favicon.ico"
            }),
            "static assets must be excluded: {paths:?}"
        );
        assert!(
            !paths.contains(&"/cgi-bin/luci"),
            "404 scanner probes must not rank as top pages: {paths:?}"
        );

        // "All" mode (high status ceiling) surfaces the 404 probe but still
        // drops static assets.
        let all = RequestLogDao::count_by_content_path(&pool, &Window::last_days(1), Audience::All, 10_000, 25).await?;
        let all_paths: Vec<&str> = all.iter().map(|p| p.path.as_str()).collect();
        assert!(
            all_paths.contains(&"/cgi-bin/luci"),
            "All mode should surface probes: {all_paths:?}"
        );
        assert!(
            !all_paths.iter().any(|p| p.starts_with("/styles")),
            "All mode still excludes static: {all_paths:?}"
        );
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn unique_per_day_below_total(pool: SqlitePool) -> Result<()> {
        for e in [
            entry("/pages/Resume", 200, Some("1.1.1.1"), None),
            entry("/pages/Resume", 200, Some("1.1.1.1"), None), // same visitor
            entry("/pages/Resume", 200, Some("2.2.2.2"), None),
            entry("/pages/Resume", 200, None, None), // null ip — not a unique visitor
        ] {
            RequestLogDao::insert(&pool, &e).await?;
        }

        let total = RequestLogDao::count_by_day(&pool, &Window::last_days(1), Audience::All).await?;
        let unique = RequestLogDao::distinct_ip_by_day(&pool, &Window::last_days(1), Audience::All).await?;
        assert_eq!(total[0].count, 4, "4 total views");
        assert_eq!(unique[0].count, 2, "2 distinct IPs (null excluded)");
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn audience_classifies_and_sums(pool: SqlitePool) -> Result<()> {
        for e in [
            entry("/", 200, Some("1.1.1.1"), Some("Mozilla/5.0 (real browser)")), // human
            entry("/", 200, Some("2.2.2.2"), Some("Googlebot/2.1")),              // bot (substr)
            entry("/wp-admin", 404, Some("3.3.3.3"), Some("python-requests/2.31")), // bot
            entry("/", 200, Some("4.4.4.4"), None),                              // bot (null UA)
        ] {
            RequestLogDao::insert(&pool, &e).await?;
        }

        let c = RequestLogDao::audience_counts(&pool, &Window::last_days(1)).await?;
        assert_eq!(c.all, 4);
        assert_eq!(c.humans, 1, "only the real-browser UA is human");
        assert_eq!(c.bots, 3, "googlebot + python-requests + null-UA");
        assert_eq!(c.humans + c.bots, c.all, "the honesty invariant: no row is both/neither");

        // The audience filter re-buckets the headline count the same way.
        assert_eq!(RequestLogDao::count_since(&pool, &Window::last_days(1), Audience::All).await?, 4);
        assert_eq!(RequestLogDao::count_since(&pool, &Window::last_days(1), Audience::Humans).await?, 1);
        assert_eq!(RequestLogDao::count_since(&pool, &Window::last_days(1), Audience::Bots).await?, 3);
        Ok(())
    }

    #[test]
    fn is_bot_classifier() {
        assert!(is_bot(None), "no UA → bot");
        assert!(is_bot(Some("")), "empty UA → bot");
        assert!(is_bot(Some("Googlebot/2.1")));
        assert!(is_bot(Some("python-requests/2.31")));
        assert!(is_bot(Some("curl/8.0")));
        assert!(
            !is_bot(Some("Mozilla/5.0 (Macintosh; Intel Mac OS X) Safari/605")),
            "a real browser is human"
        );
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn reclassify_backfills_null_is_bot(pool: SqlitePool) -> Result<()> {
        // Legacy rows (is_bot NULL) via raw insert + a fresh row already stamped by insert().
        query!("INSERT INTO request_log (method, path, status, user_agent) VALUES ('GET','/',200,'Googlebot/2.1')")
            .execute(&pool).await?;
        query!("INSERT INTO request_log (method, path, status, user_agent) VALUES ('GET','/',200,'Mozilla/5.0 real browser')")
            .execute(&pool).await?;
        RequestLogDao::insert(&pool, &entry("/", 200, Some("1.1.1.1"), Some("curl/8"))).await?;

        let null_before: i64 =
            query!(r#"SELECT COUNT(*) as "c!: i64" FROM request_log WHERE is_bot IS NULL"#)
                .fetch_one(&pool).await?.c;
        assert_eq!(null_before, 2, "the two raw rows are unstamped; the inserted one isn't");

        let updated = RequestLogDao::reclassify_bots(&pool, true).await?;
        assert_eq!(updated, 2, "backfill only touches the NULL rows");

        let c = RequestLogDao::audience_counts(&pool, &Window::last_days(1)).await?;
        assert_eq!(c.all, 3);
        assert_eq!(c.humans, 1, "the real browser");
        assert_eq!(c.bots, 2, "googlebot + curl");
        assert_eq!(c.humans + c.bots, c.all, "no NULL is_bot left → the invariant holds");
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn referer_urls_and_direct_count(pool: SqlitePool) -> Result<()> {
        // referer_urls_since returns ALL distinct non-null referers (unbounded, no host
        // filter); classification is group_referers' job (tested in web::util::referer).
        let r = |referer: Option<&str>| NewRequestLog {
            method: "GET".to_string(),
            path: "/".to_string(),
            status: 200,
            ip: None,
            user_agent: None,
            referer: referer.map(String::from),
            duration_ms: 0,
            is_bot: is_bot(None),
        };
        for e in [
            r(Some("https://news.ycombinator.com/")),
            r(Some("https://news.ycombinator.com/")),
            r(Some("https://hotchkiss.io/blog")), // internal — NOT filtered at the query level anymore
            r(None),                              // direct
            r(None),                              // direct
        ] {
            RequestLogDao::insert(&pool, &e).await?;
        }

        let refs = RequestLogDao::referer_urls_since(&pool, &Window::last_days(1)).await?;
        // 2 distinct non-null referers (HN + self), unbounded — self is kept here and
        // dropped later by group_referers, not by the SQL.
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].referer, "https://news.ycombinator.com/");
        assert_eq!(refs[0].count, 2);
        assert_eq!(RequestLogDao::direct_referer_count(&pool, &Window::last_days(1)).await?, 2);
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn status_buckets_split_403_and_404(pool: SqlitePool) -> Result<()> {
        for e in [
            entry("/", 200, Some("1.1.1.1"), None),
            entry("/moved", 301, Some("1.1.1.1"), None),
            entry("/secret", 403, Some("2.2.2.2"), None),
            entry("/wp-admin", 404, Some("3.3.3.3"), None),
            entry("/wp-login", 404, Some("3.3.3.3"), None),
            entry("/bad-req", 400, Some("4.4.4.4"), None), // other-4xx
            entry("/boom", 500, Some("5.5.5.5"), None),
        ] {
            RequestLogDao::insert(&pool, &e).await?;
        }
        let b = RequestLogDao::count_by_status_bucket(&pool, &Window::last_days(1), Audience::All).await?;
        assert_eq!(b.s2xx, 1);
        assert_eq!(b.s3xx, 1);
        assert_eq!(b.s403, 1);
        assert_eq!(b.s404, 2);
        assert_eq!(b.s4xx, 1, "400 is other-4xx, NOT double-counted with 403/404");
        assert_eq!(b.s5xx, 1);
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn noisy_ips_flags_scanners_and_never_poisons_on_null(pool: SqlitePool) -> Result<()> {
        // A scanner: one IP hitting SCAN_DISTINCT_404_THRESHOLD distinct dead paths.
        for i in 0..SCAN_DISTINCT_404_THRESHOLD {
            RequestLogDao::insert(&pool, &entry(&format!("/probe-{i}"), 404, Some("9.9.9.9"), None))
                .await?;
        }
        // A real visitor: volume, but no 404 fanout.
        for _ in 0..3 {
            RequestLogDao::insert(&pool, &entry("/", 200, Some("1.1.1.1"), None)).await?;
        }
        // NULL-ip rows (the poison boundary) — must NEVER appear in the leaderboard.
        RequestLogDao::insert(&pool, &entry("/wp-admin", 404, None, None)).await?;

        let rows = RequestLogDao::noisy_ips(&pool, &Window::last_days(1), 0, 25).await?;
        assert_eq!(rows.len(), 2, "exactly the two real IPs — NULL ip excluded, not its own bucket");

        let scanner = rows.iter().find(|r| r.ip == "9.9.9.9").expect("scanner present");
        assert_eq!(scanner.distinct_404, SCAN_DISTINCT_404_THRESHOLD);
        assert!(scanner.is_scanner(), "distinct_404 >= threshold → scanner badge");

        let visitor = rows.iter().find(|r| r.ip == "1.1.1.1").expect("visitor present (volume-sorted)");
        assert_eq!(visitor.distinct_404, 0);
        assert!(!visitor.is_scanner(), "a 200-only high-volume visitor is NOT flagged");

        // The blocklist-reuse seam: min_distinct_404 filters to offenders only.
        let offenders =
            RequestLogDao::noisy_ips(&pool, &Window::last_days(1), SCAN_DISTINCT_404_THRESHOLD, 25).await?;
        assert_eq!(offenders.len(), 1);
        assert_eq!(offenders[0].ip, "9.9.9.9");

        // One below the floor is NOT selected by that filter.
        RequestLogDao::insert(&pool, &entry("/one-404", 404, Some("8.8.8.8"), None)).await?;
        let still_one =
            RequestLogDao::noisy_ips(&pool, &Window::last_days(1), SCAN_DISTINCT_404_THRESHOLD, 25).await?;
        assert_eq!(still_one.len(), 1, "1 distinct-404 is below the scanner floor");
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn never_succeeded_lists_probe_paths_only(pool: SqlitePool) -> Result<()> {
        for e in [
            entry("/wp-admin", 404, Some("9.9.9.9"), None),
            entry("/wp-admin", 404, Some("9.9.9.9"), None), // only ever 404
            entry("/.env", 403, Some("9.9.9.9"), None),     // only ever 403 — still "never succeeded"
            entry("/blog", 200, Some("1.1.1.1"), None),     // succeeded → excluded
            entry("/blog", 404, Some("1.1.1.1"), None),     // a 404 too, but it DID succeed once
        ] {
            RequestLogDao::insert(&pool, &e).await?;
        }
        let probes = RequestLogDao::never_succeeded_paths(&pool, &Window::last_days(1), 25).await?;
        let paths: Vec<&str> = probes.iter().map(|p| p.path.as_str()).collect();
        assert!(paths.contains(&"/wp-admin"));
        assert!(paths.contains(&"/.env"), "403-only counts as never-succeeded");
        assert!(!paths.contains(&"/blog"), "a path that succeeded once is excluded");
        Ok(())
    }
}
