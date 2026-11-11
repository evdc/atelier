//! Atelier's MCP server: how agents see the other sessions (DESIGN §11).
//!
//! Minimal MCP over streamable HTTP with plain JSON responses (no SSE): one JSON-RPC message per `POST /mcp`.
//! Served on the loopback listener only. The bearer token is minted per agent process by the supervisor and
//! maps to the session that holds it, so a tool call always knows who is asking and runs as `Actor::Agent`.
//!
//! Tools live in [`tools`] as a table; adding one is adding a row.

pub mod tools;

use std::future::Future;
use std::pin::Pin;

use atelier_core::SessionId;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::App;

/// Newest first. The client's version is echoed if we know it.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// What agents are told about themselves. Short on purpose: it rides along on every turn.
const SYSTEM_PROMPT: &str = "You are Atelier session {id} (\"{title}\"). Other sessions on this machine, in this \
    and other projects, may already know what you need: before re-deriving something, look for one that does \
    (`sessions` lists them, `read` shows what was said in one) rather than starting from scratch. `send` tells one \
    something; its reply comes back to you as a message, not as a tool result. These tools come from the \
    `atelier` MCP server.";
const MAX_PROMPT_TITLE_CHARS: usize = 80;

pub fn system_prompt(id: &SessionId, title: &str) -> String {
    let title: String = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = title.chars().take(MAX_PROMPT_TITLE_CHARS).collect();
    SYSTEM_PROMPT.replace("{id}", id.as_str()).replace("{title}", &title.replace('"', "'"))
}

/// `POST /mcp`.
pub async fn endpoint(State(app): State<App>, headers: HeaderMap, body: Bytes) -> Response {
    let token =
        headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let Some(session) = token.and_then(|t| app.supervisor.session_for_token(t)) else {
        return (StatusCode::UNAUTHORIZED, "unknown or expired token").into_response();
    };
    let Ok(req) = serde_json::from_slice::<Value>(&body) else {
        return reply(StatusCode::OK, error(Value::Null, PARSE_ERROR, "parse error"));
    };
    match handle(&app, &session, req).await {
        Some(resp) => reply(StatusCode::OK, resp),
        // A notification: nothing to say back.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn reply(status: StatusCode, body: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

/// One JSON-RPC message from `session`. `None` for notifications (no `id`).
pub async fn handle(app: &App, session: &SessionId, req: Value) -> Option<Value> {
    let Some(obj) = req.as_object() else {
        return Some(error(Value::Null, INVALID_REQUEST, "expected a single JSON-RPC request"));
    };
    let id = obj.get("id").cloned();
    let method = obj.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        // `notifications/initialized` and friends; none needs action.
        return None;
    };
    Some(match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
            ok(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "atelier", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
        }
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({ "tools": tools::TOOLS.iter().map(|t| t.listing()).collect::<Vec<_>>() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let Some(tool) = tools::TOOLS.iter().find(|t| t.name == name) else {
                return Some(error(id, INVALID_PARAMS, &format!("unknown tool `{name}`")));
            };
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(a) => a.clone(),
            };
            match (tool.run)(app, session, args).await {
                Ok(text) => ok(id, json!({ "content": [{ "type": "text", "text": text }] })),
                Err(tools::ToolError::Params(e)) => error(id, INVALID_PARAMS, &e),
                // The model can act on this ("no such session"), so it's a result, not a protocol error.
                Err(tools::ToolError::Failed(e)) => {
                    ok(id, json!({ "content": [{ "type": "text", "text": e }], "isError": true }))
                }
            }
        }
        other => error(id, METHOD_NOT_FOUND, &format!("unknown method `{other}`")),
    })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

pub(crate) type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[cfg(test)]
mod tests;
