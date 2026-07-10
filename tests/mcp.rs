//! DI.1 spike coverage — the MCP server mounted at `/mcp` (rmcp streamable-http,
//! stateless + JSON) round-trips a JSON-RPC `initialize` / `tools/list` /
//! `tools/call` over a real Admin `hio_…` Bearer key through the whole middleware
//! stack, and an unauthenticated POST is gated (403) by `require_admin_for_mutations`.
//! This is the functional half of the build-vs-buy verdict (the h2 host-validation
//! half is settled in the design doc from rmcp's source).

use hotchkiss_io::test_support::spawn_test_server;
use serde_json::{Value, json};

/// POST a JSON-RPC body to `/mcp`, optionally with a Bearer key. Sends the
/// spec-required `Accept: application/json, text/event-stream`.
async fn post_mcp(
    client: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
    body: Value,
) -> reqwest::Response {
    let mut req = client
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .json(&body);
    if let Some(key) = bearer {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    req.send().await.expect("request sent")
}

#[tokio::test]
async fn mcp_round_trips_initialize_list_and_call_over_bearer() {
    let server = spawn_test_server().await.expect("test server");
    let key = server
        .seed_admin_api_key("mcp-spike")
        .await
        .expect("admin api key");
    let client = reqwest::Client::new();
    let url = server.url("/mcp");

    // initialize — the handshake, proves the transport + auth through the full stack.
    let init = post_mcp(
        &client,
        &url,
        Some(&key),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "di-spike", "version": "0" }
            }
        }),
    )
    .await;
    assert_eq!(
        init.status(),
        200,
        "initialize must 200 for an Admin Bearer key"
    );
    let init_body = init.text().await.unwrap();
    assert!(
        init_body.contains("capabilities") && init_body.contains("protocolVersion"),
        "initialize result should carry capabilities + protocolVersion: {init_body}"
    );

    // tools/list — the ping tool must be advertised (schemars-derived schema).
    let list = post_mcp(
        &client,
        &url,
        Some(&key),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(list.status(), 200, "tools/list must 200");
    let list_body = list.text().await.unwrap();
    assert!(
        list_body.contains("ping"),
        "tools/list should include the ping tool: {list_body}"
    );

    // tools/call ping — the message is echoed back, proving args in + result out
    // + AppState reachable (site_host).
    let call = post_mcp(
        &client,
        &url,
        Some(&key),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "ping", "arguments": { "message": "di-spike-ping" } }
        }),
    )
    .await;
    assert_eq!(call.status(), 200, "tools/call ping must 200");
    let call_body = call.text().await.unwrap();
    assert!(
        call_body.contains("di-spike-ping"),
        "tools/call ping should echo the message: {call_body}"
    );
}

#[tokio::test]
async fn mcp_rejects_unauthenticated_post() {
    let server = spawn_test_server().await.expect("test server");
    let client = reqwest::Client::new();

    let resp = post_mcp(
        &client,
        &server.url("/mcp"),
        None,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(
        resp.status(),
        403,
        "an unauthenticated POST /mcp must be gated by require_admin_for_mutations"
    );
}
