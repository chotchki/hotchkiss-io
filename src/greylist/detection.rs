//! Behavioral greylist detection (CX.2).
//!
//! Pure scoring over per-IP features derived from `request_log`. The rules are a hand-tuned
//! linear classifier; the `IpFeatures` -> `Verdict` split is deliberate so a fitted model
//! can replace [`score`] without touching feature extraction (design doc: "Detection").
//!
//! UA/DNS-blind by construction — [`score`] sees only counts. The verified-crawler
//! exemption (FCrDNS, CX.3) is applied by the SWEEP for the rules that carry it
//! ([`Rule::exempts_verified_crawlers`]); R1 (signature probe) never exempts, because
//! nothing legitimate probes `wp-login.php`.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

use crate::db::dao::request_log::IpPathAgg;

/// Path fragments this site NEVER serves — a hit is a dead-certain scanner tell, matched
/// case-insensitively as a substring of the request path. Single source: also used by the
/// refinement panel (CX.9) and the tests. KEEP this to things the site genuinely never
/// serves (a false entry greylists real visitors); when in doubt leave it out and let
/// R2/R3 catch it. Note `.php` carries the leading dot on purpose — a slug like
/// `why-i-left-php` must NOT match.
pub const SIGNATURE_PATTERNS: &[&str] = &[
    ".php",
    ".asp",
    ".aspx",
    ".jsp",
    "wp-login",
    "wp-admin",
    "wp-content",
    "wp-includes",
    "xmlrpc",
    "/.env",
    "/.git",
    "/.aws",
    "/.ssh",
    "/.svn",
    "phpmyadmin",
    "/cgi-bin/",
    "/vendor/phpunit",
    "/.vscode",
    "/.idea",
];

/// True if `path` matches a known scanner-probe signature (see [`SIGNATURE_PATTERNS`]).
pub fn is_signature_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    SIGNATURE_PATTERNS.iter().any(|pat| p.contains(pat))
}

/// Whether an IP should be scored at all. Loopback / private / link-local / unspecified are
/// skipped so a dev or LAN client can't greylist itself; an unparseable IP is skipped (can't
/// reason about it). Public IPs are evaluated.
pub fn should_evaluate(ip: &str) -> bool {
    match ip.parse::<IpAddr>() {
        Ok(addr) => !(addr.is_loopback() || addr.is_unspecified() || is_private_scope(&addr)),
        Err(_) => false,
    }
}

fn is_private_scope(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            // fc00::/7 unique-local OR fe80::/10 link-local (both `is_unique_local` /
            // `is_unicast_link_local` are unstable, so test the prefixes directly).
            (seg[0] & 0xfe00) == 0xfc00 || (seg[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Per-IP features over the detection window — the input to [`score`]. Derived in Rust from
/// the SQL `(ip, path, status)` aggregates so the classifier stays single-source + testable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpFeatures {
    pub ip: String,
    pub total: i64,
    pub distinct_paths: i64,
    pub distinct_404: i64,
    pub errors: i64,
    pub signature_hits: i64,
}

/// Group raw `(ip, path, status, count)` aggregates into per-IP features, dropping IPs that
/// shouldn't be evaluated (loopback / private / unparseable) up front.
pub fn build_features(rows: &[IpPathAgg]) -> Vec<IpFeatures> {
    struct Acc {
        total: i64,
        errors: i64,
        signature_hits: i64,
        paths: HashSet<String>,
        paths_404: HashSet<String>,
    }
    let mut map: HashMap<&str, Acc> = HashMap::new();
    for r in rows {
        if !should_evaluate(&r.ip) {
            continue;
        }
        let acc = map.entry(r.ip.as_str()).or_insert_with(|| Acc {
            total: 0,
            errors: 0,
            signature_hits: 0,
            paths: HashSet::new(),
            paths_404: HashSet::new(),
        });
        acc.total += r.count;
        if r.status >= 400 {
            acc.errors += r.count;
            if is_signature_path(&r.path) {
                acc.signature_hits += r.count;
            }
        }
        if r.status == 404 {
            acc.paths_404.insert(r.path.clone());
        }
        acc.paths.insert(r.path.clone());
    }
    map.into_iter()
        .map(|(ip, a)| IpFeatures {
            ip: ip.to_string(),
            total: a.total,
            distinct_paths: a.paths.len() as i64,
            distinct_404: a.paths_404.len() as i64,
            errors: a.errors,
            signature_hits: a.signature_hits,
        })
        .collect()
}

// --- Thresholds (TUNED against a 56-day / 147k-request prod snapshot, 2026-07-05) ---------
//
// Finding: R1 is the workhorse (760 IPs, ZERO false positives — see below). R2/R3 are ~99%
// redundant with it (565 of 570 R2-trippers also trip R1) so they're tuned CONSERVATIVELY
// HIGH — backstops for the rare UA-spoofing scanner that avoids signature paths, set clear of
// the operator's own footprint. Assumes the sweep evaluates a ~24h window (see the sweep).

/// R1 — signature probe. This many signature-path error hits (4xx/5xx) trips it. The
/// WORKHORSE: in the snapshot no signature pattern ever matched a served (`status < 400`)
/// path, so this rule has zero false-positive surface — every match is a probe
/// (`/cgi-bin/luci`, `/vendor/phpunit/.../eval-stdin.php`, `/.env`, …). 2 rules out a single
/// stray referred link; even ≥1 would be defensible on this data.
pub const R1_SIGNATURE_MIN: i64 = 2;

/// R2 — distinct-404 burst over the sweep window. A backstop for a UA-spoofing scraper that
/// walks dead paths WITHOUT tripping R1. Tuned HIGH: the operator's own home IP carried 20
/// distinct 404s over 56 days, so 40 clears it with margin while every real scanner sits in
/// the hundreds-to-thousands. FCrDNS still exempts verified crawlers (CX.3).
pub const R2_DISTINCT_404_MIN: i64 = 40;

/// R3 — flood: total requests from one IP over the sweep window. The blunt backstop for
/// high-volume abuse that's neither signature- nor 404-shaped; nearly redundant with R1 (the
/// sole 24h ≥600 IP in the snapshot also tripped R1). Tuned HIGH: the operator's busiest real
/// day was ~366 requests, so 1000 sits well above any human while genuine floods run 1000s+.
/// Verified crawlers exempt.
pub const R3_FLOOD_MIN: i64 = 1000;

/// Which rule tripped — carried on the verdict so the sweep knows whether the verified-crawler
/// exemption applies and so the evidence names the rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    SignatureProbe,
    Distinct404Burst,
    Flood,
}

impl Rule {
    /// Whether an FCrDNS-verified search crawler is EXEMPT from this rule. R1 never exempts
    /// (nothing legitimate probes signature paths); the blunt rules do (a real crawler can
    /// trip a 404 burst or look like volume after a restructure).
    pub fn exempts_verified_crawlers(self) -> bool {
        !matches!(self, Rule::SignatureProbe)
    }

    pub fn label(self) -> &'static str {
        match self {
            Rule::SignatureProbe => "R1: signature probe",
            Rule::Distinct404Burst => "R2: 404 burst",
            Rule::Flood => "R3: flood",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Clear,
    Greylist {
        rule: Rule,
        reason: String,
        evidence: String,
    },
}

/// Score one IP's features. First matching rule wins, most-confident first (R1 > R2 > R3).
/// Pure + UA/DNS-blind: the verified-crawler exemption for R2/R3 is applied by the caller
/// (the sweep) via [`Rule::exempts_verified_crawlers`], never here.
pub fn score(f: &IpFeatures) -> Verdict {
    let rule = if f.signature_hits >= R1_SIGNATURE_MIN {
        Some(Rule::SignatureProbe)
    } else if f.distinct_404 >= R2_DISTINCT_404_MIN {
        Some(Rule::Distinct404Burst)
    } else if f.total >= R3_FLOOD_MIN {
        Some(Rule::Flood)
    } else {
        None
    };
    match rule {
        Some(rule) => Verdict::Greylist {
            rule,
            reason: rule.label().to_string(),
            evidence: format!(
                "signature_hits={} distinct_404={} errors={} total={} distinct_paths={}",
                f.signature_hits, f.distinct_404, f.errors, f.total, f.distinct_paths
            ),
        },
        None => Verdict::Clear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agg(ip: &str, path: &str, status: i64, count: i64) -> IpPathAgg {
        IpPathAgg {
            ip: ip.into(),
            path: path.into(),
            status,
            count,
        }
    }

    /// Only the trip-relevant fields; the rest zeroed. Tests reference the threshold CONSTS
    /// (not literals) so tuning the numbers never breaks the edge assertions.
    fn features(signature_hits: i64, distinct_404: i64, total: i64) -> IpFeatures {
        IpFeatures {
            ip: "203.0.113.7".into(),
            total,
            distinct_paths: 0,
            distinct_404,
            errors: 0,
            signature_hits,
        }
    }

    fn greylist_rule(v: &Verdict) -> Rule {
        match v {
            Verdict::Greylist { rule, .. } => *rule,
            Verdict::Clear => panic!("expected a greylist verdict, got Clear"),
        }
    }

    #[test]
    fn signature_classifier_matches_probes_not_content() {
        assert!(is_signature_path("/wp-login.php"));
        assert!(is_signature_path("/index.PHP")); // case-insensitive
        assert!(is_signature_path("/.env"));
        assert!(is_signature_path("/vendor/phpunit/phpunit/src/Util/PHP/eval-stdin.php"));
        assert!(is_signature_path("/wp-content/uploads/x"));
        // Legitimate content that merely CONTAINS "php"/"asp"/"git" as letters must not match.
        assert!(!is_signature_path("/blog/why-i-left-php"));
        assert!(!is_signature_path("/pages/projects/recon-gen"));
        assert!(!is_signature_path("/blog/asparagus-notes"));
        assert!(!is_signature_path("/"));
    }

    #[test]
    fn should_evaluate_skips_private_loopback_and_garbage() {
        assert!(should_evaluate("203.0.113.7"));
        assert!(should_evaluate("2606:4700:4700::1111"));
        assert!(!should_evaluate("127.0.0.1"));
        assert!(!should_evaluate("10.1.2.3"));
        assert!(!should_evaluate("192.168.1.5"));
        assert!(!should_evaluate("172.16.0.1"));
        assert!(!should_evaluate("169.254.1.1")); // link-local
        assert!(!should_evaluate("0.0.0.0"));
        assert!(!should_evaluate("::1"));
        assert!(!should_evaluate("fe80::1"));
        assert!(!should_evaluate("fc00::1"));
        assert!(!should_evaluate("not-an-ip"));
    }

    #[test]
    fn build_features_groups_counts_and_drops_private() {
        let rows = vec![
            agg("203.0.113.7", "/", 200, 3),
            agg("203.0.113.7", "/wp-login.php", 404, 2),
            agg("203.0.113.7", "/.env", 403, 1),
            agg("203.0.113.7", "/missing-a", 404, 1),
            agg("203.0.113.7", "/missing-b", 404, 1),
            agg("10.0.0.1", "/wp-login.php", 404, 50), // private -> dropped
        ];
        let mut f = build_features(&rows);
        assert_eq!(f.len(), 1, "private IP dropped entirely");
        let x = f.remove(0);
        assert_eq!(x.ip, "203.0.113.7");
        assert_eq!(x.total, 8);
        assert_eq!(x.errors, 5); // 2 + 1 + 1 + 1
        assert_eq!(x.distinct_paths, 5);
        assert_eq!(x.distinct_404, 3); // wp-login.php, missing-a, missing-b (.env was a 403)
        assert_eq!(x.signature_hits, 3); // wp-login.php(2) + .env(1), both >= 400
    }

    #[test]
    fn r1_signature_fires_at_threshold_not_below() {
        assert_eq!(score(&features(R1_SIGNATURE_MIN - 1, 0, 0)), Verdict::Clear);
        assert_eq!(
            greylist_rule(&score(&features(R1_SIGNATURE_MIN, 0, 0))),
            Rule::SignatureProbe
        );
    }

    #[test]
    fn r2_404_burst_fires_at_threshold_not_below() {
        assert_eq!(score(&features(0, R2_DISTINCT_404_MIN - 1, 0)), Verdict::Clear);
        assert_eq!(
            greylist_rule(&score(&features(0, R2_DISTINCT_404_MIN, 0))),
            Rule::Distinct404Burst
        );
    }

    #[test]
    fn r3_flood_fires_at_threshold_not_below() {
        assert_eq!(score(&features(0, 0, R3_FLOOD_MIN - 1)), Verdict::Clear);
        assert_eq!(
            greylist_rule(&score(&features(0, 0, R3_FLOOD_MIN))),
            Rule::Flood
        );
    }

    #[test]
    fn rule_precedence_is_r1_then_r2_then_r3() {
        // Tripping all three attributes to the most-confident rule.
        assert_eq!(
            greylist_rule(&score(&features(
                R1_SIGNATURE_MIN,
                R2_DISTINCT_404_MIN,
                R3_FLOOD_MIN
            ))),
            Rule::SignatureProbe
        );
        assert_eq!(
            greylist_rule(&score(&features(0, R2_DISTINCT_404_MIN, R3_FLOOD_MIN))),
            Rule::Distinct404Burst
        );
    }

    #[test]
    fn crawler_exemption_only_for_blunt_rules() {
        assert!(!Rule::SignatureProbe.exempts_verified_crawlers());
        assert!(Rule::Distinct404Burst.exempts_verified_crawlers());
        assert!(Rule::Flood.exempts_verified_crawlers());
    }

    #[test]
    fn quiet_ip_is_cleared() {
        assert_eq!(
            score(&features(
                R1_SIGNATURE_MIN - 1,
                R2_DISTINCT_404_MIN - 1,
                R3_FLOOD_MIN - 1
            )),
            Verdict::Clear
        );
    }
}
