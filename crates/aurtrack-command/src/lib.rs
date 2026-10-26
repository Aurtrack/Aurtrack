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
            "sample_rate": { "type": "integer", "minimum": 8000, "maximum": 192_000 },
            "tempo_bpm": { "type": "number", "minimum": 20, "maximum": 300, "default": 120 },
            "time_signature": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "numerator": { "type": "integer", "minimum": 1, "maximum": 32 },
                    "denominator": { "type": "integer", "enum": [1, 2, 4, 8, 16, 32] }
                },
                "required": ["numerator", "denominator"]
            },
            "length_bars": { "type": "integer", "minimum": 1, "maximum": 512, "default": 4 }
        }),
        &[],
    )
}

fn project_update_schema() -> Value {
    object_schema(
        json!({
            "tempo_bpm": { "type": "number", "minimum": 20, "maximum": 300 },
            "time_signature": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "numerator": { "type": "integer", "minimum": 1, "maximum": 32 },
                    "denominator": { "type": "integer", "enum": [1, 2, 4, 8, 16, 32] }
                },
                "required": ["numerator", "denominator"]
            },
            "length_bars": { "type": "integer", "minimum": 1, "maximum": 512 },
            "loop_start": { "type": "number", "minimum": 0, "description": "循环起点，单位秒" },
            "loop_end": { "type": "number", "minimum": 0, "description": "循环终点，单位秒" }
        }),
        &[],
    )
}

fn track_create_schema() -> Value {
    object_schema(
        json!({
            "name": { "type": "string", "description": "轨道名，不能为空" },
            "source": { "type": "string", "enum": ["silence", "sine", "piano"], "default": "silence" },
            "frequency": { "type": "number", "minimum": 20, "maximum": 20000, "default": 440 }
        }),
        &["name"],
    )
}

fn group_voice_schema() -> Value {
    object_schema(
        json!({
            "group_id": { "type": "string" },
            "voice": { "type": "object", "description": "完整 VoiceGraph" }
        }),
        &["group_id", "voice"],
    )
}

fn node_types_schema() -> Value {
    object_schema(json!({}), &[])
}

fn group_create_schema() -> Value {
    object_schema(
        json!({
            "name": { "type": "string" },
            "group_id": { "type": "string" }
        }),
        &[],
    )
}

fn node_flags_schema() -> Value {
    object_schema(
        json!({
            "track_id": { "type": "string" },
            "node_id": { "type": "string" },
            "muted": { "type": "boolean" },
            "collapsed": { "type": "boolean" }
        }),
        &["track_id", "node_id"],
    )
}

fn group_from_nodes_schema() -> Value {
    object_schema(
        json!({
            "source_group_id": { "type": "string" },
            "node_ids": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1
            },
            "name": { "type": "string" },
            "group_id": { "type": "string" }
        }),
        &["source_group_id", "node_ids"],
    )
}

fn group_ungroup_schema() -> Value {
    object_schema(
        json!({
            "group_id": { "type": "string" },
            "node_id": { "type": "string" }
        }),
        &["group_id", "node_id"],
    )
}

fn group_delete_schema() -> Value {
    object_schema(json!({ "group_id": { "type": "string" } }), &["group_id"])
}

fn group_rename_schema() -> Value {
    object_schema(
        json!({
            "group_id": { "type": "string" },
            "name": { "type": "string" }
        }),
        &["group_id", "name"],
    )
}

fn group_interface_schema() -> Value {
    object_schema(
        json!({
            "group_id": { "type": "string" },
            "interface": { "type": "object", "description": "NodeGroupInterface" }
        }),
        &["group_id", "interface"],
    )
}

fn device_add_schema() -> Value {
    object_schema(
        json!({
            "track_id": { "type": "string" },
            "kind": { "type": "string", "enum": ["gain", "amp.adsr", "filter.lowpass", "mix", "plugin"] },
            "plugin_path": { "type": "string" },
            "plugin_id": { "type": "string" },
            "plugin_state": { "type": "string" },
            "params": {
                "type": "object",
                "properties": {
                    "gain": { "type": "number", "minimum": 0, "maximum": 4 },
                    "attack": { "type": "number", "minimum": 0.001, "maximum": 10 },
                    "decay": { "type": "number", "minimum": 0, "maximum": 10 },
                    "sustain": { "type": "number", "minimum": 0, "maximum": 1 },
                    "release": { "type": "number", "minimum": 0.001, "maximum": 10 },
                    "cutoff": { "type": "number", "minimum": 20, "maximum": 20000 },
                    "resonance": { "type": "number", "minimum": 0, "maximum": 0.95 }
                },
                "additionalProperties": false
            },
            "position": {
                "type": "array",
                "items": { "type": "number" },
                "minItems": 2,
                "maxItems": 2
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
            "start": { "type": "number", "minimum": 0, "description": "片段起点，单位秒，必须在工程长度内" },
            "duration": { "type": "number", "exclusiveMinimum": 0, "description": "片段长度，单位秒" }
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
            "start": { "type": "number", "minimum": 0, "description": "片段起点，单位秒" },
            "duration": { "type": "number", "exclusiveMinimum": 0, "description": "片段长度，单位秒" }
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

fn clip_move_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "/tracks/{id}/clips/{id}" },
            "track_id": { "type": "string", "description": "目标轨道 ID" },
            "start": {
                "type": "number",
                "minimum": 0,
                "description": "新起点，单位秒；省略时保持原起点"
            }
        }),
        &["path", "track_id"],
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
                "description": "量化网格，单位秒，例如 120 BPM 下 0.125 表示 1/16"
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
            "start": { "type": "number", "minimum": 0, "description": "片段起点，单位秒" },
            "duration": { "type": "number", "exclusiveMinimum": 0, "description": "片段长度，单位秒" },
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

fn note_remove_many_schema() -> Value {
    object_schema(
        json!({
            "paths": {
                "type": "array",
                "minItems": 1,
                "maxItems": 512,
                "uniqueItems": true,
                "items": {
                    "type": "string",
                    "description": "/tracks/{id}/clips/{id}/notes/{id}"
                }
            }
        }),
        &["paths"],
    )
}

fn note_quantize_many_schema() -> Value {
    object_schema(
        json!({
            "paths": {
                "type": "array",
                "minItems": 1,
                "maxItems": 512,
                "uniqueItems": true,
                "items": {
                    "type": "string",
                    "description": "/tracks/{id}/clips/{id}/notes/{id}"
                }
            },
            "grid": {
                "type": "number",
                "exclusiveMinimum": 0,
                "description": "量化网格，单位秒，例如 120 BPM 下 0.125 表示 1/16"
            }
        }),
        &["paths", "grid"],
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
            "path": { "type": "string", "description": "/tracks/{id}/devices/{id} 或 /nodes/{id}" }
        }),
        &["path"],
    )
}

fn node_move_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "要移动的设备节点" },
            "before": { "type": "string", "description": "目标节点，移动到它之前" }
        }),
        &["path", "before"],
    )
}

fn node_connect_schema() -> Value {
    object_schema(
        json!({
            "from": { "type": "string", "description": "源节点路径" },
            "to": { "type": "string", "description": "目标节点路径" },
            "from_port": { "type": "string", "description": "输出端口，默认 out" },
            "to_port": { "type": "string", "description": "输入端口；mix 用 in1 / in2" }
        }),
        &["from", "to"],
    )
}

fn node_position_schema() -> Value {
    object_schema(
        json!({
            "positions": {
                "type": "array",
                "minItems": 1,
                "maxItems": 128,
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "x": { "type": "number" },
                        "y": { "type": "number" }
                    },
                    "required": ["path", "x", "y"],
                    "additionalProperties": false
                }
            }
        }),
        &["positions"],
    )
}

fn node_duplicate_schema() -> Value {
    object_schema(
        json!({
            "paths": {
                "type": "array",
                "minItems": 1,
                "maxItems": 128,
                "items": { "type": "string" }
            },
            "offset": {
                "type": "array",
                "items": { "type": "number" },
                "minItems": 2,
                "maxItems": 2
            }
        }),
        &["paths"],
    )
}

fn nodes_remove_schema() -> Value {
    object_schema(
        json!({
            "paths": {
                "type": "array",
                "minItems": 1,
                "maxItems": 128,
                "items": { "type": "string" }
            }
        }),
        &["paths"],
    )
}

fn plugin_scan_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "插件目录；省略时使用默认目录" }
        }),
        &[],
    )
}

fn plugin_inspect_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "CLAP 动态库路径" },
            "id": { "type": "string", "description": "插件描述符 ID" }
        }),
        &["path", "id"],
    )
}

fn project_master_plugin_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string" },
            "id": { "type": "string" },
            "state": { "type": "string", "description": "可选 base64 状态 blob" }
        }),
        &["path", "id"],
    )
}

fn project_master_plugin_off_schema() -> Value {
    object_schema(json!({}), &[])
}

fn plugin_master_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "CLAP 动态库路径" },
            "id": { "type": "string", "description": "插件描述符 ID" },
            "state": { "type": "string", "description": "可选 base64 状态 blob" }
        }),
        &["path", "id"],
    )
}

fn plugin_master_off_schema() -> Value {
    object_schema(json!({}), &[])
}

fn plugin_param_schema() -> Value {
    object_schema(
        json!({
            "id": { "type": "integer", "minimum": 0, "description": "CLAP 参数 ID" },
            "value": { "type": "number", "description": "参数 plain value" }
        }),
        &["id", "value"],
    )
}

fn plugin_state_save_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "CLAP 动态库路径" },
            "id": { "type": "string", "description": "插件描述符 ID" }
        }),
        &["path", "id"],
    )
}

fn plugin_state_load_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "CLAP 动态库路径" },
            "id": { "type": "string", "description": "插件描述符 ID" },
            "state": { "type": "string", "description": "base64 状态 blob" }
        }),
        &["path", "id", "state"],
    )
}

fn extension_list_schema() -> Value {
    object_schema(
        json!({
            "path": { "type": "string", "description": "扩展目录；省略时使用默认目录" }
        }),
        &[],
    )
}

fn extension_call_schema() -> Value {
    object_schema(
        json!({
            "extension": { "type": "string" },
            "command": { "type": "string" },
            "args": {},
            "path": { "type": "string", "description": "扩展目录；省略时使用默认目录" }
        }),
        &["extension", "command"],
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

fn transport_metronome_schema() -> Value {
    object_schema(
        json!({
            "enabled": { "type": "boolean", "description": "是否启用节拍器" }
        }),
        &["enabled"],
    )
}

static COMMANDS: [CommandSpec; 62] = [
    CommandSpec {
        id: "project.create",
        summary: "创建或替换当前工程",
        undo: UndoKind::Snapshot,
        input_schema: project_create_schema,
    },
    CommandSpec {
        id: "project.update",
        summary: "修改速度、拍号、长度或循环区间",
        undo: UndoKind::Snapshot,
        input_schema: project_update_schema,
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
        id: "project.master_plugin",
        summary: "记录工程的主输出插件与状态",
        undo: UndoKind::Snapshot,
        input_schema: project_master_plugin_schema,
    },
    CommandSpec {
        id: "project.master_plugin_off",
        summary: "清除工程的主输出插件",
        undo: UndoKind::Snapshot,
        input_schema: project_master_plugin_off_schema,
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
        id: "clip.move",
        summary: "把片段移动到另一条轨道或新起点",
        undo: UndoKind::Snapshot,
        input_schema: clip_move_schema,
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
        id: "note.remove_many",
        summary: "批量删除音符",
        undo: UndoKind::Snapshot,
        input_schema: note_remove_many_schema,
    },
    CommandSpec {
        id: "note.quantize_many",
        summary: "批量量化音符起点",
        undo: UndoKind::Snapshot,
        input_schema: note_quantize_many_schema,
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
        id: "node.types",
        summary: "列出可用节点类型、端口和参数",
        undo: UndoKind::None,
        input_schema: node_types_schema,
    },
    CommandSpec {
        id: "group.create",
        summary: "创建一个工程级节点组",
        undo: UndoKind::Snapshot,
        input_schema: group_create_schema,
    },
    CommandSpec {
        id: "group.delete",
        summary: "删除一个未被引用的节点组",
        undo: UndoKind::Snapshot,
        input_schema: group_delete_schema,
    },
    CommandSpec {
        id: "group.rename",
        summary: "重命名节点组",
        undo: UndoKind::Snapshot,
        input_schema: group_rename_schema,
    },
    CommandSpec {
        id: "group.interface",
        summary: "替换节点组接口",
        undo: UndoKind::Snapshot,
        input_schema: group_interface_schema,
    },
    CommandSpec {
        id: "group.voice",
        summary: "替换节点组的 Voice 图",
        undo: UndoKind::Snapshot,
        input_schema: group_voice_schema,
    },
    CommandSpec {
        id: "group.from_nodes",
        summary: "把选中节点打包成子节点组",
        undo: UndoKind::Snapshot,
        input_schema: group_from_nodes_schema,
    },
    CommandSpec {
        id: "group.ungroup",
        summary: "把组节点展开回父图",
        undo: UndoKind::Snapshot,
        input_schema: group_ungroup_schema,
    },
    CommandSpec {
        id: "node.flags",
        summary: "设置节点静音或折叠状态",
        undo: UndoKind::Snapshot,
        input_schema: node_flags_schema,
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
        id: "plugin.scan",
        summary: "扫描 CLAP 插件目录",
        undo: UndoKind::None,
        input_schema: plugin_scan_schema,
    },
    CommandSpec {
        id: "plugin.inspect",
        summary: "读取 CLAP 插件参数和状态能力",
        undo: UndoKind::None,
        input_schema: plugin_inspect_schema,
    },
    CommandSpec {
        id: "plugin.master",
        summary: "把 CLAP 插件挂到实时主输出",
        undo: UndoKind::None,
        input_schema: plugin_master_schema,
    },
    CommandSpec {
        id: "plugin.master_off",
        summary: "卸载实时主输出插件",
        undo: UndoKind::None,
        input_schema: plugin_master_off_schema,
    },
    CommandSpec {
        id: "plugin.param",
        summary: "向实时主输出插件写入参数",
        undo: UndoKind::None,
        input_schema: plugin_param_schema,
    },
    CommandSpec {
        id: "plugin.state_save",
        summary: "读取 CLAP 插件状态为 base64",
        undo: UndoKind::None,
        input_schema: plugin_state_save_schema,
    },
    CommandSpec {
        id: "plugin.state_load",
        summary: "从 base64 恢复 CLAP 插件状态",
        undo: UndoKind::None,
        input_schema: plugin_state_load_schema,
    },
    CommandSpec {
        id: "extension.list",
        summary: "扫描 Aurtrack 应用扩展",
        undo: UndoKind::None,
        input_schema: extension_list_schema,
    },
    CommandSpec {
        id: "extension.call",
        summary: "调用应用扩展命令",
        undo: UndoKind::None,
        input_schema: extension_call_schema,
    },
    CommandSpec {
        id: "node.add",
        summary: "向轨道节点图添加未连接的设备节点",
        undo: UndoKind::Snapshot,
        input_schema: device_add_schema,
    },
    CommandSpec {
        id: "node.remove",
        summary: "从轨道节点图删除设备节点",
        undo: UndoKind::Snapshot,
        input_schema: device_remove_schema,
    },
    CommandSpec {
        id: "node.move",
        summary: "把设备节点移动到另一个节点之前",
        undo: UndoKind::Snapshot,
        input_schema: node_move_schema,
    },
    CommandSpec {
        id: "node.connect",
        summary: "连接两个节点端口",
        undo: UndoKind::Snapshot,
        input_schema: node_connect_schema,
    },
    CommandSpec {
        id: "node.disconnect",
        summary: "断开两个节点之间的连接",
        undo: UndoKind::Snapshot,
        input_schema: node_connect_schema,
    },
    CommandSpec {
        id: "node.position",
        summary: "批量记录节点画布坐标",
        undo: UndoKind::Snapshot,
        input_schema: node_position_schema,
    },
    CommandSpec {
        id: "node.duplicate",
        summary: "批量复制选中的设备节点",
        undo: UndoKind::Snapshot,
        input_schema: node_duplicate_schema,
    },
    CommandSpec {
        id: "node.remove_many",
        summary: "批量删除设备节点",
        undo: UndoKind::Snapshot,
        input_schema: nodes_remove_schema,
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
        id: "transport.metronome",
        summary: "开关节拍器",
        undo: UndoKind::None,
        input_schema: transport_metronome_schema,
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
