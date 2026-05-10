use hotchkiss_io::real_main;

/// Built off this tutorial: https://joeymckenzie.tech/blog/templates-with-rust-axum-htmx-askama
fn main() -> anyhow::Result<()> {
    real_main()
}

// PLAN 0.6.3 deliberate-failure smoke test — invalid syntax to force a build error.
// Reverted in the immediately-following commit.
fn deploy_test_broken_build() {
    let _: () = ;
}
