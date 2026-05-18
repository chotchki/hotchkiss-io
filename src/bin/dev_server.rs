//! Long-running standalone dev server for interactive iteration on phone /
//! iOS Simulator. Wraps `test_support::spawn_test_server`: fresh SQLite per
//! run, plain HTTP, bound to `0.0.0.0` so a sim / phone on the same wifi can
//! hit it via the LAN URL. The first user to hit `/login` becomes Admin.
//!
//! Trade-off vs. the full Phase 11.3 dev-HTTPS setup: no PWA install (needs
//! HTTPS), no persisted DB across restarts. Fine for editor-facelift
//! iteration; switch to the dev-HTTPS recipe when PWA testing matters.
//!
//! Run with: `cargo run --bin dev_server`. Ctrl-C shuts it down.

use anyhow::Result;
use hotchkiss_io::test_support::spawn_test_server;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let server = spawn_test_server().await?;
    println!();
    println!("=== hotchkiss-io dev server ===");
    println!("  local : {}", server.url(""));
    match server.lan_url("") {
        Ok(lan) => println!("  LAN   : {lan}    ← point your iOS Simulator / phone here"),
        Err(e) => println!("  LAN   : <unavailable: {e}>"),
    }
    println!();
    println!("Fresh DB. First user to hit /login becomes Admin.");
    println!("Ctrl-C to shut down.");
    println!();

    tokio::signal::ctrl_c().await?;
    println!();
    println!("shutting down");
    drop(server);
    Ok(())
}
