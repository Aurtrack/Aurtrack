//! 职责：
//! - 把命令注册表暴露成 MCP tools
//! - 通过 stdio 处理 JSON-RPC
//!
//! 禁止：
//! - 在 MCP 层写业务逻辑
//! - 绕过 `aurtrack-session`

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::{self, BufRead, Write};

use aurtrack_command::{CommandRequest, command_id, find, registry, tool_name};
use aurtrack_session::Session;
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2024-11-05";

fn main() {
    let mut session = Session::default();
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&mut session, &line) {
            if writeln!(stdout, "{response}").is_err() {
                break;
            }
            if stdout.flush().is_err() {
                break;
            }
        }
    }
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn handle_line(session: &mut Session, line: &str) -> Option<String> {
    let request: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return Some(rpc_error(
                Value::Null,
                -32700,
                &format!("JSON 解析失败：{error}"),
            ));
        },
    };
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str)?;
    // 通知没有 id，不需要响应。
    let id = id?;

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "aurtrack",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": "Aurtrack DAW 的控制接口。先调用 describe 查看能力，再建工程。"
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_list() })),
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        "tools/call" => call_tool(
            session,
            request.get("params").cloned().unwrap_or_else(empty_object),
        ),
        other => Err((-32601, format!("未知方法：{other}"))),
    };

    Some(match result {
        Ok(value) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": value
        })
        .to_string(),
        Err((code, message)) => rpc_error(id, code, &message),
    })
}

fn tool_list() -> Vec<Value> {
    registry()
        .iter()
        .map(|spec| {
            json!({
                "name": tool_name(spec.id),
                "description": spec.summary,
                "inputSchema": (spec.input_schema)(),
            })
        })
        .collect()
}

fn call_tool(session: &mut Session, params: Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call 缺少 name".to_string()))?;
    let cmd = command_id(name);
    if find(&cmd).is_none() {
        return Err((-32602, format!("未知工具：{name}")));
    }
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(empty_object);

    match session.execute(&CommandRequest { cmd, args }) {
        Ok(response) => {
            let text =
                serde_json::to_string(&response).map_err(|error| (-32603, error.to_string()))?;
            Ok(json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false
            }))
        },
        Err(error) => {
            let text = json!({
                "ok": false,
                "error": { "code": error.code, "message": error.message }
            })
            .to_string();
            Ok(json!({
                "content": [{ "type": "text", "text": text }],
                "isError": true
            }))
        },
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_and_list_seven_tools() {
        let mut session = Session::default();
        let init = handle_line(
            &mut session,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .unwrap();
        assert!(init.contains("protocolVersion"));

        let list = handle_line(
            &mut session,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&list).unwrap();
        assert_eq!(value["result"]["tools"].as_array().unwrap().len(), 7);
    }

    #[test]
    fn call_tool_creates_project() {
        let mut session = Session::default();
        let call = handle_line(
            &mut session,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"project_create","arguments":{"name":"mcp-demo"}}}"#,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&call).unwrap();
        assert_eq!(value["result"]["isError"], false);
        let text = value["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("mcp-demo"));
    }

    #[test]
    fn unknown_method_returns_error() {
        let mut session = Session::default();
        let out = handle_line(&mut session, r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#).unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["error"]["code"], -32601);
    }
}
