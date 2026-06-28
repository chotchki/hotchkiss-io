//! House lint: no test may carry the `ignore` attribute.
//!
//! Ignored tests are skipped silently by `cargo test` AND by CI, so they rot —
//! exactly how three browser e2e tests drifted out of sync with the UI (hamburger
//! nav, moved authoring form, restyled 403) without anyone noticing until a manual
//! `--ignored` run. The rule: a test that shouldn't always run gets a Cargo
//! *feature* gate (visible in `Cargo.toml`, runnable on demand, exercised on the
//! mini) — never a silent ignore.
//!
//! `no_test_carries_the_ignore_attribute` scans the source tree and fails if the
//! attribute reappears. `detection_catches_attributes_but_not_mentions` guards the
//! lint ITSELF — a permanent test that the detection still bites (catches real
//! attributes) and doesn't over-reach (spares mentions in comments/strings), so a
//! future edit can't silently neuter it into an always-pass no-op.

use std::fs;
use std::path::Path;

#[test]
fn no_test_carries_the_ignore_attribute() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    for dir in ["src", "tests"] {
        scan(&root.join(dir), &mut offenders);
    }
    assert!(
        offenders.is_empty(),
        "the test `ignore` attribute is banned — it rots silently. Feature-gate \
         the test instead (see Cargo.toml `[features]`). Found at:\n  {}",
        offenders.join("\n  ")
    );
}

/// Core detection: the 1-based line numbers in `src` that carry a leading
/// `#[ignore` attribute. A mention inside a comment or string (not line-leading)
/// is deliberately spared. Pure + total so it can be unit-tested directly.
fn ignore_attribute_lines(src: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with("#[ignore"))
        .map(|(i, _)| i + 1)
        .collect()
}

fn scan(dir: &Path, offenders: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, offenders);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let Ok(src) = fs::read_to_string(&path) else {
                continue;
            };
            for line in ignore_attribute_lines(&src) {
                offenders.push(format!("{}:{}", path.display(), line));
            }
        }
    }
}

#[test]
fn detection_catches_attributes_but_not_mentions() {
    // Real attribute forms ARE caught — bare, with args, and indented.
    assert_eq!(ignore_attribute_lines("#[ignore]\nfn a() {}"), vec![1]);
    assert_eq!(
        ignore_attribute_lines("#[test]\n    #[ignore = \"flaky\"]\nfn a() {}"),
        vec![2]
    );
    // Mentions that are NOT a line-leading attribute are spared, so this very
    // file (which talks about the attribute) doesn't flag itself.
    assert!(
        ignore_attribute_lines("// avoid #[ignore] here\nfn a() {}").is_empty(),
        "a comment mention must not be flagged"
    );
    assert!(
        ignore_attribute_lines("let needle = \"#[ignore\";").is_empty(),
        "a string-literal mention must not be flagged"
    );
    // Clean source → nothing. (Guards against an always-fail regression too.)
    assert!(ignore_attribute_lines("#[test]\nfn a() {}").is_empty());
}
