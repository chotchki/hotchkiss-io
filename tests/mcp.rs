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

/// Call a tool by name and return the raw JSON-RPC response body (asserts 200).
async fn tool_call(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    name: &str,
    arguments: Value,
) -> String {
    let r = post_mcp(
        client,
        url,
        Some(key),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }),
    )
    .await;
    assert_eq!(r.status(), 200, "tool call {name} should 200");
    r.text().await.unwrap()
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

/// DI.5: the read tools apply the caller's visibility gate via the shared
/// `is_visible_to`. An Admin viewer is gate-exempt, so it SEES a Family-gated page —
/// which also proves the `api_key_auth`-injected identity carries through rmcp into
/// the tool (a broken derivation → Anonymous → the gated page would be hidden).
#[tokio::test]
async fn read_tools_honor_the_gate_as_the_admin_viewer() {
    let server = spawn_test_server().await.expect("test server");
    let key = server
        .seed_admin_api_key("mcp-read")
        .await
        .expect("admin key");
    server.seed_blog_post("public-post", "# Public").await.unwrap();
    server
        .seed_blog_post("gated-post", "# Secret Words")
        .await
        .unwrap();
    sqlx::query("UPDATE content_pages SET min_role = 'Family' WHERE page_name = 'gated-post'")
        .execute(&server.pool)
        .await
        .unwrap();

    let client = reqwest::Client::new();
    let url = server.url("/mcp");

    let body = tool_call(&client, &url, &key, "list_pages", json!({ "parent_path": "blog" })).await;
    assert!(body.contains("public-post"), "public post listed: {body}");
    assert!(
        body.contains("gated-post"),
        "an Admin viewer sees the gated page — proves the identity carries through: {body}"
    );

    let body = tool_call(&client, &url, &key, "get_page", json!({ "path": "blog/gated-post" })).await;
    assert!(body.contains("Secret Words"), "get_page returns the content: {body}");
}
