//! JSON-RPC 2.0 framing and the MCP handshake pieces that are not tools.

use serde_json::{json, Value};

/// Protocol versions this server will echo back.
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub const PARSE_ERROR: i64 = -32700;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// The version to speak: the client's if supported, else the newest.
pub fn negotiate(requested: Option<&str>) -> &'static str {
    SUPPORTED_VERSIONS
        .iter()
        .find(|v| Some(**v) == requested)
        .copied()
        .unwrap_or(SUPPORTED_VERSIONS[0])
}

/// A success response line.
pub fn result(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// An error response line.
pub fn error(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// The `initialize` result.
pub fn initialize_result(requested: Option<&str>) -> Value {
    json!({
        "protocolVersion": negotiate(requested),
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "velo-mcp", "version": env!("CARGO_PKG_VERSION")},
    })
}
