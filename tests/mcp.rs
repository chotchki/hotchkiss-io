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

/// A valid but NON-admin API key can't reach a tool call — the one global authz
/// path (require_admin_for_mutations) gates every POST to Admin, so a Registered
/// key resolves to a Registered identity and 403s. Mutations get permission checks.
#[tokio::test]
async fn mcp_rejects_a_non_admin_key() {
    let server = spawn_test_server().await.expect("test server");
    let key = server
        .seed_registered_api_key("mcp-registered")
        .await
        .expect("registered key");
    let client = reqwest::Client::new();

    let resp = post_mcp(
        &client,
        &server.url("/mcp"),
        Some(&key),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(
        resp.status(),
        403,
        "a valid non-admin key must be gated by require_admin_for_mutations"
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

/// DI.6: the write tools round-trip create → get → partial-update → delete through the
/// shared PageWrite service. A partial update must NOT wipe unmentioned fields (the gate).
#[tokio::test]
async fn write_tools_create_update_delete_a_page() {
    let server = spawn_test_server().await.expect("test server");
    let key = server
        .seed_admin_api_key("mcp-write")
        .await
        .expect("admin key");
    let client = reqwest::Client::new();
    let url = server.url("/mcp");

    // Create a blog post with content + a Family gate in one call.
    let body = tool_call(
        &client,
        &url,
        &key,
        "create_page",
        json!({
            "parent_path": "blog",
            "title": "Agent Post",
            "markdown": "# Agent Post\n\nfrom the agent",
            "min_role": "Family"
        }),
    )
    .await;
    assert!(body.contains("blog/agent-post"), "create returns the path: {body}");

    let body = tool_call(&client, &url, &key, "get_page", json!({ "path": "blog/agent-post" })).await;
    assert!(body.contains("from the agent"), "content set: {body}");
    assert!(body.contains("Family"), "gate set: {body}");

    // PARTIAL update: change only the markdown — the Family gate must persist.
    tool_call(
        &client,
        &url,
        &key,
        "update_page",
        json!({ "path": "blog/agent-post", "markdown": "# Agent Post\n\nEDITED" }),
    )
    .await;
    let body = tool_call(&client, &url, &key, "get_page", json!({ "path": "blog/agent-post" })).await;
    assert!(body.contains("EDITED"), "markdown updated: {body}");
    assert!(body.contains("Family"), "the gate PERSISTED across a partial update: {body}");

    // delete without confirm is refused.
    let refused = tool_call(
        &client,
        &url,
        &key,
        "delete_page",
        json!({ "path": "blog/agent-post", "confirm": false }),
    )
    .await;
    assert!(refused.contains("confirm"), "delete without confirm is refused: {refused}");

    // delete with confirm removes it.
    tool_call(
        &client,
        &url,
        &key,
        "delete_page",
        json!({ "path": "blog/agent-post", "confirm": true }),
    )
    .await;
    let gone = tool_call(&client, &url, &key, "get_page", json!({ "path": "blog/agent-post" })).await;
    assert!(gone.contains("not found"), "the page is gone: {gone}");
}

/// DI.7: the action tools flip scheduled + featured state, and feature is idempotent.
#[tokio::test]
async fn action_tools_schedule_and_feature_a_page() {
    let server = spawn_test_server().await.expect("test server");
    let key = server
        .seed_admin_api_key("mcp-actions")
        .await
        .expect("admin key");
    server.seed_blog_post("act-post", "# Act").await.unwrap();
    let client = reqwest::Client::new();
    let url = server.url("/mcp");

    let body = tool_call(&client, &url, &key, "unpublish_page", json!({ "path": "blog/act-post" })).await;
    assert!(body.contains("\"scheduled\":true"), "unpublish -> draft: {body}");
    let body = tool_call(&client, &url, &key, "publish_page", json!({ "path": "blog/act-post" })).await;
    assert!(body.contains("\"scheduled\":false"), "publish -> live: {body}");

    let body = tool_call(&client, &url, &key, "feature_page", json!({ "path": "blog/act-post", "featured": true })).await;
    assert!(body.contains("\"featured\":true"), "featured: {body}");
    // Idempotent — a second featured=true stays true (not a toggle).
    let body = tool_call(&client, &url, &key, "feature_page", json!({ "path": "blog/act-post", "featured": true })).await;
    assert!(body.contains("\"featured\":true"), "idempotent set: {body}");
    let body = tool_call(&client, &url, &key, "feature_page", json!({ "path": "blog/act-post", "featured": false })).await;
    assert!(body.contains("\"featured\":false"), "unfeatured: {body}");
}

/// DI.7: the media-upload recipe hands the agent a ready-to-run curl for the out-of-band lane.
#[tokio::test]
async fn media_upload_recipe_gives_a_curl() {
    let server = spawn_test_server().await.expect("test server");
    let key = server
        .seed_admin_api_key("mcp-recipe")
        .await
        .expect("admin key");
    let client = reqwest::Client::new();
    let body = tool_call(
        &client,
        &server.url("/mcp"),
        &key,
        "media_upload_recipe",
        json!({}),
    )
    .await;
    assert!(body.contains("/admin/media/upload"), "targets the upload endpoint: {body}");
    assert!(body.contains("curl"), "it's a curl: {body}");
}
