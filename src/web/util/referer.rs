//! Referer normalization + smart grouping (CQ.5). The shipped `count_by_referer`
//! grouped by the FULL referer URL and self-filtered with `NOT LIKE '%hotchkiss.io%'`
//! — which polluted the list with IP-literal referers (`http://45.33.x.x/`), split one
//! real referrer across path/query variants, AND wrongly swallowed a spoofed
//! `hotchkiss.io.evil.com` (it contains the substring). This replaces that with a pure
//! host-based classifier on the `url` crate: `url::Host::{Ipv4,Ipv6}` IS the free,
//! spec-correct IP-literal test, hosts group by a registrable-ish key (no `psl` dep),
//! and the junk is COUNTED (not silently dropped) so the dashboard can say "N hidden".
//!
//! Referrers are spoofable / often stripped — everything here is DIRECTIONAL.

use crate::db::dao::request_log::RefererCount;
use url::Url;

/// Coarse bucket for an external referrer. Everything unmatched is `Referral`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefererCategory {
    Search,
    Social,
    Aggregator,
    Referral,
}

impl RefererCategory {
    pub fn as_label(self) -> &'static str {
        match self {
            RefererCategory::Search => "Search",
            RefererCategory::Social => "Social",
            RefererCategory::Aggregator => "Aggregator",
            RefererCategory::Referral => "Referral",
        }
    }
}

/// The classification of one referer URL string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefererClass {
    /// Empty/whitespace value (the NULL case is counted separately as "direct").
    Direct,
    /// Unparseable, or a URL with no host (`mailto:`, `data:`, `about:`).
    Malformed,
    /// Host is an IP literal (v4 dotted-quad or v6) — the pollution chris flagged.
    IpLiteral,
    /// Host is the site's own registrable host (site, `www.`, `beta.`) — internal nav.
    Internal,
    /// A real external referrer, grouped by host key + bucketed by category.
    External {
        host: String,
        category: RefererCategory,
    },
}

/// (host substring, category) — checked in order, first match wins. Amend HERE as new
/// referrers show up (the same amend-here pattern as `count_by_content_path`'s
/// exclusion set). Everything unmatched is `Referral` (a plain external link).
const CATEGORY_RULES: &[(&str, RefererCategory)] = &[
    ("news.ycombinator.com", RefererCategory::Aggregator),
    ("reddit.com", RefererCategory::Aggregator),
    ("lobste.rs", RefererCategory::Aggregator),
    ("slashdot.org", RefererCategory::Aggregator),
    ("google.", RefererCategory::Search),
    ("bing.com", RefererCategory::Search),
    ("duckduckgo.com", RefererCategory::Search),
    ("yahoo.com", RefererCategory::Search),
    ("yandex.", RefererCategory::Search),
    ("ecosia.org", RefererCategory::Search),
    ("baidu.com", RefererCategory::Search),
    ("t.co", RefererCategory::Social),
    ("twitter.com", RefererCategory::Social),
    ("x.com", RefererCategory::Social),
    ("facebook.com", RefererCategory::Social),
    ("instagram.com", RefererCategory::Social),
    ("linkedin.com", RefererCategory::Social),
    ("youtube.com", RefererCategory::Social),
    ("bsky.app", RefererCategory::Social),
    ("mastodon.", RefererCategory::Social),
];

/// Lowercase the host + strip a single leading `www.` / `m.` / `amp.` so
/// `www.example.com`, `m.example.com`, `example.com` group as one key. No `psl` dep —
/// this deliberately doesn't collapse to the eTLD+1 (a `foo.example.com` stays
/// distinct), which is the honest behavior for a personal-site referrer list.
fn host_key(host: &str) -> String {
    let h = host.to_ascii_lowercase();
    for p in ["www.", "m.", "amp."] {
        if let Some(rest) = h.strip_prefix(p) {
            return rest.to_string();
        }
    }
    h
}

/// True if `key` (already `host_key`-normalized) is the site itself or its `beta.`
/// subdomain. `www.` is already stripped by `host_key`, so `www.site` matches `site`.
/// Crucially an EXACT match, so `hotchkiss.io.evil.com` / `myhotchkiss.io` are NOT
/// internal (the substring-`LIKE` bug this replaces).
fn is_internal(key: &str, site_host: &str) -> bool {
    let site = site_host.to_ascii_lowercase();
    key == site || key == format!("beta.{site}")
}

fn categorize(key: &str) -> RefererCategory {
    for (pat, cat) in CATEGORY_RULES {
        if key.contains(pat) {
            return *cat;
        }
    }
    RefererCategory::Referral
}

/// Classify one raw referer string against the site's own host.
pub fn normalize_referer(raw: &str, site_host: &str) -> RefererClass {
    let raw = raw.trim();
    if raw.is_empty() {
        return RefererClass::Direct;
    }
    let Ok(url) = Url::parse(raw) else {
        return RefererClass::Malformed;
    };
    match url.host() {
        // IP-literal referers (v4 AND bracketed v6) are pure pollution.
        Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)) => RefererClass::IpLiteral,
        Some(url::Host::Domain(d)) => {
            let key = host_key(d);
            if is_internal(&key, site_host) {
                RefererClass::Internal
            } else {
                let category = categorize(&key);
                RefererClass::External {
                    host: key,
                    category,
                }
            }
        }
        // No host: mailto:, data:, about:, etc.
        None => RefererClass::Malformed,
    }
}

/// One external referrer host + its category + hit count.
#[derive(Clone, Debug)]
pub struct ExternalReferer {
    pub host: String,
    pub category: RefererCategory,
    pub count: i64,
}

/// A category + its total, for the summary chip row.
#[derive(Clone, Debug)]
pub struct CategoryCount {
    pub label: &'static str,
    pub count: i64,
}

/// The folded referrer picture for the dashboard.
#[derive(Clone, Debug)]
pub struct GroupedReferers {
    /// External referrers, host-grouped, sorted by count desc, truncated to 25.
    pub top_external: Vec<ExternalReferer>,
    /// Category totals (fixed order, only non-zero) — a chip, not a panel.
    pub by_category: Vec<CategoryCount>,
    /// IP-literal + malformed referers — the pollution, counted not shown.
    pub noise_count: i64,
    /// Requests with NO referer (direct / stripped). Passed in from a NULL count.
    pub direct_count: i64,
}

const TOP_EXTERNAL_LIMIT: usize = 25;

/// Fold the raw distinct-referer rows into the dashboard picture (CQ.5). Internal
/// referers are dropped (not a source); IP-literal + malformed roll into `noise_count`;
/// external referers group by host key, category-bucketed, sorted, truncated.
pub fn group_referers(
    rows: &[RefererCount],
    site_host: &str,
    direct_count: i64,
) -> GroupedReferers {
    use std::collections::HashMap;

    let mut hosts: HashMap<String, ExternalReferer> = HashMap::new();
    let mut noise_count = 0i64;

    for row in rows {
        match normalize_referer(&row.referer, site_host) {
            RefererClass::External { host, category } => {
                let e = hosts.entry(host.clone()).or_insert(ExternalReferer {
                    host,
                    category,
                    count: 0,
                });
                e.count += row.count;
            }
            RefererClass::IpLiteral | RefererClass::Malformed | RefererClass::Direct => {
                noise_count += row.count;
            }
            RefererClass::Internal => {} // internal nav — not a source, not noise
        }
    }

    let mut top_external: Vec<ExternalReferer> = hosts.into_values().collect();
    // Stable, deterministic ordering: count desc, then host asc.
    top_external.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.host.cmp(&b.host)));

    // Category totals in a fixed display order, only the non-zero ones.
    let by_category = [
        RefererCategory::Search,
        RefererCategory::Social,
        RefererCategory::Aggregator,
        RefererCategory::Referral,
    ]
    .into_iter()
    .filter_map(|cat| {
        let count: i64 = top_external
            .iter()
            .filter(|e| e.category == cat)
            .map(|e| e.count)
            .sum();
        (count > 0).then_some(CategoryCount {
            label: cat.as_label(),
            count,
        })
    })
    .collect();

    top_external.truncate(TOP_EXTERNAL_LIMIT);

    GroupedReferers {
        top_external,
        by_category,
        noise_count,
        direct_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "hotchkiss.io";

    fn ext(host: &str) -> RefererClass {
        RefererClass::External {
            host: host.to_string(),
            category: categorize(host),
        }
    }

    #[test]
    fn ip_literals_are_quarantined() {
        assert_eq!(normalize_referer("http://45.33.12.9/", SITE), RefererClass::IpLiteral);
        assert_eq!(normalize_referer("https://127.0.0.1:8080/x", SITE), RefererClass::IpLiteral);
        assert_eq!(normalize_referer("http://[2001:db8::1]/", SITE), RefererClass::IpLiteral);
    }

    #[test]
    fn malformed_and_direct() {
        assert_eq!(normalize_referer("", SITE), RefererClass::Direct);
        assert_eq!(normalize_referer("   ", SITE), RefererClass::Direct);
        assert_eq!(normalize_referer("not a url", SITE), RefererClass::Malformed);
        assert_eq!(normalize_referer("mailto:me@example.com", SITE), RefererClass::Malformed);
        assert_eq!(normalize_referer("data:text/html,hi", SITE), RefererClass::Malformed);
    }

    #[test]
    fn internal_is_recognized_www_and_beta() {
        assert_eq!(normalize_referer("https://hotchkiss.io/blog", SITE), RefererClass::Internal);
        assert_eq!(normalize_referer("https://www.hotchkiss.io/x", SITE), RefererClass::Internal);
        assert_eq!(normalize_referer("https://beta.hotchkiss.io/y", SITE), RefererClass::Internal);
    }

    #[test]
    fn the_like_bug_is_fixed() {
        // The old `NOT LIKE '%hotchkiss.io%'` wrongly swallowed these as "internal".
        assert_eq!(normalize_referer("https://hotchkiss.io.evil.com/", SITE), ext("hotchkiss.io.evil.com"));
        assert_eq!(normalize_referer("https://myhotchkiss.io/", SITE), ext("myhotchkiss.io"));
    }

    #[test]
    fn host_grouping_and_categories() {
        // www/m stripped → one key; path/query variants collapse.
        assert_eq!(
            normalize_referer("https://www.google.com/search?q=x", SITE),
            RefererClass::External { host: "google.com".to_string(), category: RefererCategory::Search }
        );
        assert_eq!(
            normalize_referer("https://news.ycombinator.com/item?id=1", SITE),
            RefererClass::External { host: "news.ycombinator.com".to_string(), category: RefererCategory::Aggregator }
        );
        assert_eq!(
            normalize_referer("https://t.co/abc", SITE),
            RefererClass::External { host: "t.co".to_string(), category: RefererCategory::Social }
        );
        assert_eq!(
            normalize_referer("https://example.org/a", SITE),
            RefererClass::External { host: "example.org".to_string(), category: RefererCategory::Referral }
        );
    }

    #[test]
    fn group_folds_external_and_counts_noise() {
        // Mirrors the retired `referer_external_only` DAO test, now on the pure fn:
        // self-host is Internal (dropped), the external one survives + aggregates.
        let rows = vec![
            RefererCount { referer: "https://news.ycombinator.com/".to_string(), count: 2 },
            RefererCount { referer: "https://news.ycombinator.com/item?id=9".to_string(), count: 3 }, // same host
            RefererCount { referer: "https://hotchkiss.io/blog".to_string(), count: 5 },              // internal → dropped
            RefererCount { referer: "http://45.33.1.1/".to_string(), count: 4 },                      // IP-literal → noise
            RefererCount { referer: "mailto:x@y.com".to_string(), count: 1 },                         // malformed → noise
        ];
        let g = group_referers(&rows, SITE, 10);
        assert_eq!(g.top_external.len(), 1, "one external host after grouping");
        assert_eq!(g.top_external[0].host, "news.ycombinator.com");
        assert_eq!(g.top_external[0].count, 5, "path variants collapse to one host");
        assert_eq!(g.top_external[0].category, RefererCategory::Aggregator);
        assert_eq!(g.noise_count, 5, "IP-literal (4) + mailto (1)");
        assert_eq!(g.direct_count, 10);
        assert_eq!(g.by_category.len(), 1);
        assert_eq!(g.by_category[0].label, "Aggregator");
        assert_eq!(g.by_category[0].count, 5);
    }
}
