//! 职责：
//! - 定义命令注册表、请求/响应类型与错误模型
//! - 为 CLI、会话协议和 MCP 提供同一份命令元数据
//!
//! 禁止：
//! - 依赖 IO、音频设备或 UI
//! - 在这里执行命令（执行属于 `aurtrack-session`）

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value
)]

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 会话协议版本，与 MCP 的 `protocolVersion` 无关。
pub const PROTOCOL_VERSION: &str = "0.1";

fn empty_args() -> Value {
    Value::Object(serde_json::Map::new())
}

/// 一条待执行的命令。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    /// 命令 ID，例如 `track.create`。
    pub cmd: String,
    /// 命令参数；缺省为空对象。
    #[serde(default = "empty_args")]
    pub args: Value,
}

/// 成功响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResponse {
    pub ok: bool,
    pub data: Value,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_token: Option<String>,
}

impl CommandResponse {
    pub fn ok(data: Value) -> Self {
        Self {
            ok: true,
            data,
            warnings: Vec::new(),
            undo_token: None,
        }
    }

    #[must_use]
    pub fn with_undo(mut self, token: String) -> Self {
        self.undo_token = Some(token);
        self
    }
}

/// 错误响应，供 CLI 与 MCP 复用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub ok: bool,
    pub error: CommandError,
}

impl From<CommandError> for ErrorResponse {
    fn from(error: CommandError) -> Self {
        Self { ok: false, error }
    }
}

/// 命令错误。`code` 是稳定标识，`message` 给人看。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl CommandError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn invalid_args(message: impl Into<String>) -> Self {
        Self::new("invalid_args", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn unknown_command(cmd: &str) -> Self {
        Self::new("unknown_command", format!("未知命令：{cmd}"))
    }

    pub fn nothing_to_undo() -> Self {
        Self::new("nothing_to_undo", "没有可撤销的操作")
    }

    pub fn nothing_to_redo() -> Self {
        Self::new("nothing_to_redo", "没有可重做的操作")
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

/// 撤销语义。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoKind {
    /// 执行前保存工程快照。
    Snapshot,
    /// 不产生快照。
    None,
}

/// 命令元数据。
pub struct CommandSpec {
    pub id: &'static str,
    pub summary: &'static str,
    pub undo: UndoKind,
    pub input_schema: fn() -> Value,
}

impl fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandSpec")
            .field("id", &self.id)
            .field("summary", &self.summary)
            .field("undo", &self.undo)
            .finish_non_exhaustive()
    }
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn project_create_schema() -> Value {
    object_schema(
        json!({
            "name": { "type": "string", "description": "工程名，不能为空" },
            "sample_rate": { "type": "integer", "minimum": 8000, "maximum": 192_000 }
        }),
        &[],
    )
}

fn track_create_schema() -> Value {
    object_schema(
        json!({
            "name": { "type": "string", "description": "轨道名，不能为空" },
            "source": { "type": "string", "enum": ["silence", "sine"], "default": "silence" },
            "frequency": { "type": "number", "minimum": 20, "maximum": 20000, "default": 440 }
        }),
        &["name"],
    )
}

fn device_add_schema() -> Value {
    object_schema(
        json!({
            "track_id": { "type": "string" },
            "kind": { "type": "string", "enum": ["gain"] },
            "params": {
                "type": "object",
                "properties": {
                    "gain": { "type": "number", "minimum": 0, "maximum": 4 }
                },
                "additionalProperties": false
            }
        }),
        &["track_id", "kind"],
    )
}

fn param_set_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "/tracks/{id}/devices/{id}/params/{name}" },
            "value": { "type": "number" }
        }),
        &["path", "value"],
    )
}

fn render_segment_schema() -> Value {
    object_schema(
        json!({
            "start_seconds": { "type": "number", "minimum": 0, "default": 0 },
            "duration_seconds": { "type": "number", "minimum": 0.01, "maximum": 600, "default": 1 },
            "path": { "type": "string", "default": "aurtrack-render.wav" }
        }),
        &[],
    )
}

fn describe_schema() -> Value {
    object_schema(json!({}), &[])
}

fn undo_schema() -> Value {
    object_schema(
        json!({
            "steps": { "type": "integer", "minimum": 1, "maximum": 100, "default": 1 }
        }),
        &[],
    )
}

fn redo_schema() -> Value {
    undo_schema()
}

fn project_state_schema() -> Value {
    object_schema(json!({}), &[])
}

fn project_path_schema() -> Value {
    object_schema(
        json!({
            "path": {
                "type": "string",
                "description": "工程文件路径，建议使用 .aurtrack.json"
            }
        }),
        &["path"],
    )
}

fn project_save_schema() -> Value {
    project_path_schema()
}

fn project_open_schema() -> Value {
    project_path_schema()
}

fn track_remove_schema() -> Value {
    object_schema(json!({ "track_id": { "type": "string" } }), &["track_id"])
}

fn clip_create_schema() -> Value {
    object_schema(
        json!({
            "track_id": { "type": "string" },
            "name": { "type": "string", "description": "片段名，省略时自动生成" },
            "start": { "type": "number", "minimum": 0, "maximum": 5 },
            "duration": { "type": "number", "exclusiveMinimum": 0, "maximum": 5 }
        }),
        &["track_id", "start", "duration"],
    )
}

fn clip_remove_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "/tracks/{id}/clips/{id}" }
        }),
        &["path"],
    )
}

fn clip_update_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "/tracks/{id}/clips/{id}" },
            "name": { "type": "string" },
            "start": { "type": "number", "minimum": 0, "maximum": 5 },
            "duration": { "type": "number", "exclusiveMinimum": 0, "maximum": 5 }
        }),
        &["path"],
    )
}

fn clip_duplicate_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "/tracks/{id}/clips/{id}" }
        }),
        &["path"],
    )
}

fn note_duplicate_schema() -> Value {
    object_schema(
        json!({
            "path": {
                "type": "string",
                "description": "/tracks/{id}/clips/{id}/notes/{id}"
            }
        }),
        &["path"],
    )
}

fn note_quantize_schema() -> Value {
    object_schema(
        json!({
            "path": {
                "type": "string",
                "description": "/tracks/{id}/clips/{id}/notes/{id}"
            },
            "grid": {
                "type": "number",
                "exclusiveMinimum": 0,
                "maximum": 5,
                "description": "量化网格，单位秒，例如 0.125 表示 1/16"
            }
        }),
        &["path", "grid"],
    )
}

fn note_paste_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "/tracks/{id}/clips/{id}" },
            "notes": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "pitch": { "type": "integer", "minimum": 0, "maximum": 127 },
                        "start": { "type": "number", "minimum": 0 },
                        "duration": { "type": "number", "exclusiveMinimum": 0 },
                        "velocity": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.8 }
                    },
                    "required": ["pitch", "start", "duration"]
                }
            }
        }),
        &["path", "notes"],
    )
}

fn clip_paste_schema() -> Value {
    object_schema(
        json!({
            "track_id": { "type": "string" },
            "name": { "type": "string" },
            "start": { "type": "number", "minimum": 0, "maximum": 5 },
            "duration": { "type": "number", "exclusiveMinimum": 0, "maximum": 5 },
            "notes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "pitch": { "type": "integer", "minimum": 0, "maximum": 127 },
                        "start": { "type": "number", "minimum": 0 },
                        "duration": { "type": "number", "exclusiveMinimum": 0 },
                        "velocity": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.8 }
                    },
                    "required": ["pitch", "start", "duration"]
                }
            }
        }),
        &["track_id", "start", "duration", "notes"],
    )
}

fn note_add_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "/tracks/{id}/clips/{id}" },
            "pitch": { "type": "integer", "minimum": 0, "maximum": 127 },
            "start": { "type": "number", "minimum": 0 },
            "duration": { "type": "number", "exclusiveMinimum": 0 },
            "velocity": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.8 }
        }),
        &["path", "pitch", "start", "duration"],
    )
}

fn note_remove_schema() -> Value {
    object_schema(
        json!({
            "path": {
                "type": "string",
                "description": "/tracks/{id}/clips/{id}/notes/{id}"
            }
        }),
        &["path"],
    )
}

fn note_update_schema() -> Value {
    object_schema(
        json!({
            "path": {
                "type": "string",
                "description": "/tracks/{id}/clips/{id}/notes/{id}"
            },
            "pitch": { "type": "integer", "minimum": 0, "maximum": 127 },
            "start": { "type": "number", "minimum": 0 },
            "duration": { "type": "number", "exclusiveMinimum": 0 },
            "velocity": { "type": "number", "minimum": 0, "maximum": 1 }
        }),
        &["path"],
    )
}

fn device_remove_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "/tracks/{id}/devices/{id}" }
        }),
        &["path"],
    )
}

fn transport_play_schema() -> Value {
    object_schema(json!({}), &[])
}

fn transport_stop_schema() -> Value {
    object_schema(json!({}), &[])
}

fn transport_seek_schema() -> Value {
    object_schema(
        json!({
            "seconds": { "type": "number", "minimum": 0, "description": "目标位置，单位秒" }
        }),
        &["seconds"],
    )
}

fn transport_state_schema() -> Value {
    object_schema(json!({}), &[])
}

fn transport_gain_schema() -> Value {
    object_schema(
        json!({
            "gain": { "type": "number", "minimum": 0, "maximum": 4, "description": "实时音量倍数" }
        }),
        &["gain"],
    )
}

static COMMANDS: [CommandSpec; 29] = [
    CommandSpec {
        id: "project.create",
        summary: "创建或替换当前工程",
        undo: UndoKind::Snapshot,
        input_schema: project_create_schema,
    },
    CommandSpec {
        id: "project.state",
        summary: "返回完整工程状态",
        undo: UndoKind::None,
        input_schema: project_state_schema,
    },
    CommandSpec {
        id: "project.save",
        summary: "把当前工程保存到文件",
        undo: UndoKind::None,
        input_schema: project_save_schema,
    },
    CommandSpec {
        id: "project.open",
        summary: "从文件打开工程",
        undo: UndoKind::Snapshot,
        input_schema: project_open_schema,
    },
    CommandSpec {
        id: "track.create",
        summary: "新建一条轨道",
        undo: UndoKind::Snapshot,
        input_schema: track_create_schema,
    },
    CommandSpec {
        id: "track.remove",
        summary: "删除一条轨道",
        undo: UndoKind::Snapshot,
        input_schema: track_remove_schema,
    },
    CommandSpec {
        id: "clip.create",
        summary: "在轨道上新建片段",
        undo: UndoKind::Snapshot,
        input_schema: clip_create_schema,
    },
    CommandSpec {
        id: "clip.update",
        summary: "修改片段名称、起点或长度",
        undo: UndoKind::Snapshot,
        input_schema: clip_update_schema,
    },
    CommandSpec {
        id: "clip.remove",
        summary: "删除一个片段",
        undo: UndoKind::Snapshot,
        input_schema: clip_remove_schema,
    },
    CommandSpec {
        id: "clip.duplicate",
        summary: "在原片段后面复制一个片段",
        undo: UndoKind::Snapshot,
        input_schema: clip_duplicate_schema,
    },
    CommandSpec {
        id: "note.add",
        summary: "在片段里添加音符",
        undo: UndoKind::Snapshot,
        input_schema: note_add_schema,
    },
    CommandSpec {
        id: "note.update",
        summary: "修改音符音高、位置、时值或力度",
        undo: UndoKind::Snapshot,
        input_schema: note_update_schema,
    },
    CommandSpec {
        id: "note.remove",
        summary: "删除一个音符",
        undo: UndoKind::Snapshot,
        input_schema: note_remove_schema,
    },
    CommandSpec {
        id: "note.duplicate",
        summary: "在原音符后面复制一个音符",
        undo: UndoKind::Snapshot,
        input_schema: note_duplicate_schema,
    },
    CommandSpec {
        id: "note.quantize",
        summary: "把音符起点量化到指定网格",
        undo: UndoKind::Snapshot,
        input_schema: note_quantize_schema,
    },
    CommandSpec {
        id: "note.paste",
        summary: "批量粘贴音符到片段",
        undo: UndoKind::Snapshot,
        input_schema: note_paste_schema,
    },
    CommandSpec {
        id: "clip.paste",
        summary: "批量粘贴片段及音符到轨道",
        undo: UndoKind::Snapshot,
        input_schema: clip_paste_schema,
    },
    CommandSpec {
        id: "device.add",
        summary: "向轨道添加一个设备",
        undo: UndoKind::Snapshot,
        input_schema: device_add_schema,
    },
    CommandSpec {
        id: "device.remove",
        summary: "从轨道删除一个设备",
        undo: UndoKind::Snapshot,
        input_schema: device_remove_schema,
    },
    CommandSpec {
        id: "param.set",
        summary: "按路径设置参数",
        undo: UndoKind::Snapshot,
        input_schema: param_set_schema,
    },
    CommandSpec {
        id: "render.segment",
        summary: "渲染一段音频并返回分析",
        undo: UndoKind::None,
        input_schema: render_segment_schema,
    },
    CommandSpec {
        id: "transport.play",
        summary: "开始实时播放",
        undo: UndoKind::None,
        input_schema: transport_play_schema,
    },
    CommandSpec {
        id: "transport.stop",
        summary: "停止实时播放",
        undo: UndoKind::None,
        input_schema: transport_stop_schema,
    },
    CommandSpec {
        id: "transport.seek",
        summary: "定位播放头",
        undo: UndoKind::None,
        input_schema: transport_seek_schema,
    },
    CommandSpec {
        id: "transport.state",
        summary: "查询走带状态",
        undo: UndoKind::None,
        input_schema: transport_state_schema,
    },
    CommandSpec {
        id: "transport.gain",
        summary: "设置实时音量",
        undo: UndoKind::None,
        input_schema: transport_gain_schema,
    },
    CommandSpec {
        id: "describe",
        summary: "返回命令清单与工程摘要",
        undo: UndoKind::None,
        input_schema: describe_schema,
    },
    CommandSpec {
        id: "undo",
        summary: "撤销最近的可撤销命令",
        undo: UndoKind::None,
        input_schema: undo_schema,
    },
    CommandSpec {
        id: "redo",
        summary: "重做最近撤销的命令",
        undo: UndoKind::None,
        input_schema: redo_schema,
    },
];

/// 全部命令的元数据。
pub fn registry() -> &'static [CommandSpec] {
    &COMMANDS
}

/// 按命令 ID 查找元数据。
pub fn find(id: &str) -> Option<&'static CommandSpec> {
    registry().iter().find(|spec| spec.id == id)
}

/// 命令 ID 转 MCP 工具名：`track.create` -> `track_create`。
pub fn tool_name(id: &str) -> String {
    id.replace('.', "_")
}

/// MCP 工具名转命令 ID：`track_create` -> `track.create`。
pub fn command_id(tool: &str) -> String {
    tool.replace('_', ".")
}
