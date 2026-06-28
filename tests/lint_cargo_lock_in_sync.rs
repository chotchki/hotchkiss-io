//! Guard against the deploy footgun where a `Cargo.toml` version bump is committed
//! WITHOUT the matching `Cargo.lock` update. The mini builds with `cargo --locked`,
//! so a lagging lock aborts the build BEFORE the atomic bundle swap and silently
//! leaves beta/prod on the OLD binary — exactly how v0.0.75 (Phase CH) shipped.
//!
//! This checks the COMMITTED tree (`git show HEAD:…`), NOT the working tree: a
//! local `cargo build`/`test` silently syncs the working-tree `Cargo.lock`, so the
//! working copy always matches and would never catch the bug — only the committed
//! copy is honest. Skips gracefully where git/HEAD isn't available (this never runs
//! on the mini's `.git`-less deploy tree — the mini builds, it doesn't test).

use std::process::Command;

#[test]
fn committed_cargo_lock_version_matches_cargo_toml() {
    let (Some(toml), Some(lock)) = (git_show("HEAD:Cargo.toml"), git_show("HEAD:Cargo.lock"))
    else {
        eprintln!("skipping: `git show HEAD:Cargo.{{toml,lock}}` unavailable (no git / no HEAD)");
        return;
    };
    let toml_version = package_version(&toml).expect("[package] version in Cargo.toml");
    let lock_version =
        lock_version_of(&lock, "hotchkiss-io").expect("hotchkiss-io entry in Cargo.lock");
    assert_eq!(
        lock_version, toml_version,
        "COMMITTED Cargo.lock has hotchkiss-io {lock_version} but Cargo.toml is {toml_version}. \
         A version bump didn't carry the lock — `git add Cargo.lock` and commit it, or the mini's \
         `build.sh --locked` aborts before the bundle swap and leaves beta/prod on the OLD binary \
         (memory: cargo-lock --locked deploy footgun)."
    );
}

/// `git show <rev:path>` → file contents, or `None` if git or the object is absent.
fn git_show(obj: &str) -> Option<String> {
    let out = Command::new("git").args(["show", obj]).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).ok())
        .flatten()
}

/// The `version` value of the `[package]` table in a Cargo.toml.
fn package_version(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_package = t == "[package]";
            continue;
        }
        if in_package && t.strip_prefix("version").is_some_and(|r| r.trim_start().starts_with('='))
        {
            return t.split('"').nth(1).map(str::to_string);
        }
    }
    None
}

/// The `version` of the `[[package]]` entry named `name` in a Cargo.lock.
fn lock_version_of(lock: &str, name: &str) -> Option<String> {
    let needle = format!("name = \"{name}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == needle {
            for next in lines.by_ref() {
                let t = next.trim();
                if let Some(v) = t.strip_prefix("version = \"") {
                    return Some(v.trim_end_matches('"').to_string());
                }
                if t == "[[package]]" {
                    break; // ran past this package without a version (shouldn't happen)
                }
            }
            return None;
        }
    }
    None
}

#[test]
fn parsers_extract_the_right_versions() {
    let toml = "[package]\nname = \"hotchkiss-io\"\ndescription = \"x\"\nversion = \"1.2.3\"\n\n[dependencies]\nfoo = { version = \"9.9.9\" }\n";
    assert_eq!(package_version(toml).as_deref(), Some("1.2.3"));

    let lock = "[[package]]\nname = \"other\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"hotchkiss-io\"\nversion = \"4.5.6\"\ndependencies = []\n";
    assert_eq!(lock_version_of(lock, "hotchkiss-io").as_deref(), Some("4.5.6"));
    assert_eq!(lock_version_of(lock, "absent"), None);
}
