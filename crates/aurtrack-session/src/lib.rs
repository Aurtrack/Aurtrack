//! 职责：
//! - 维护内存中的最小工程模型
//! - 执行命令、维护撤销快照
//! - 渲染 WAV 片段并返回分析结果
//!
//! 禁止：
//! - 依赖 UI 或音频设备
//! - 在渲染路径里调用外部进程

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value
)]

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use aurtrack_command::{CommandError, CommandRequest, CommandResponse, registry};
use aurtrack_engine::{
    AdsrPlan, FilterPlan, FilterSettings, GraphNodePlan, GraphOp, GraphPlan, NotePlan, PluginPlan,
    VoiceNodePlan, VoiceOp, VoicePlan, adsr_envelope, midi_to_hz, note_envelope,
};
use aurtrack_graph::{
    Graph, GraphError, Node, NodeGroup, NodeGroupInterface, NodeGroupSocketType, NodeId, NodeKind,
    PORT_IN, PortRef, VoiceGraph, VoiceNodeKind,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 每四分音符的 tick 数。
pub const TICKS_PER_QUARTER: u64 = 960;
/// 默认速度。
pub const DEFAULT_BPM: f64 = 120.0;
/// 默认工程长度。
pub const DEFAULT_LENGTH_BARS: u32 = 4;

/// 拍号，分母只允许 1、2、4、8、16、32。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

/// 最小工程模型。位置以 tick 存储，秒只在命令与 UI 边界出现。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub sample_rate: u32,
    pub tempo_bpm: f64,
    pub time_signature: TimeSignature,
    pub length_bars: u32,
    pub loop_start_tick: u64,
    pub loop_end_tick: u64,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub node_groups: BTreeMap<String, NodeGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_plugin: Option<MasterPlugin>,
}

/// 工程里记录的实时主输出插件；state 是 base64 blob。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MasterPlugin {
    pub path: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

impl Project {
    pub fn ticks_per_bar(&self) -> u64 {
        let denominator = u64::from(self.time_signature.denominator.max(1));
        TICKS_PER_QUARTER * u64::from(self.time_signature.numerator) * 4 / denominator
    }

    pub fn length_ticks(&self) -> u64 {
        self.ticks_per_bar() * u64::from(self.length_bars)
    }

    pub fn seconds_per_tick(&self) -> f64 {
        60.0 / (self.tempo_bpm.max(1.0) * TICKS_PER_QUARTER as f64)
    }

    pub fn ticks_to_seconds(&self, ticks: u64) -> f64 {
        ticks as f64 * self.seconds_per_tick()
    }

    pub fn seconds_to_ticks(&self, seconds: f64) -> u64 {
        (seconds / self.seconds_per_tick()).round() as u64
    }

    pub fn bar_seconds(&self) -> f64 {
        self.ticks_to_seconds(self.ticks_per_bar())
    }

    pub fn length_seconds(&self) -> f64 {
        self.ticks_to_seconds(self.length_ticks())
    }

    pub fn loop_start_seconds(&self) -> f64 {
        self.ticks_to_seconds(self.loop_start_tick)
    }

    pub fn loop_end_seconds(&self) -> f64 {
        self.ticks_to_seconds(self.loop_end_tick)
    }
}

/// 一条轨道。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub graph: Graph,
    pub clips: Vec<Clip>,
}

impl Track {
    pub fn source(&self) -> Source {
        let instrument = self.graph.nodes().find(|node| node.kind().is_instrument());
        match instrument.map(Node::kind) {
            Some(NodeKind::InstrumentSine) => Source::Sine {
                frequency: instrument
                    .and_then(|node| node.param("frequency"))
                    .unwrap_or(440.0),
            },
            Some(NodeKind::InstrumentPiano | NodeKind::VoiceGroup) => Source::Piano,
            _ => Source::Silence,
        }
    }

    pub fn devices(&self) -> Vec<Device> {
        self.graph
            .device_nodes()
            .unwrap_or_default()
            .into_iter()
            .map(|node| Device {
                id: node.id().to_string(),
                kind: node.kind().as_str().to_string(),
                params: node.params().clone(),
            })
            .collect()
    }

    pub fn compiled(&self) -> Result<aurtrack_graph::CompiledChain, GraphError> {
        self.graph.compile_linear()
    }

    pub fn playback_graph(
        &self,
        groups: &BTreeMap<String, NodeGroup>,
    ) -> Result<(Option<f64>, Option<aurtrack_graph::AdsrSettings>, GraphPlan), GraphError> {
        let compiled = self.graph.compile_graph()?;
        let nodes: Vec<GraphNodePlan> = compiled
            .nodes
            .iter()
            .map(|node| {
                let op = match &node.op {
                    aurtrack_graph::CompiledOp::Voice => GraphOp::Voice,
                    aurtrack_graph::CompiledOp::Filter(settings) => GraphOp::Filter(
                        FilterPlan::new(FilterSettings::new(settings.cutoff, settings.resonance)),
                    ),
                    aurtrack_graph::CompiledOp::Gain(gain) => GraphOp::Gain(*gain),
                    aurtrack_graph::CompiledOp::Mix => GraphOp::Mix,
                    aurtrack_graph::CompiledOp::Plugin => GraphOp::Plugin,
                    aurtrack_graph::CompiledOp::Output => GraphOp::Output,
                    _ => return Err(GraphError::NotConnected),
                };
                Ok(GraphNodePlan {
                    op,
                    inputs: node.inputs.clone(),
                })
            })
            .collect::<Result<_, _>>()?;
        if nodes.len() > aurtrack_engine::MAX_GRAPH_NODES {
            return Err(GraphError::TooManyNodes(nodes.len()));
        }
        let piano = if let Some(group_id) = compiled.group.as_ref() {
            let group = groups
                .get(group_id)
                .ok_or_else(|| GraphError::GroupMissing(group_id.clone()))?;
            group.validate_with_groups(groups)?;
            let mut piano = aurtrack_engine::PianoPlan::new(10, 1.2, 0.5, 0.2);
            piano.voice = Some(compile_voice(&group.graph, &group.interface, groups)?);
            Some(piano)
        } else {
            None
        };
        Ok((
            compiled.source_frequency,
            compiled.adsr,
            GraphPlan {
                nodes,
                output: compiled.output,
                plugin: compiled
                    .plugin
                    .map(|plugin| PluginPlan::new(plugin.path, plugin.id, plugin.state)),
                piano,
            },
        ))
    }
}

fn compile_voice(
    voice: &VoiceGraph,
    interface: &NodeGroupInterface,
    groups: &BTreeMap<String, NodeGroup>,
) -> Result<VoicePlan, GraphError> {
    compile_voice_inner(voice, interface, groups, &mut Vec::new(), None)
}

fn compile_voice_inner(
    voice: &VoiceGraph,
    interface: &NodeGroupInterface,
    groups: &BTreeMap<String, NodeGroup>,
    stack: &mut Vec<String>,
    input_bindings: Option<&BTreeMap<String, usize>>,
) -> Result<VoicePlan, GraphError> {
    let order = voice.topological_order()?;
    let mut indices = BTreeMap::<(NodeId, String), usize>::new();
    let mut nodes = Vec::new();
    let mut output = None;
    for id in order {
        let node = voice.node(&id).ok_or(GraphError::VoiceUnknownNode)?;
        let mut incoming: Vec<_> = voice
            .connections
            .iter()
            .filter(|connection| connection.to.node() == &id)
            .collect();
        incoming.sort_by(|left, right| left.to.port().cmp(right.to.port()));
        let inputs = incoming
            .iter()
            .map(|connection| {
                indices
                    .get(&(
                        connection.from.node().clone(),
                        connection.from.port().to_string(),
                    ))
                    .copied()
                    .ok_or(GraphError::VoiceUnknownNode)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if node.kind == VoiceNodeKind::GroupInput {
            if let Some(bindings) = input_bindings {
                for socket in &interface.inputs {
                    let index = bindings
                        .get(&socket.id)
                        .copied()
                        .ok_or(GraphError::VoiceUnknownNode)?;
                    indices.insert((id.clone(), socket.id.clone()), index);
                }
                continue;
            }
            for socket in &interface.inputs {
                let op = match (socket.ty, socket.id.as_str()) {
                    (NodeGroupSocketType::Float, "pitch") => VoiceOp::Pitch,
                    (NodeGroupSocketType::Float, "velocity") => VoiceOp::Velocity,
                    (NodeGroupSocketType::Gate, _) => VoiceOp::Gate,
                    _ => VoiceOp::Constant(0.0),
                };
                let index = nodes.len();
                nodes.push(VoiceNodePlan::new(op, Vec::new()));
                indices.insert((id.clone(), socket.id.clone()), index);
            }
            continue;
        }
        if node.kind == VoiceNodeKind::Group {
            let group_id = node.group.as_deref().ok_or(GraphError::EmptyGroupId)?;
            if stack.iter().any(|item| item == group_id) {
                return Err(GraphError::GroupCycle(group_id.to_string()));
            }
            let group = groups
                .get(group_id)
                .ok_or_else(|| GraphError::GroupMissing(group_id.to_string()))?;
            let mut bindings = BTreeMap::new();
            for connection in &incoming {
                let input = indices
                    .get(&(
                        connection.from.node().clone(),
                        connection.from.port().to_string(),
                    ))
                    .copied()
                    .ok_or(GraphError::VoiceUnknownNode)?;
                bindings.insert(connection.to.port().to_string(), input);
            }
            stack.push(group_id.to_string());
            let child = compile_voice_inner(
                &group.graph,
                &group.interface,
                groups,
                stack,
                Some(&bindings),
            )?;
            stack.pop();
            let offset = nodes.len();
            nodes.extend(child.nodes);
            let child_output = offset + child.output;
            indices.insert((id.clone(), "out".to_string()), child_output);
            continue;
        }
        let param = |name: &str, default: f64| node.params.get(name).copied().unwrap_or(default);
        let op = match node.kind {
            VoiceNodeKind::OscPartials => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let partials = (param("partials", 12.0).round() as usize).clamp(1, 16);
                VoiceOp::OscPartials {
                    partials,
                    inharmonicity: param("inharmonicity", 0.0004),
                    brightness: param("brightness", 0.55),
                    decay: param("decay", 1.4),
                }
            },
            VoiceNodeKind::EnvPercussive => VoiceOp::EnvPercussive {
                attack: param("attack", 0.004),
                decay: param("decay", 1.8),
            },
            VoiceNodeKind::NoiseBurst => VoiceOp::NoiseBurst {
                level: param("level", 0.15),
                duration: param("duration", 0.008),
            },
            VoiceNodeKind::Mix => VoiceOp::Mix,
            VoiceNodeKind::GroupOutput => {
                output = Some(nodes.len());
                VoiceOp::Output
            },
            _ => return Err(GraphError::VoiceUnknownNode),
        };
        let index = nodes.len();
        if node.kind != VoiceNodeKind::GroupOutput {
            indices.insert((id.clone(), "out".to_string()), index);
        }
        nodes.push(VoiceNodePlan::new(op, inputs));
    }
    let output = output.ok_or(GraphError::VoiceTerminals)?;
    Ok(VoicePlan::new(nodes, output))
}

/// 轨道上的一个片段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub id: String,
    pub name: String,
    pub start_tick: u64,
    pub duration_tick: u64,
    pub notes: Vec<Note>,
}

/// 片段里的一个音符。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub pitch: u8,
    pub start_tick: u64,
    pub duration_tick: u64,
    pub velocity: f64,
}

/// 轨道信号源。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Silence,
    Sine { frequency: f64 },
    Piano,
}

/// 轨道上的设备。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub kind: String,
    pub params: BTreeMap<String, f64>,
}

const PROJECT_FORMAT_VERSION: u32 = 4;
const MAX_BATCH_NOTES: usize = 512;
const MIN_BPM: f64 = 20.0;
const MAX_BPM: f64 = 300.0;
const MAX_LENGTH_BARS: u32 = 512;
const ADSR_PARAMS: [&str; 4] = ["attack", "decay", "sustain", "release"];

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    format_version: u32,
    project: Project,
}

#[derive(Debug, Clone)]
struct Snapshot {
    project: Option<Project>,
    command: String,
}

/// 命令执行会话。
#[derive(Debug, Default)]
pub struct Session {
    project: Option<Project>,
    history: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    next_track: u64,
    next_device: u64,
    next_clip: u64,
    next_note: u64,
    next_undo: u64,
}

impl Session {
    /// 只读访问当前工程，供渲染与实时播放编译计划。
    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    /// 执行一条命令。
    pub fn execute(&mut self, request: &CommandRequest) -> Result<CommandResponse, CommandError> {
        match request.cmd.as_str() {
            "project.create" => self.cmd_project_create(&request.args),
            "project.update" => self.cmd_project_update(&request.args),
            "project.state" => Ok(self.cmd_project_state()),
            "project.save" => self.cmd_project_save(&request.args),
            "project.open" => self.cmd_project_open(&request.args),
            "project.master_plugin" => self.cmd_master_plugin(&request.args),
            "project.master_plugin_off" => self.cmd_master_plugin_off(),
            "track.create" => self.cmd_track_create(&request.args),
            "track.remove" => self.cmd_track_remove(&request.args),
            "group.create" => self.cmd_group_create(&request.args),
            "group.delete" => self.cmd_group_delete(&request.args),
            "group.rename" => self.cmd_group_rename(&request.args),
            "group.interface" => self.cmd_group_interface(&request.args),
            "group.voice" => self.cmd_group_voice(&request.args),
            "clip.create" => self.cmd_clip_create(&request.args),
            "clip.update" => self.cmd_clip_update(&request.args),
            "clip.remove" => self.cmd_clip_remove(&request.args),
            "clip.duplicate" => self.cmd_clip_duplicate(&request.args),
            "clip.move" => self.cmd_clip_move(&request.args),
            "note.add" => self.cmd_note_add(&request.args),
            "note.update" => self.cmd_note_update(&request.args),
            "note.remove" => self.cmd_note_remove(&request.args),
            "note.duplicate" => self.cmd_note_duplicate(&request.args),
            "note.quantize" => self.cmd_note_quantize(&request.args),
            "note.remove_many" => self.cmd_note_remove_many(&request.args),
            "note.quantize_many" => self.cmd_note_quantize_many(&request.args),
            "note.paste" => self.cmd_note_paste(&request.args),
            "clip.paste" => self.cmd_clip_paste(&request.args),
            "device.add" => self.cmd_device_add(&request.args),
            "device.remove" => self.cmd_device_remove(&request.args),
            "node.add" => self.cmd_node_add(&request.args),
            "node.remove" => self.cmd_node_remove(&request.args),
            "node.move" => self.cmd_node_move(&request.args),
            "node.connect" => self.cmd_node_connect(&request.args),
            "node.disconnect" => self.cmd_node_disconnect(&request.args),
            "node.position" => self.cmd_node_position(&request.args),
            "node.duplicate" => self.cmd_node_duplicate(&request.args),
            "node.remove_many" => self.cmd_node_remove_many(&request.args),
            "param.set" => self.cmd_param_set(&request.args),
            "render.segment" => self.cmd_render_segment(&request.args),
            "describe" => Ok(self.cmd_describe()),
            "undo" => self.cmd_undo(&request.args),
            "redo" => self.cmd_redo(&request.args),
            other => Err(CommandError::unknown_command(other)),
        }
    }

    fn snapshot(&mut self, command: &str) -> String {
        self.redo_stack.clear();
        self.next_undo += 1;
        let number = self.next_undo;
        self.history.push(Snapshot {
            project: self.project.clone(),
            command: command.to_string(),
        });
        format!("undo-{number}")
    }

    fn cmd_project_create(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ProjectCreateArgs = parse_args(args)?;
        let name = args.name.unwrap_or_else(|| "Untitled".to_string());
        if name.trim().is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }
        let sample_rate = args.sample_rate.unwrap_or(48_000);
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(CommandError::invalid_args(
                "sample_rate 必须在 8000 到 192000 之间",
            ));
        }
        let tempo_bpm = args.tempo_bpm.unwrap_or(DEFAULT_BPM);
        validate_bpm(tempo_bpm)?;
        let time_signature = args.time_signature.unwrap_or_default();
        validate_time_signature(time_signature)?;
        let length_bars = args.length_bars.unwrap_or(DEFAULT_LENGTH_BARS);
        validate_length_bars(length_bars)?;
        let mut project = Project {
            name,
            sample_rate,
            tempo_bpm,
            time_signature,
            length_bars,
            loop_start_tick: 0,
            loop_end_tick: 0,
            tracks: Vec::new(),
            node_groups: BTreeMap::from([("piano.voice".to_string(), NodeGroup::piano_default())]),
            master_plugin: None,
        };
        project.loop_end_tick = project.length_ticks();
        let token = self.snapshot("project.create");
        self.project = Some(project);
        Ok(CommandResponse::ok(json!({ "project": self.project_summary() })).with_undo(token))
    }

    fn cmd_project_update(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ProjectUpdateArgs = parse_args(args)?;
        if args.tempo_bpm.is_none()
            && args.time_signature.is_none()
            && args.length_bars.is_none()
            && args.loop_start.is_none()
            && args.loop_end.is_none()
        {
            return Err(CommandError::invalid_args("至少提供一个字段"));
        }
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let tempo_bpm = args.tempo_bpm.unwrap_or(project.tempo_bpm);
        validate_bpm(tempo_bpm)?;
        let time_signature = args.time_signature.unwrap_or(project.time_signature);
        validate_time_signature(time_signature)?;
        let length_bars = args.length_bars.unwrap_or(project.length_bars);
        validate_length_bars(length_bars)?;

        let mut preview = project.clone();
        preview.tempo_bpm = tempo_bpm;
        preview.time_signature = time_signature;
        preview.length_bars = length_bars;
        let length_ticks = preview.length_ticks();
        if project.tracks.iter().any(|track| {
            track
                .clips
                .iter()
                .any(|clip| clip.start_tick.saturating_add(clip.duration_tick) > length_ticks)
        }) {
            return Err(CommandError::invalid_args("有片段超出新的工程长度"));
        }
        let loop_start_tick = args
            .loop_start
            .map_or(project.loop_start_tick, |value| {
                preview.seconds_to_ticks(value)
            })
            .min(length_ticks.saturating_sub(1));
        let loop_end_tick = args
            .loop_end
            .map_or(project.loop_end_tick, |value| {
                preview.seconds_to_ticks(value)
            })
            .min(length_ticks);
        let (loop_start_tick, loop_end_tick) = if loop_start_tick >= loop_end_tick {
            (0, length_ticks)
        } else {
            (loop_start_tick, loop_end_tick)
        };
        validate_loop_range(loop_start_tick, loop_end_tick, length_ticks)?;

        let token = self.snapshot("project.update");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        project.tempo_bpm = tempo_bpm;
        project.time_signature = time_signature;
        project.length_bars = length_bars;
        project.loop_start_tick = loop_start_tick;
        project.loop_end_tick = loop_end_tick;

        Ok(CommandResponse::ok(json!({ "project": self.project_summary() })).with_undo(token))
    }

    fn cmd_track_create(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: TrackCreateArgs = parse_args(args)?;
        if args.name.trim().is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }
        let sample_rate = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?
            .sample_rate;
        let (instrument, frequency) = match args.source.as_deref().unwrap_or("silence") {
            "silence" => (NodeKind::InstrumentSilence, None),
            "piano" => (NodeKind::VoiceGroup, None),
            "sine" => {
                let frequency = args.frequency.unwrap_or(440.0);
                if !frequency.is_finite() || !(20.0..=20_000.0).contains(&frequency) {
                    return Err(CommandError::invalid_args(
                        "frequency 必须在 20 到 20000 之间",
                    ));
                }
                if frequency >= f64::from(sample_rate) / 2.0 {
                    return Err(CommandError::invalid_args("frequency 必须小于采样率的一半"));
                }
                (NodeKind::InstrumentSine, Some(frequency))
            },
            other => {
                return Err(CommandError::invalid_args(format!(
                    "不支持的 source：{other}"
                )));
            },
        };
        let mut graph = if instrument == NodeKind::VoiceGroup {
            Graph::group_track("piano.voice").map_err(graph_error)?
        } else {
            Graph::empty_track(instrument).map_err(graph_error)?
        };
        if let Some(frequency) = frequency {
            graph
                .set_param(
                    &NodeId::new("instrument").map_err(graph_error)?,
                    "frequency",
                    frequency,
                )
                .map_err(graph_error)?;
        }
        let token = self.snapshot("track.create");
        self.next_track += 1;
        let number = self.next_track;
        let track_id = format!("track-{number}");
        let path = format!("/tracks/{track_id}");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        project.tracks.push(Track {
            id: track_id.clone(),
            name: args.name,
            graph,
            clips: Vec::new(),
        });
        Ok(CommandResponse::ok(json!({ "track_id": track_id, "path": path })).with_undo(token))
    }

    fn cmd_device_add(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        self.cmd_add_node(args, "device.add", "devices")
    }

    fn cmd_node_add(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        self.cmd_add_node(args, "node.add", "nodes")
    }

    fn cmd_add_node(
        &mut self,
        args: &Value,
        command: &str,
        segment: &str,
    ) -> Result<CommandResponse, CommandError> {
        let args: DeviceAddArgs = parse_args(args)?;
        let kind = match args.kind.as_str() {
            "gain" => NodeKind::Gain,
            "amp.adsr" => NodeKind::AmpAdsr,
            "filter.lowpass" => NodeKind::FilterLowpass,
            "mix" => NodeKind::Mix,
            "plugin" => NodeKind::Plugin,
            other => {
                return Err(CommandError::invalid_args(format!(
                    "不支持的 kind：{other}"
                )));
            },
        };
        let track_graph = self
            .project
            .as_ref()
            .and_then(|project| {
                project
                    .tracks
                    .iter()
                    .find(|track| track.id == args.track_id)
            })
            .map(|track| &track.graph);
        let Some(track_graph) = track_graph else {
            return Err(CommandError::not_found(format!(
                "找不到轨道：{}",
                args.track_id
            )));
        };
        if kind == NodeKind::AmpAdsr
            && track_graph
                .device_nodes()
                .map_err(graph_error)?
                .iter()
                .any(|node| node.kind() == NodeKind::AmpAdsr)
        {
            return Err(CommandError::invalid_args("一条轨道暂时只能有一个 ADSR"));
        }
        let token = self.snapshot(command);
        self.next_device += 1;
        let number = self.next_device;
        let device_id = format!("device-{number}");
        let path = format!("/tracks/{}/{segment}/{device_id}", args.track_id);
        let mut node = Node::new(NodeId::new(device_id.clone()).map_err(graph_error)?, kind);
        for (name, value) in args.params.unwrap_or_default() {
            node.set_param(&name, value).map_err(graph_error)?;
        }
        if let Some([x, y]) = args.position {
            node.set_position(x, y).map_err(graph_error)?;
        }
        if kind == NodeKind::Plugin {
            let path = args.plugin_path.unwrap_or_default();
            let id = args.plugin_id.unwrap_or_default();
            if path.trim().is_empty() || id.trim().is_empty() {
                return Err(CommandError::invalid_args(
                    "plugin 节点需要 plugin_path 和 plugin_id",
                ));
            }
            node.set_plugin(Some(aurtrack_graph::PluginConfig::new(
                path,
                id,
                args.plugin_state,
            )))
            .map_err(graph_error)?;
        }
        let free = command == "node.add";
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == args.track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{}", args.track_id)))?;
        if free {
            track.graph.add_node(node).map_err(graph_error)?;
        } else {
            track
                .graph
                .insert_before_output(node)
                .map_err(graph_error)?;
        }
        Ok(CommandResponse::ok(json!({
            "device_id": device_id,
            "node_id": device_id,
            "path": path,
        }))
        .with_undo(token))
    }

    fn cmd_param_set(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ParamSetArgs = parse_args(args)?;
        let path = args.path.clone();
        let (track_id, device_id, name) = parse_param_path(&path)?;
        let node_id = NodeId::new(device_id).map_err(graph_error)?;
        let node = self
            .project
            .as_ref()
            .and_then(|project| project.tracks.iter().find(|track| track.id == track_id))
            .and_then(|track| track.graph.node(&node_id))
            .ok_or_else(|| CommandError::not_found(format!("找不到参数：{path}")))?;
        if !node.kind().is_device() {
            return Err(CommandError::not_found(format!("找不到参数：{path}")));
        }
        match node.kind() {
            NodeKind::Gain if name == "gain" => validate_gain(args.value)?,
            NodeKind::AmpAdsr if ADSR_PARAMS.contains(&name) => {
                validate_adsr_param(name, args.value)?;
            },
            NodeKind::FilterLowpass if name == "cutoff" || name == "resonance" => {
                validate_filter_param(name, args.value)?;
            },
            _ => {
                return Err(CommandError::invalid_args(format!("不支持的参数：{name}")));
            },
        }
        let token = self.snapshot("param.set");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到参数：{path}")))?;
        track
            .graph
            .set_param(&node_id, name, args.value)
            .map_err(graph_error)?;
        Ok(CommandResponse::ok(json!({ "path": path, "value": args.value })).with_undo(token))
    }

    fn cmd_render_segment(&self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: RenderArgs = parse_args(args)?;
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let start_seconds = args.start_seconds.unwrap_or(0.0);
        if !start_seconds.is_finite() || start_seconds < 0.0 {
            return Err(CommandError::invalid_args(
                "start_seconds 必须是大于等于 0 的有限数",
            ));
        }
        let duration_seconds = args.duration_seconds.unwrap_or(1.0);
        if !duration_seconds.is_finite() || !(0.01..=600.0).contains(&duration_seconds) {
            return Err(CommandError::invalid_args(
                "duration_seconds 必须在 0.01 到 600 之间",
            ));
        }
        let path = args
            .path
            .unwrap_or_else(|| "aurtrack-render.wav".to_string());
        if path.trim().is_empty() {
            return Err(CommandError::invalid_args("path 不能为空"));
        }
        let data = render_project(project, start_seconds, duration_seconds, &path)?;
        Ok(CommandResponse::ok(data))
    }

    fn cmd_describe(&self) -> CommandResponse {
        let commands: Vec<Value> = registry()
            .iter()
            .map(|spec| {
                json!({
                    "id": spec.id,
                    "summary": spec.summary,
                    "undo": spec.undo,
                    "input_schema": (spec.input_schema)(),
                })
            })
            .collect();
        CommandResponse::ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "commands": commands,
            "project": self.project_summary(),
        }))
    }

    fn cmd_undo(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: UndoArgs = parse_args(args)?;
        let steps = args.steps.unwrap_or(1);
        if !(1..=100).contains(&steps) {
            return Err(CommandError::invalid_args("steps 必须在 1 到 100 之间"));
        }
        if self.history.is_empty() {
            return Err(CommandError::nothing_to_undo());
        }
        let mut undone = Vec::new();
        for _ in 0..steps {
            let Some(snapshot) = self.history.pop() else {
                break;
            };
            self.redo_stack.push(Snapshot {
                project: self.project.clone(),
                command: snapshot.command.clone(),
            });
            self.project = snapshot.project;
            undone.push(snapshot.command);
        }
        Ok(CommandResponse::ok(json!({ "undone": undone })))
    }

    fn cmd_project_state(&self) -> CommandResponse {
        let project = self.project.as_ref().map(project_view);
        CommandResponse::ok(json!({ "project": project }))
    }

    fn cmd_master_plugin(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct MasterPluginArgs {
            path: String,
            id: String,
            #[serde(default)]
            state: Option<String>,
        }
        let args: MasterPluginArgs = parse_args(args)?;
        if args.path.trim().is_empty() || args.id.trim().is_empty() {
            return Err(CommandError::invalid_args("path 和 id 不能为空"));
        }
        if self.project.is_none() {
            return Err(CommandError::not_found("还没有工程，先执行 project.create"));
        }
        let token = self.snapshot("project.master_plugin");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        project.master_plugin = Some(MasterPlugin {
            path: args.path,
            id: args.id,
            state: args.state,
        });
        Ok(CommandResponse::ok(json!({
            "master_plugin": project.master_plugin
        }))
        .with_undo(token))
    }

    fn cmd_master_plugin_off(&mut self) -> Result<CommandResponse, CommandError> {
        if self.project.is_none() {
            return Err(CommandError::not_found("还没有工程，先执行 project.create"));
        }
        let token = self.snapshot("project.master_plugin_off");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        project.master_plugin = None;
        Ok(CommandResponse::ok(json!({ "master_plugin": null })).with_undo(token))
    }

    fn cmd_project_save(&self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ProjectPathArgs = parse_args(args)?;
        let path = args.path.trim();
        if path.is_empty() {
            return Err(CommandError::invalid_args("path 不能为空"));
        }
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let file = ProjectFile {
            format_version: PROJECT_FORMAT_VERSION,
            project: project.clone(),
        };
        let text = serde_json::to_string_pretty(&file)
            .map_err(|error| CommandError::new("io_error", format!("序列化工程失败：{error}")))?;
        let target = Path::new(path);
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    CommandError::new("io_error", format!("创建工程目录失败：{error}"))
                })?;
            }
        }
        std::fs::write(target, text.as_bytes())
            .map_err(|error| CommandError::new("io_error", format!("写工程失败：{error}")))?;
        Ok(CommandResponse::ok(json!({
            "path": path,
            "bytes": text.len(),
        })))
    }

    fn cmd_project_open(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ProjectPathArgs = parse_args(args)?;
        let path = args.path.trim();
        if path.is_empty() {
            return Err(CommandError::invalid_args("path 不能为空"));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|error| CommandError::new("io_error", format!("读工程失败：{error}")))?;
        let value: Value = serde_json::from_str(&text).map_err(|error| {
            CommandError::invalid_args(format!("工程文件不是合法 JSON：{error}"))
        })?;
        let version = value
            .get("format_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| CommandError::invalid_args("工程文件缺少 format_version"))?;
        if version != u64::from(PROJECT_FORMAT_VERSION) {
            return Err(CommandError::invalid_args(format!(
                "不支持的工程格式版本：{version}"
            )));
        }
        let file: ProjectFile = serde_json::from_value(value).map_err(|error| {
            CommandError::invalid_args(format!("工程文件不是合法 JSON：{error}"))
        })?;
        let (next_track, next_device, next_clip, next_note) = validate_project(&file.project)?;
        let token = self.snapshot("project.open");
        self.project = Some(file.project);
        self.next_track = next_track;
        self.next_device = next_device;
        self.next_clip = next_clip;
        self.next_note = next_note;
        Ok(CommandResponse::ok(json!({
            "path": path,
            "project": self.project_summary(),
        }))
        .with_undo(token))
    }

    fn cmd_group_create(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct CreateArgs {
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            group_id: Option<String>,
        }
        let args: CreateArgs = parse_args(args)?;
        let token = self.snapshot("group.create");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let id = if let Some(id) = args.group_id {
            NodeId::new(id.clone()).map_err(graph_error)?;
            if project.node_groups.contains_key(&id) {
                return Err(CommandError::invalid_args(format!("节点组已存在：{id}")));
            }
            id
        } else {
            let mut number = project.node_groups.len() + 1;
            loop {
                let id = format!("group-{number}");
                if !project.node_groups.contains_key(&id) {
                    break id;
                }
                number += 1;
            }
        };
        let name = args
            .name
            .unwrap_or_else(|| format!("Group {id}"))
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }
        let group = NodeGroup::empty(id.clone(), name);
        group.validate().map_err(graph_error)?;
        project.node_groups.insert(id.clone(), group);
        Ok(CommandResponse::ok(json!({ "group_id": id })).with_undo(token))
    }

    fn cmd_group_delete(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct DeleteArgs {
            group_id: String,
        }
        let args: DeleteArgs = parse_args(args)?;
        let token = self.snapshot("group.delete");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let referenced = project.tracks.iter().any(|track| {
            track.graph.nodes().any(|node| {
                node.kind() == NodeKind::VoiceGroup && node.group() == Some(args.group_id.as_str())
            })
        });
        if referenced {
            return Err(CommandError::invalid_args("节点组仍被轨道引用，不能删除"));
        }
        if project.node_groups.remove(&args.group_id).is_none() {
            return Err(CommandError::not_found(format!(
                "找不到节点组：{}",
                args.group_id
            )));
        }
        Ok(CommandResponse::ok(json!({ "group_id": args.group_id })).with_undo(token))
    }

    fn cmd_group_rename(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RenameArgs {
            group_id: String,
            name: String,
        }
        let args: RenameArgs = parse_args(args)?;
        let name = args.name.trim();
        if name.is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }
        let token = self.snapshot("group.rename");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let group = project
            .node_groups
            .get_mut(&args.group_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点组：{}", args.group_id)))?;
        group.name = name.to_string();
        Ok(
            CommandResponse::ok(json!({ "group_id": args.group_id, "name": group.name }))
                .with_undo(token),
        )
    }

    fn cmd_group_interface(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct InterfaceArgs {
            group_id: String,
            interface: aurtrack_graph::NodeGroupInterface,
        }
        let args: InterfaceArgs = parse_args(args)?;
        let token = self.snapshot("group.interface");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let groups = project.node_groups.clone();
        let group = project
            .node_groups
            .get_mut(&args.group_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点组：{}", args.group_id)))?;
        let previous = group.interface.clone();
        group.interface = args.interface;
        if let Err(error) = group.validate_with_groups(&groups) {
            group.interface = previous;
            return Err(graph_error(error));
        }
        Ok(CommandResponse::ok(json!({ "group_id": args.group_id })).with_undo(token))
    }

    fn cmd_group_voice(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct VoiceArgs {
            group_id: String,
            voice: VoiceGraph,
        }
        let args: VoiceArgs = parse_args(args)?;
        let token = self.snapshot("group.voice");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let groups = project.node_groups.clone();
        let group = project
            .node_groups
            .get_mut(&args.group_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点组：{}", args.group_id)))?;
        args.voice
            .validate_with_groups(&group.interface, &groups)
            .map_err(graph_error)?;
        group.graph = args.voice;
        Ok(CommandResponse::ok(json!({
            "group_id": args.group_id,
            "voice": "updated",
        }))
        .with_undo(token))
    }

    fn cmd_track_remove(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: TrackRemoveArgs = parse_args(args)?;
        let track_id = args.track_id;
        let exists = self
            .project
            .as_ref()
            .is_some_and(|project| project.tracks.iter().any(|track| track.id == track_id));
        if !exists {
            return Err(CommandError::not_found(format!("找不到轨道：{track_id}")));
        }
        let token = self.snapshot("track.remove");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        project.tracks.retain(|track| track.id != track_id);
        Ok(CommandResponse::ok(json!({ "track_id": track_id })).with_undo(token))
    }

    fn cmd_clip_create(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ClipCreateArgs = parse_args(args)?;
        let track_id = args.track_id;
        let (start_tick, duration_tick, length_ticks) = {
            let project = self
                .project
                .as_ref()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            (
                project.seconds_to_ticks(args.start),
                project.seconds_to_ticks(args.duration),
                project.length_ticks(),
            )
        };
        validate_clip_range(start_tick, duration_tick, length_ticks)?;
        let exists = self
            .project
            .as_ref()
            .is_some_and(|project| project.tracks.iter().any(|track| track.id == track_id));
        if !exists {
            return Err(CommandError::not_found(format!("找不到轨道：{track_id}")));
        }
        let name = args
            .name
            .unwrap_or_else(|| format!("片段 {}", self.next_clip + 1));
        if name.trim().is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }

        let token = self.snapshot("clip.create");
        self.next_clip += 1;
        let clip_id = format!("clip-{}", self.next_clip);
        let clip = Clip {
            id: clip_id.clone(),
            name,
            start_tick,
            duration_tick,
            notes: Vec::new(),
        };
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
        track.clips.push(clip.clone());
        let response_clip = clip_view(project, &clip);

        Ok(CommandResponse::ok(json!({
            "track_id": track_id,
            "clip_id": clip_id,
            "path": format!("/tracks/{track_id}/clips/{clip_id}"),
            "clip": response_clip,
        }))
        .with_undo(token))
    }

    fn cmd_clip_update(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ClipUpdateArgs = parse_args(args)?;
        if args.name.is_none() && args.start.is_none() && args.duration.is_none() {
            return Err(CommandError::invalid_args("至少提供一个字段"));
        }
        let path = args.path;
        let (track_id, clip_id) = parse_clip_path(&path)?;
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let existing = project
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
            .cloned()
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        let length_ticks = project.length_ticks();

        let start_tick = args
            .start
            .map_or(existing.start_tick, |value| project.seconds_to_ticks(value));
        let duration_tick = args.duration.map_or(existing.duration_tick, |value| {
            project.seconds_to_ticks(value)
        });
        let name = args.name.unwrap_or(existing.name);
        validate_clip_range(start_tick, duration_tick, length_ticks)?;
        if name.trim().is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }
        for note in &existing.notes {
            validate_note_values(
                note.pitch,
                note.start_tick,
                note.duration_tick,
                note.velocity,
                duration_tick,
            )?;
        }

        let token = self.snapshot("clip.update");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        let clip = track
            .clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        clip.start_tick = start_tick;
        clip.duration_tick = duration_tick;
        clip.name = name;
        let updated = clip.clone();
        let updated_view = clip_view(project, &updated);

        Ok(CommandResponse::ok(json!({ "path": path, "clip": updated_view })).with_undo(token))
    }

    fn cmd_clip_remove(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ClipPathArgs = parse_args(args)?;
        let path = args.path;
        let (track_id, clip_id) = parse_clip_path(&path)?;
        let exists = self.project.as_ref().is_some_and(|project| {
            project.tracks.iter().any(|track| {
                track.id == track_id && track.clips.iter().any(|clip| clip.id == clip_id)
            })
        });
        if !exists {
            return Err(CommandError::not_found(format!("找不到片段：{path}")));
        }

        let token = self.snapshot("clip.remove");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        track.clips.retain(|clip| clip.id != clip_id);
        Ok(CommandResponse::ok(json!({ "path": path })).with_undo(token))
    }

    fn cmd_clip_duplicate(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ClipPathArgs = parse_args(args)?;
        let path = args.path;
        let (track_id, clip_id) = parse_clip_path(&path)?;
        let source = self
            .project
            .as_ref()
            .and_then(|project| project.tracks.iter().find(|track| track.id == track_id))
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
            .cloned()
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        let start_tick = source.start_tick + source.duration_tick;
        let length_ticks = self.project.as_ref().map_or(0, Project::length_ticks);
        if start_tick + source.duration_tick > length_ticks {
            return Err(CommandError::invalid_args(
                "片段后面没有空间，先缩短或移动片段",
            ));
        }

        let token = self.snapshot("clip.duplicate");
        self.next_clip += 1;
        let new_clip_id = format!("clip-{}", self.next_clip);
        let mut notes = Vec::with_capacity(source.notes.len());
        for note in &source.notes {
            self.next_note += 1;
            notes.push(Note {
                id: format!("note-{}", self.next_note),
                ..note.clone()
            });
        }
        let clip = Clip {
            id: new_clip_id.clone(),
            name: format!("{} 副本", source.name),
            start_tick,
            duration_tick: source.duration_tick,
            notes,
        };

        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        track.clips.push(clip.clone());
        let response_clip = clip_view(project, &clip);

        Ok(CommandResponse::ok(json!({
            "track_id": track_id,
            "clip_id": new_clip_id,
            "path": format!("/tracks/{track_id}/clips/{new_clip_id}"),
            "clip": response_clip,
        }))
        .with_undo(token))
    }

    fn cmd_clip_move(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ClipMoveArgs = parse_args(args)?;
        let path = args.path;
        let (track_id, clip_id) = parse_clip_path(&path)?;
        let source = self
            .project
            .as_ref()
            .and_then(|project| project.tracks.iter().find(|track| track.id == track_id))
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
            .cloned()
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        let target_exists = self
            .project
            .as_ref()
            .is_some_and(|project| project.tracks.iter().any(|track| track.id == args.track_id));
        if !target_exists {
            return Err(CommandError::not_found(format!(
                "找不到轨道：{}",
                args.track_id
            )));
        }
        let length_ticks = self.project.as_ref().map_or(0, Project::length_ticks);
        let start_tick = args.start.map_or(source.start_tick, |value| {
            self.project
                .as_ref()
                .map_or(source.start_tick, |project| project.seconds_to_ticks(value))
        });
        validate_clip_range(start_tick, source.duration_tick, length_ticks)?;
        if track_id == args.track_id && start_tick == source.start_tick {
            return Err(CommandError::invalid_args("片段没有移动"));
        }

        let source_track_index = self
            .project
            .as_ref()
            .and_then(|project| project.tracks.iter().position(|track| track.id == track_id))
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        let clip_index = self
            .project
            .as_ref()
            .and_then(|project| {
                project.tracks[source_track_index]
                    .clips
                    .iter()
                    .position(|clip| clip.id == clip_id)
            })
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        let target_track_index = self
            .project
            .as_ref()
            .and_then(|project| {
                project
                    .tracks
                    .iter()
                    .position(|track| track.id == args.track_id)
            })
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{}", args.track_id)))?;

        let token = self.snapshot("clip.move");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let clip = if source_track_index == target_track_index {
            let clip = project.tracks[source_track_index]
                .clips
                .get_mut(clip_index)
                .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
            clip.start_tick = start_tick;
            clip.clone()
        } else {
            let mut clip = project.tracks[source_track_index].clips.remove(clip_index);
            clip.start_tick = start_tick;
            project.tracks[target_track_index].clips.push(clip.clone());
            clip
        };
        let new_path = format!("/tracks/{}/clips/{clip_id}", args.track_id);
        let response_clip = clip_view(project, &clip);

        Ok(CommandResponse::ok(json!({
            "path": new_path,
            "track_id": args.track_id,
            "clip": response_clip,
        }))
        .with_undo(token))
    }

    fn cmd_note_add(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NoteAddArgs = parse_args(args)?;
        let path = args.path;
        let (track_id, clip_id) = parse_clip_path(&path)?;
        let velocity = args.velocity.unwrap_or(0.8);
        let (start_tick, duration_tick, clip_duration_tick) = {
            let project = self
                .project
                .as_ref()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            let clip = project
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
                .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
            (
                project.seconds_to_ticks(args.start),
                project.seconds_to_ticks(args.duration),
                clip.duration_tick,
            )
        };
        validate_note_values(
            args.pitch,
            start_tick,
            duration_tick,
            velocity,
            clip_duration_tick,
        )?;

        let token = self.snapshot("note.add");
        self.next_note += 1;
        let note_id = format!("note-{}", self.next_note);
        let note = Note {
            id: note_id.clone(),
            pitch: args.pitch,
            start_tick,
            duration_tick,
            velocity,
        };
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        let clip = track
            .clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        clip.notes.push(note.clone());
        let response_note = note_view(project, &note);

        Ok(CommandResponse::ok(json!({
            "path": format!("{path}/notes/{note_id}"),
            "note_id": note_id,
            "note": response_note,
        }))
        .with_undo(token))
    }

    fn cmd_note_update(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NoteUpdateArgs = parse_args(args)?;
        if args.pitch.is_none()
            && args.start.is_none()
            && args.duration.is_none()
            && args.velocity.is_none()
        {
            return Err(CommandError::invalid_args("至少提供一个字段"));
        }
        let path = args.path;
        let (track_id, clip_id, note_id) = parse_note_path(&path)?;
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let (clip_duration, existing) = project
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
            .and_then(|clip| {
                clip.notes
                    .iter()
                    .find(|note| note.id == note_id)
                    .map(|note| (clip.duration_tick, note.clone()))
            })
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;

        let pitch = args.pitch.unwrap_or(existing.pitch);
        let start_tick = args
            .start
            .map_or(existing.start_tick, |value| project.seconds_to_ticks(value));
        let duration_tick = args.duration.map_or(existing.duration_tick, |value| {
            project.seconds_to_ticks(value)
        });
        let velocity = args.velocity.unwrap_or(existing.velocity);
        validate_note_values(pitch, start_tick, duration_tick, velocity, clip_duration)?;

        let token = self.snapshot("note.update");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        let clip = track
            .clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        let note = clip
            .notes
            .iter_mut()
            .find(|note| note.id == note_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        note.pitch = pitch;
        note.start_tick = start_tick;
        note.duration_tick = duration_tick;
        note.velocity = velocity;
        let updated = note.clone();
        let response_note = note_view(project, &updated);

        Ok(CommandResponse::ok(json!({ "path": path, "note": response_note })).with_undo(token))
    }

    fn cmd_note_remove(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NotePathArgs = parse_args(args)?;
        let path = args.path;
        let (track_id, clip_id, note_id) = parse_note_path(&path)?;
        let exists = self.project.as_ref().is_some_and(|project| {
            project.tracks.iter().any(|track| {
                track.id == track_id
                    && track.clips.iter().any(|clip| {
                        clip.id == clip_id && clip.notes.iter().any(|note| note.id == note_id)
                    })
            })
        });
        if !exists {
            return Err(CommandError::not_found(format!("找不到音符：{path}")));
        }

        let token = self.snapshot("note.remove");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        let clip = track
            .clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        clip.notes.retain(|note| note.id != note_id);
        Ok(CommandResponse::ok(json!({ "path": path })).with_undo(token))
    }

    fn cmd_note_duplicate(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NotePathArgs = parse_args(args)?;
        let path = args.path;
        let (track_id, clip_id, note_id) = parse_note_path(&path)?;
        let (clip_duration, source) = self
            .project
            .as_ref()
            .and_then(|project| project.tracks.iter().find(|track| track.id == track_id))
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
            .and_then(|clip| {
                clip.notes
                    .iter()
                    .find(|note| note.id == note_id)
                    .map(|note| (clip.duration_tick, note.clone()))
            })
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        let start_tick = source.start_tick + source.duration_tick;
        if start_tick + source.duration_tick > clip_duration {
            return Err(CommandError::invalid_args(
                "音符后面没有空间，先缩短或移动音符",
            ));
        }

        let token = self.snapshot("note.duplicate");
        self.next_note += 1;
        let new_note_id = format!("note-{}", self.next_note);
        let note = Note {
            id: new_note_id.clone(),
            start_tick,
            ..source
        };
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        let clip = track
            .clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        clip.notes.push(note.clone());
        let response_note = note_view(project, &note);

        Ok(CommandResponse::ok(json!({
            "path": format!("/tracks/{track_id}/clips/{clip_id}/notes/{new_note_id}"),
            "note_id": new_note_id,
            "note": response_note,
        }))
        .with_undo(token))
    }

    fn cmd_note_quantize(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NoteQuantizeArgs = parse_args(args)?;
        let grid_tick = {
            let project = self
                .project
                .as_ref()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            if !args.grid.is_finite() || args.grid <= 0.0 || args.grid > project.length_seconds() {
                return Err(CommandError::invalid_args(
                    "grid 必须是落在工程长度内的正数",
                ));
            }
            project.seconds_to_ticks(args.grid).max(1)
        };
        let path = args.path;
        let (track_id, clip_id, note_id) = parse_note_path(&path)?;
        let (clip_duration, existing) = self
            .project
            .as_ref()
            .and_then(|project| project.tracks.iter().find(|track| track.id == track_id))
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
            .and_then(|clip| {
                clip.notes
                    .iter()
                    .find(|note| note.id == note_id)
                    .map(|note| (clip.duration_tick, note.clone()))
            })
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        let max_start = clip_duration.saturating_sub(existing.duration_tick);
        let start_tick = quantize_tick(existing.start_tick, grid_tick, max_start);
        validate_note_values(
            existing.pitch,
            start_tick,
            existing.duration_tick,
            existing.velocity,
            clip_duration,
        )?;

        let token = self.snapshot("note.quantize");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        let clip = track
            .clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        let note = clip
            .notes
            .iter_mut()
            .find(|note| note.id == note_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
        note.start_tick = start_tick;
        let updated = note.clone();
        let response_note = note_view(project, &updated);

        Ok(CommandResponse::ok(json!({
            "path": path,
            "note": response_note,
            "grid": args.grid,
        }))
        .with_undo(token))
    }

    fn cmd_note_remove_many(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NotePathsArgs = parse_args(args)?;
        if args.paths.is_empty() {
            return Err(CommandError::invalid_args("paths 不能为空"));
        }
        if args.paths.len() > MAX_BATCH_NOTES {
            return Err(CommandError::invalid_args(format!(
                "paths 一次最多 {MAX_BATCH_NOTES} 个"
            )));
        }
        let mut seen = BTreeSet::new();
        let mut targets = Vec::with_capacity(args.paths.len());
        for path in &args.paths {
            if !seen.insert(path.as_str()) {
                return Err(CommandError::invalid_args(format!(
                    "paths 不能重复：{path}"
                )));
            }
            let (track_id, clip_id, note_id) = parse_note_path(path)?;
            let exists = self.project.as_ref().is_some_and(|project| {
                project.tracks.iter().any(|track| {
                    track.id == track_id
                        && track.clips.iter().any(|clip| {
                            clip.id == clip_id && clip.notes.iter().any(|note| note.id == note_id)
                        })
                })
            });
            if !exists {
                return Err(CommandError::not_found(format!("找不到音符：{path}")));
            }
            targets.push((
                track_id.to_string(),
                clip_id.to_string(),
                note_id.to_string(),
            ));
        }

        let token = self.snapshot("note.remove_many");
        for (track_id, clip_id, note_id) in &targets {
            let project = self
                .project
                .as_mut()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            let track = project
                .tracks
                .iter_mut()
                .find(|track| &track.id == track_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
            let clip = track
                .clips
                .iter_mut()
                .find(|clip| &clip.id == clip_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到片段：{clip_id}")))?;
            clip.notes.retain(|note| &note.id != note_id);
        }

        Ok(CommandResponse::ok(json!({
            "paths": args.paths,
            "removed": targets.len(),
        }))
        .with_undo(token))
    }

    fn cmd_note_quantize_many(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NoteQuantizeManyArgs = parse_args(args)?;
        if args.paths.is_empty() {
            return Err(CommandError::invalid_args("paths 不能为空"));
        }
        if args.paths.len() > MAX_BATCH_NOTES {
            return Err(CommandError::invalid_args(format!(
                "paths 一次最多 {MAX_BATCH_NOTES} 个"
            )));
        }
        let grid_tick = {
            let project = self
                .project
                .as_ref()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            if !args.grid.is_finite() || args.grid <= 0.0 || args.grid > project.length_seconds() {
                return Err(CommandError::invalid_args(
                    "grid 必须是落在工程长度内的正数",
                ));
            }
            project.seconds_to_ticks(args.grid).max(1)
        };
        let mut seen = BTreeSet::new();
        let mut targets = Vec::with_capacity(args.paths.len());
        for path in &args.paths {
            if !seen.insert(path.as_str()) {
                return Err(CommandError::invalid_args(format!(
                    "paths 不能重复：{path}"
                )));
            }
            let (track_id, clip_id, note_id) = parse_note_path(path)?;
            let (clip_duration, existing) = self
                .project
                .as_ref()
                .and_then(|project| project.tracks.iter().find(|track| track.id == track_id))
                .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
                .and_then(|clip| {
                    clip.notes
                        .iter()
                        .find(|note| note.id == note_id)
                        .map(|note| (clip.duration_tick, note.clone()))
                })
                .ok_or_else(|| CommandError::not_found(format!("找不到音符：{path}")))?;
            let max_start = clip_duration.saturating_sub(existing.duration_tick);
            let start_tick = quantize_tick(existing.start_tick, grid_tick, max_start);
            validate_note_values(
                existing.pitch,
                start_tick,
                existing.duration_tick,
                existing.velocity,
                clip_duration,
            )?;
            targets.push((
                track_id.to_string(),
                clip_id.to_string(),
                note_id.to_string(),
                start_tick,
            ));
        }

        let token = self.snapshot("note.quantize_many");
        for (track_id, clip_id, note_id, start) in &targets {
            let project = self
                .project
                .as_mut()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            let track = project
                .tracks
                .iter_mut()
                .find(|track| &track.id == track_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
            let clip = track
                .clips
                .iter_mut()
                .find(|clip| &clip.id == clip_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到片段：{clip_id}")))?;
            let note = clip
                .notes
                .iter_mut()
                .find(|note| &note.id == note_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到音符：{note_id}")))?;
            note.start_tick = *start;
        }

        Ok(CommandResponse::ok(json!({
            "paths": args.paths,
            "grid": args.grid,
            "count": targets.len(),
        }))
        .with_undo(token))
    }

    fn cmd_note_paste(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NotePasteArgs = parse_args(args)?;
        if args.notes.is_empty() {
            return Err(CommandError::invalid_args("notes 不能为空"));
        }
        let path = args.path;
        let (track_id, clip_id) = parse_clip_path(&path)?;
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let clip_duration = project
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .and_then(|track| track.clips.iter().find(|clip| clip.id == clip_id))
            .map(|clip| clip.duration_tick)
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        let mut drafts = Vec::with_capacity(args.notes.len());
        for draft in &args.notes {
            let start_tick = project.seconds_to_ticks(draft.start);
            let duration_tick = project.seconds_to_ticks(draft.duration);
            validate_note_values(
                draft.pitch,
                start_tick,
                duration_tick,
                draft.velocity.unwrap_or(0.8),
                clip_duration,
            )?;
            drafts.push((start_tick, duration_tick));
        }

        let token = self.snapshot("note.paste");
        let mut notes = Vec::with_capacity(args.notes.len());
        for (draft, (start_tick, duration_tick)) in args.notes.into_iter().zip(drafts) {
            self.next_note += 1;
            notes.push(Note {
                id: format!("note-{}", self.next_note),
                pitch: draft.pitch,
                start_tick,
                duration_tick,
                velocity: draft.velocity.unwrap_or(0.8),
            });
        }
        let note_ids = notes.iter().map(|note| note.id.clone()).collect::<Vec<_>>();
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        let clip = track
            .clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到片段：{path}")))?;
        clip.notes.extend(notes.clone());
        let response_notes = notes
            .iter()
            .map(|note| note_view(project, note))
            .collect::<Vec<_>>();

        Ok(CommandResponse::ok(json!({
            "path": path,
            "note_ids": note_ids,
            "notes": response_notes,
        }))
        .with_undo(token))
    }

    fn cmd_clip_paste(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let ClipPasteArgs {
            track_id,
            name,
            start,
            duration,
            notes,
        } = parse_args(args)?;
        let (start_tick, duration_tick, length_ticks) = {
            let project = self
                .project
                .as_ref()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            (
                project.seconds_to_ticks(start),
                project.seconds_to_ticks(duration),
                project.length_ticks(),
            )
        };
        validate_clip_range(start_tick, duration_tick, length_ticks)?;
        let name = name.unwrap_or_else(|| format!("片段 {}", self.next_clip + 1));
        if name.trim().is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }
        let track_exists = self
            .project
            .as_ref()
            .is_some_and(|project| project.tracks.iter().any(|track| track.id == track_id));
        if !track_exists {
            return Err(CommandError::not_found(format!("找不到轨道：{track_id}")));
        }
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut drafts = Vec::with_capacity(notes.len());
        for draft in &notes {
            let note_start = project.seconds_to_ticks(draft.start);
            let note_duration = project.seconds_to_ticks(draft.duration);
            validate_note_values(
                draft.pitch,
                note_start,
                note_duration,
                draft.velocity.unwrap_or(0.8),
                duration_tick,
            )?;
            drafts.push((note_start, note_duration));
        }

        let token = self.snapshot("clip.paste");
        self.next_clip += 1;
        let clip_id = format!("clip-{}", self.next_clip);
        let notes = notes
            .into_iter()
            .zip(drafts)
            .map(|(draft, (note_start, note_duration))| {
                self.next_note += 1;
                Note {
                    id: format!("note-{}", self.next_note),
                    pitch: draft.pitch,
                    start_tick: note_start,
                    duration_tick: note_duration,
                    velocity: draft.velocity.unwrap_or(0.8),
                }
            })
            .collect::<Vec<_>>();
        let clip = Clip {
            id: clip_id.clone(),
            name,
            start_tick,
            duration_tick,
            notes,
        };
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
        track.clips.push(clip.clone());
        let response_clip = clip_view(project, &clip);

        Ok(CommandResponse::ok(json!({
            "track_id": track_id,
            "clip_id": clip_id,
            "path": format!("/tracks/{track_id}/clips/{clip_id}"),
            "clip": response_clip,
        }))
        .with_undo(token))
    }

    fn cmd_device_remove(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        self.cmd_remove_node(args, "device.remove")
    }

    fn cmd_node_remove(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        self.cmd_remove_node(args, "node.remove")
    }

    fn cmd_remove_node(
        &mut self,
        args: &Value,
        command: &str,
    ) -> Result<CommandResponse, CommandError> {
        let args: DeviceRemoveArgs = parse_args(args)?;
        let path = args.path;
        let (track_id, device_id) = parse_node_path(&path)?;
        let node_id = NodeId::new(device_id).map_err(graph_error)?;
        let node_kind = self
            .project
            .as_ref()
            .and_then(|project| project.tracks.iter().find(|track| track.id == track_id))
            .and_then(|track| track.graph.node(&node_id))
            .filter(|node| node.kind().is_device())
            .map(Node::kind);
        let Some(node_kind) = node_kind else {
            return Err(CommandError::not_found(format!("找不到设备：{path}")));
        };
        let token = self.snapshot(command);
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点：{path}")))?;
        if node_kind == NodeKind::Mix {
            track.graph.remove_node(&node_id).map_err(graph_error)?;
        } else {
            track
                .graph
                .remove_chain_node(&node_id)
                .map_err(graph_error)?;
        }
        Ok(CommandResponse::ok(json!({ "path": path })).with_undo(token))
    }

    fn cmd_node_move(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NodeMoveArgs = parse_args(args)?;
        let (track_id, node) = parse_node_path(&args.path)?;
        let (before_track, before) = parse_node_path(&args.before)?;
        if track_id != before_track {
            return Err(CommandError::invalid_args("节点必须属于同一条轨道"));
        }
        let node_id = NodeId::new(node).map_err(graph_error)?;
        let before_id = NodeId::new(before).map_err(graph_error)?;
        let track = self
            .project
            .as_ref()
            .and_then(|project| project.tracks.iter().find(|track| track.id == track_id))
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
        let node = track
            .graph
            .node(&node_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点：{}", args.path)))?;
        if !node.kind().is_device() {
            return Err(CommandError::invalid_args("只有设备节点可以移动"));
        }
        let target = track
            .graph
            .node(&before_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点：{}", args.before)))?;
        if !target.kind().has_input(PORT_IN) {
            return Err(CommandError::invalid_args("目标节点没有输入端口"));
        }
        let token = self.snapshot("node.move");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
        track
            .graph
            .move_before(&node_id, &before_id)
            .map_err(graph_error)?;
        Ok(CommandResponse::ok(json!({
            "path": args.path,
            "before": args.before,
        }))
        .with_undo(token))
    }

    fn cmd_node_connect(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NodeConnectArgs = parse_args(args)?;
        let (track_id, from_node) = parse_node_path(&args.from)?;
        let (to_track, to_node) = parse_node_path(&args.to)?;
        if track_id != to_track {
            return Err(CommandError::invalid_args("节点必须属于同一条轨道"));
        }
        let from = PortRef::new(
            NodeId::new(from_node).map_err(graph_error)?,
            args.from_port.clone(),
        );
        let to = PortRef::new(
            NodeId::new(to_node).map_err(graph_error)?,
            args.to_port.clone(),
        );
        let token = self.snapshot("node.connect");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
        track.graph.connect(from, to).map_err(graph_error)?;
        Ok(CommandResponse::ok(json!({
            "from": args.from,
            "from_port": args.from_port,
            "to": args.to,
            "to_port": args.to_port,
        }))
        .with_undo(token))
    }

    fn cmd_node_disconnect(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NodeConnectArgs = parse_args(args)?;
        let (track_id, from_node) = parse_node_path(&args.from)?;
        let (to_track, to_node) = parse_node_path(&args.to)?;
        if track_id != to_track {
            return Err(CommandError::invalid_args("节点必须属于同一条轨道"));
        }
        let from = PortRef::new(
            NodeId::new(from_node).map_err(graph_error)?,
            args.from_port.clone(),
        );
        let to = PortRef::new(
            NodeId::new(to_node).map_err(graph_error)?,
            args.to_port.clone(),
        );
        let token = self.snapshot("node.disconnect");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
        track.graph.disconnect(&from, &to).map_err(graph_error)?;
        Ok(CommandResponse::ok(json!({
            "from": args.from,
            "from_port": args.from_port,
            "to": args.to,
            "to_port": args.to_port,
        }))
        .with_undo(token))
    }

    fn cmd_node_position(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NodePositionArgs = parse_args(args)?;
        if args.positions.is_empty() {
            return Err(CommandError::invalid_args("positions 不能为空"));
        }
        if args.positions.len() > 128 {
            return Err(CommandError::invalid_args("一次最多移动 128 个节点"));
        }
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut parsed = Vec::with_capacity(args.positions.len());
        for position in &args.positions {
            let (track_id, node) = parse_node_path(&position.path)?;
            let node_id = NodeId::new(node).map_err(graph_error)?;
            let track = project
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
            if track.graph.node(&node_id).is_none() {
                return Err(CommandError::not_found(format!(
                    "找不到节点：{}",
                    position.path
                )));
            }
            parsed.push((track_id.to_string(), node_id, position.x, position.y));
        }
        let token = self.snapshot("node.position");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        for (track_id, node_id, x, y) in &parsed {
            let track = project
                .tracks
                .iter_mut()
                .find(|track| track.id == *track_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
            track
                .graph
                .set_position(node_id, *x, *y)
                .map_err(graph_error)?;
        }
        Ok(CommandResponse::ok(json!({ "positions": args.positions })).with_undo(token))
    }

    fn cmd_node_duplicate(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NodeDuplicateArgs = parse_args(args)?;
        if args.paths.is_empty() {
            return Err(CommandError::invalid_args("paths 不能为空"));
        }
        if args.paths.len() > 128 {
            return Err(CommandError::invalid_args("一次最多复制 128 个节点"));
        }
        let offset = args.offset.unwrap_or([40.0, 40.0]);
        if !offset[0].is_finite() || !offset[1].is_finite() {
            return Err(CommandError::invalid_args("offset 必须是有限数"));
        }
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut groups: BTreeMap<String, Vec<(NodeId, NodeId, String)>> = BTreeMap::new();
        for path in &args.paths {
            let (track_id, node) = parse_node_path(path)?;
            let old_id = NodeId::new(node).map_err(graph_error)?;
            let node = project
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .and_then(|track| track.graph.node(&old_id))
                .ok_or_else(|| CommandError::not_found(format!("找不到节点：{path}")))?;
            if !node.kind().is_device()
                || node.kind().is_instrument()
                || node.kind() == NodeKind::AmpAdsr
                || node.kind() == NodeKind::Output
            {
                return Err(CommandError::invalid_args("这个节点不能复制"));
            }
            let number = self.next_device + 1;
            self.next_device = number;
            let new_id = NodeId::new(format!("device-{number}")).map_err(graph_error)?;
            groups
                .entry(track_id.to_string())
                .or_default()
                .push((old_id, new_id, path.clone()));
        }
        let token = self.snapshot("node.duplicate");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut copied = Vec::new();
        for (track_id, entries) in groups {
            let track = project
                .tracks
                .iter_mut()
                .find(|track| track.id == track_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
            let old_ids: Vec<NodeId> = entries.iter().map(|entry| entry.0.clone()).collect();
            let new_ids: Vec<NodeId> = entries.iter().map(|entry| entry.1.clone()).collect();
            let mapping = track
                .graph
                .duplicate_nodes(&old_ids, &new_ids, offset)
                .map_err(graph_error)?;
            for (source, target) in mapping {
                let source_path = entries
                    .iter()
                    .find(|entry| entry.0 == source)
                    .map(|entry| entry.2.clone())
                    .unwrap_or_default();
                copied.push(json!({
                    "source": source_path,
                    "node_id": target.to_string(),
                }));
            }
        }
        Ok(CommandResponse::ok(json!({ "nodes": copied })).with_undo(token))
    }

    fn cmd_node_remove_many(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: NodesRemoveArgs = parse_args(args)?;
        if args.paths.is_empty() {
            return Err(CommandError::invalid_args("paths 不能为空"));
        }
        if args.paths.len() > 128 {
            return Err(CommandError::invalid_args("一次最多删除 128 个节点"));
        }
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut parsed = Vec::with_capacity(args.paths.len());
        for path in &args.paths {
            let (track_id, node) = parse_node_path(path)?;
            let node_id = NodeId::new(node).map_err(graph_error)?;
            let node_kind = project
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .and_then(|track| track.graph.node(&node_id))
                .map(Node::kind)
                .ok_or_else(|| CommandError::not_found(format!("找不到节点：{path}")))?;
            if !node_kind.is_device() {
                return Err(CommandError::invalid_args("只能删除设备节点"));
            }
            parsed.push((track_id.to_string(), node_id, node_kind));
        }
        let token = self.snapshot("node.remove_many");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        for (track_id, node_id, _) in parsed {
            let track = project
                .tracks
                .iter_mut()
                .find(|track| track.id == track_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
            track.graph.remove_node(&node_id).map_err(graph_error)?;
        }
        Ok(CommandResponse::ok(json!({ "paths": args.paths })).with_undo(token))
    }

    fn cmd_redo(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: UndoArgs = parse_args(args)?;
        let steps = args.steps.unwrap_or(1);
        if !(1..=100).contains(&steps) {
            return Err(CommandError::invalid_args("steps 必须在 1 到 100 之间"));
        }
        if self.redo_stack.is_empty() {
            return Err(CommandError::nothing_to_redo());
        }
        let mut redone = Vec::new();
        for _ in 0..steps {
            let Some(snapshot) = self.redo_stack.pop() else {
                break;
            };
            self.history.push(Snapshot {
                project: self.project.clone(),
                command: snapshot.command.clone(),
            });
            self.project = snapshot.project;
            redone.push(snapshot.command);
        }
        Ok(CommandResponse::ok(json!({ "redone": redone })))
    }

    fn project_summary(&self) -> Value {
        let Some(project) = &self.project else {
            return Value::Null;
        };
        json!({
            "name": &project.name,
            "sample_rate": project.sample_rate,
            "tempo_bpm": project.tempo_bpm,
            "time_signature": project.time_signature,
            "length_bars": project.length_bars,
            "length_seconds": project.length_seconds(),
            "bar_seconds": project.bar_seconds(),
            "loop_start": project.loop_start_seconds(),
            "loop_end": project.loop_end_seconds(),
            "track_count": project.tracks.len(),
            "tracks": project.tracks.iter().map(|track| json!({
                "id": &track.id,
                "name": &track.name,
                "source": track.source(),
                "device_count": track.devices().len(),
            })).collect::<Vec<_>>(),
        })
    }
}

fn project_view(project: &Project) -> Value {
    json!({
        "name": &project.name,
        "sample_rate": project.sample_rate,
        "tempo_bpm": project.tempo_bpm,
        "time_signature": project.time_signature,
        "length_bars": project.length_bars,
        "length_seconds": project.length_seconds(),
        "bar_seconds": project.bar_seconds(),
        "length_ticks": project.length_ticks(),
        "loop_start": project.loop_start_seconds(),
        "loop_end": project.loop_end_seconds(),
        "loop_start_tick": project.loop_start_tick,
        "loop_end_tick": project.loop_end_tick,
        "master_plugin": project.master_plugin,
        "node_groups": &project.node_groups,
        "tracks": project.tracks.iter().map(|track| json!({
            "id": &track.id,
            "name": &track.name,
            "source": track.source(),
            "devices": track.devices(),
            "graph": &track.graph,
            "clips": track.clips.iter().map(|clip| clip_view(project, clip)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn clip_view(project: &Project, clip: &Clip) -> Value {
    json!({
        "id": &clip.id,
        "name": &clip.name,
        "start": project.ticks_to_seconds(clip.start_tick),
        "duration": project.ticks_to_seconds(clip.duration_tick),
        "start_tick": clip.start_tick,
        "duration_tick": clip.duration_tick,
        "notes": clip.notes.iter().map(|note| note_view(project, note)).collect::<Vec<_>>(),
    })
}

fn note_view(project: &Project, note: &Note) -> Value {
    json!({
        "id": &note.id,
        "pitch": note.pitch,
        "start": project.ticks_to_seconds(note.start_tick),
        "duration": project.ticks_to_seconds(note.duration_tick),
        "start_tick": note.start_tick,
        "duration_tick": note.duration_tick,
        "velocity": note.velocity,
    })
}

fn graph_error(error: GraphError) -> CommandError {
    CommandError::invalid_args(format!("节点图错误：{error}"))
}

fn parse_args<T: DeserializeOwned>(args: &Value) -> Result<T, CommandError> {
    let normalized = if args.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        args.clone()
    };
    serde_json::from_value(normalized)
        .map_err(|error| CommandError::invalid_args(format!("参数不合法：{error}")))
}

fn parse_param_path(path: &str) -> Result<(&str, &str, &str), CommandError> {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (
            Some("tracks"),
            Some(track),
            Some("devices" | "nodes"),
            Some(device),
            Some("params"),
            Some(name),
            None,
        ) => Ok((track, device, name)),
        _ => Err(CommandError::invalid_args(format!(
            "参数路径不合法：{path}"
        ))),
    }
}

fn parse_node_path(path: &str) -> Result<(&str, &str), CommandError> {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some("tracks"), Some(track), Some("devices" | "nodes"), Some(node), None) => {
            Ok((track, node))
        },
        _ => Err(CommandError::invalid_args(format!(
            "节点路径不合法：{path}"
        ))),
    }
}

fn parse_clip_path(path: &str) -> Result<(&str, &str), CommandError> {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some("tracks"), Some(track), Some("clips"), Some(clip), None) => Ok((track, clip)),
        _ => Err(CommandError::invalid_args(format!(
            "片段路径不合法：{path}"
        ))),
    }
}

fn parse_note_path(path: &str) -> Result<(&str, &str, &str), CommandError> {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (
            Some("tracks"),
            Some(track),
            Some("clips"),
            Some(clip),
            Some("notes"),
            Some(note),
            None,
        ) => Ok((track, clip, note)),
        _ => Err(CommandError::invalid_args(format!(
            "音符路径不合法：{path}"
        ))),
    }
}

fn quantize_tick(start: u64, grid: u64, max_start: u64) -> u64 {
    if grid == 0 {
        return start.min(max_start);
    }
    (((start + grid / 2) / grid) * grid).min(max_start)
}

fn validate_clip_range(start: u64, duration: u64, length: u64) -> Result<(), CommandError> {
    if duration == 0 {
        return Err(CommandError::invalid_args("片段长度必须大于 0"));
    }
    if start.saturating_add(duration) > length {
        return Err(CommandError::invalid_args("片段必须落在工程长度内"));
    }
    Ok(())
}

fn validate_note_values(
    pitch: u8,
    start: u64,
    duration: u64,
    velocity: f64,
    clip_duration: u64,
) -> Result<(), CommandError> {
    if pitch > 127 {
        return Err(CommandError::invalid_args("pitch 必须在 0 到 127 之间"));
    }
    if duration == 0 {
        return Err(CommandError::invalid_args("音符长度必须大于 0"));
    }
    if start.saturating_add(duration) > clip_duration {
        return Err(CommandError::invalid_args("音符超出片段长度"));
    }
    if !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
        return Err(CommandError::invalid_args("velocity 必须在 0 到 1 之间"));
    }
    Ok(())
}

fn parse_id(id: &str, prefix: &str) -> Option<u64> {
    let number = id.strip_prefix(prefix)?.parse::<u64>().ok()?;
    (number > 0).then_some(number)
}

fn validate_project(project: &Project) -> Result<(u64, u64, u64, u64), CommandError> {
    if project.name.trim().is_empty() {
        return Err(CommandError::invalid_args("工程名不能为空"));
    }
    if !(8_000..=192_000).contains(&project.sample_rate) {
        return Err(CommandError::invalid_args(
            "sample_rate 必须在 8000 到 192000 之间",
        ));
    }
    validate_bpm(project.tempo_bpm)?;
    validate_time_signature(project.time_signature)?;
    validate_length_bars(project.length_bars)?;
    let length_ticks = project.length_ticks();
    validate_loop_range(project.loop_start_tick, project.loop_end_tick, length_ticks)?;

    let mut track_ids = BTreeSet::new();
    let mut device_ids = BTreeSet::new();
    let mut clip_ids = BTreeSet::new();
    let mut note_ids = BTreeSet::new();
    let mut max_track = 0;
    let mut max_device = 0;
    let mut max_clip = 0;
    let mut max_note = 0;

    for track in &project.tracks {
        let Some(number) = parse_id(&track.id, "track-") else {
            return Err(CommandError::invalid_args(format!(
                "轨道 ID 不合法：{}",
                track.id
            )));
        };
        max_track = max_track.max(number);

        if !track_ids.insert(track.id.as_str()) {
            return Err(CommandError::invalid_args(format!(
                "轨道 ID 重复：{}",
                track.id
            )));
        }
        if track.name.trim().is_empty() {
            return Err(CommandError::invalid_args(format!(
                "轨道名不能为空：{}",
                track.id
            )));
        }
        track.graph.validate().map_err(graph_error)?;
        if let Source::Sine { frequency } = track.source() {
            if !frequency.is_finite()
                || frequency <= 0.0
                || frequency >= f64::from(project.sample_rate) / 2.0
            {
                return Err(CommandError::invalid_args(format!(
                    "轨道频率不合法：{}",
                    track.id
                )));
            }
        }

        for device in track.graph.device_nodes().map_err(graph_error)? {
            let device_id = device.id().as_str();
            let Some(number) = parse_id(device_id, "device-") else {
                return Err(CommandError::invalid_args(format!(
                    "设备 ID 不合法：{device_id}"
                )));
            };
            max_device = max_device.max(number);
            if !device_ids.insert(device_id) {
                return Err(CommandError::invalid_args(format!(
                    "设备 ID 重复：{device_id}"
                )));
            }
        }

        for clip in &track.clips {
            let Some(number) = parse_id(&clip.id, "clip-") else {
                return Err(CommandError::invalid_args(format!(
                    "片段 ID 不合法：{}",
                    clip.id
                )));
            };
            max_clip = max_clip.max(number);

            if !clip_ids.insert(clip.id.as_str()) {
                return Err(CommandError::invalid_args(format!(
                    "片段 ID 重复：{}",
                    clip.id
                )));
            }
            if clip.name.trim().is_empty() {
                return Err(CommandError::invalid_args(format!(
                    "片段名不能为空：{}",
                    clip.id
                )));
            }
            validate_clip_range(clip.start_tick, clip.duration_tick, length_ticks)?;

            for note in &clip.notes {
                let Some(number) = parse_id(&note.id, "note-") else {
                    return Err(CommandError::invalid_args(format!(
                        "音符 ID 不合法：{}",
                        note.id
                    )));
                };
                max_note = max_note.max(number);

                if !note_ids.insert(note.id.as_str()) {
                    return Err(CommandError::invalid_args(format!(
                        "音符 ID 重复：{}",
                        note.id
                    )));
                }
                validate_note_values(
                    note.pitch,
                    note.start_tick,
                    note.duration_tick,
                    note.velocity,
                    clip.duration_tick,
                )?;
            }
        }
    }

    Ok((max_track, max_device, max_clip, max_note))
}

fn validate_bpm(value: f64) -> Result<(), CommandError> {
    if !value.is_finite() || !(MIN_BPM..=MAX_BPM).contains(&value) {
        return Err(CommandError::invalid_args(format!(
            "tempo_bpm 必须在 {MIN_BPM} 到 {MAX_BPM} 之间"
        )));
    }
    Ok(())
}

fn validate_time_signature(value: TimeSignature) -> Result<(), CommandError> {
    if !(1..=32).contains(&value.numerator) {
        return Err(CommandError::invalid_args("拍号分子必须在 1 到 32 之间"));
    }
    if !matches!(value.denominator, 1 | 2 | 4 | 8 | 16 | 32) {
        return Err(CommandError::invalid_args(
            "拍号分母必须是 1、2、4、8、16 或 32",
        ));
    }
    Ok(())
}

fn validate_length_bars(value: u32) -> Result<(), CommandError> {
    if !(1..=MAX_LENGTH_BARS).contains(&value) {
        return Err(CommandError::invalid_args(format!(
            "length_bars 必须在 1 到 {MAX_LENGTH_BARS} 之间"
        )));
    }
    Ok(())
}

fn validate_loop_range(start: u64, end: u64, length: u64) -> Result<(), CommandError> {
    if start >= end {
        return Err(CommandError::invalid_args("循环起点必须早于终点"));
    }
    if end > length {
        return Err(CommandError::invalid_args("循环区间不能超过工程长度"));
    }
    Ok(())
}

fn validate_adsr_param(name: &str, value: f64) -> Result<(), CommandError> {
    if !value.is_finite() {
        return Err(CommandError::invalid_args(format!("{name} 必须是有限数")));
    }
    let valid = match name {
        "attack" | "release" => (0.001..=10.0).contains(&value),
        "decay" => (0.0..=10.0).contains(&value),
        "sustain" => (0.0..=1.0).contains(&value),
        _ => false,
    };
    if !valid {
        return Err(CommandError::invalid_args(format!("{name} 超出允许范围")));
    }
    Ok(())
}

fn validate_filter_param(name: &str, value: f64) -> Result<(), CommandError> {
    if !value.is_finite() {
        return Err(CommandError::invalid_args(format!("{name} 必须是有限数")));
    }
    let valid = match name {
        "cutoff" => (20.0..=20_000.0).contains(&value),
        "resonance" => (0.0..=0.95).contains(&value),
        _ => false,
    };
    if !valid {
        return Err(CommandError::invalid_args(format!("{name} 超出允许范围")));
    }
    Ok(())
}

fn validate_gain(value: f64) -> Result<(), CommandError> {
    if !value.is_finite() || !(0.0..=4.0).contains(&value) {
        return Err(CommandError::invalid_args(
            "gain 必须是 0 到 4 之间的有限数",
        ));
    }
    Ok(())
}

fn render_project(
    project: &Project,
    start_seconds: f64,
    duration_seconds: f64,
    path: &str,
) -> Result<Value, CommandError> {
    let sample_rate = project.sample_rate;
    let sample_rate_f64 = f64::from(sample_rate);
    let frames = (duration_seconds * sample_rate_f64).round() as usize;
    let mut mix = vec![0.0_f32; frames];
    for track in &project.tracks {
        let (source_frequency, adsr_settings, graph) = track
            .playback_graph(&project.node_groups)
            .map_err(graph_error)?;
        let adsr = adsr_settings.map(|settings| AdsrPlan {
            attack: settings.attack,
            decay: settings.decay,
            sustain: settings.sustain,
            release: settings.release,
        });
        let piano = graph.piano.as_ref();
        let piano_notes: Vec<NotePlan> = if piano.is_some() {
            track
                .clips
                .iter()
                .flat_map(|clip| {
                    clip.notes.iter().map(move |note| NotePlan {
                        start: project.ticks_to_seconds(clip.start_tick + note.start_tick),
                        duration: project.ticks_to_seconds(note.duration_tick),
                        frequency: midi_to_hz(note.pitch),
                        velocity: note.velocity,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let release = adsr.map_or(0.0, |envelope| envelope.release.max(0.001));
        for (index, slot) in mix.iter_mut().enumerate() {
            let time = start_seconds + index as f64 / sample_rate_f64;
            let mut sample = if let Some(piano) = piano.as_ref() {
                piano.sample(&piano_notes, time)
            } else {
                let mut sample = source_frequency.map_or(0.0, |frequency| {
                    (std::f64::consts::TAU * frequency * time).sin() * 0.25
                });
                for clip in &track.clips {
                    for note in &clip.notes {
                        let note_start =
                            project.ticks_to_seconds(clip.start_tick + note.start_tick);
                        let note_duration = project.ticks_to_seconds(note.duration_tick);
                        let note_end = note_start + note_duration;
                        if time >= note_start && time < note_end + release {
                            let local = time - note_start;
                            let envelope = adsr.map_or_else(
                                || note_envelope(local, note_duration),
                                |envelope| adsr_envelope(local, note_duration, envelope),
                            );
                            sample += (std::f64::consts::TAU * midi_to_hz(note.pitch) * local)
                                .sin()
                                * note.velocity
                                * envelope
                                * 0.25;
                        }
                    }
                }
                sample
            };
            sample = graph.sample(sample, project.sample_rate);
            *slot += sample as f32;
        }
    }

    let mut peak = 0.0_f32;
    let mut sum_squares = 0.0_f64;
    let mut pcm = Vec::with_capacity(frames);
    for sample in &mix {
        let clamped = sample.clamp(-1.0, 1.0);
        peak = peak.max(clamped.abs());
        sum_squares += f64::from(clamped) * f64::from(clamped);
        pcm.push((clamped * 32_767.0) as i16);
    }
    let rms = if frames == 0 {
        0.0
    } else {
        (sum_squares / frames as f64).sqrt()
    };

    write_wav(Path::new(path), sample_rate, &pcm)
        .map_err(|error| CommandError::new("io_error", format!("写 WAV 失败：{error}")))?;

    Ok(json!({
        "path": path,
        "sample_rate": sample_rate,
        "frames": frames,
        "duration_seconds": frames as f64 / sample_rate_f64,
        "peak": peak,
        "rms": rms,
    }))
}

fn write_wav(path: &Path, sample_rate: u32, samples: &[i16]) -> io::Result<()> {
    let data_len = samples.len() * 2;
    let data_len_u32 = u32::try_from(data_len).map_err(|_| io::Error::other("WAV 数据过大"))?;
    let riff_len = 36_u32
        .checked_add(data_len_u32)
        .ok_or_else(|| io::Error::other("WAV 过大"))?;
    let mut bytes = Vec::with_capacity(44 + data_len);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&riff_len.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.saturating_mul(2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len_u32.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectCreateArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sample_rate: Option<u32>,
    #[serde(default)]
    tempo_bpm: Option<f64>,
    #[serde(default)]
    time_signature: Option<TimeSignature>,
    #[serde(default)]
    length_bars: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectUpdateArgs {
    #[serde(default)]
    tempo_bpm: Option<f64>,
    #[serde(default)]
    time_signature: Option<TimeSignature>,
    #[serde(default)]
    length_bars: Option<u32>,
    #[serde(default)]
    loop_start: Option<f64>,
    #[serde(default)]
    loop_end: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackCreateArgs {
    name: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    frequency: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceAddArgs {
    track_id: String,
    kind: String,
    #[serde(default)]
    params: Option<BTreeMap<String, f64>>,
    #[serde(default)]
    position: Option<[f64; 2]>,
    #[serde(default)]
    plugin_path: Option<String>,
    #[serde(default)]
    plugin_id: Option<String>,
    #[serde(default)]
    plugin_state: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamSetArgs {
    path: String,
    value: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderArgs {
    #[serde(default)]
    start_seconds: Option<f64>,
    #[serde(default)]
    duration_seconds: Option<f64>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UndoArgs {
    #[serde(default)]
    steps: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackRemoveArgs {
    track_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceRemoveArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeMoveArgs {
    path: String,
    before: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeConnectArgs {
    from: String,
    to: String,
    #[serde(default = "default_from_port")]
    from_port: String,
    #[serde(default = "default_to_port")]
    to_port: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodePositionArgs {
    positions: Vec<NodePositionArg>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NodePositionArg {
    path: String,
    x: f64,
    y: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodesRemoveArgs {
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeDuplicateArgs {
    paths: Vec<String>,
    #[serde(default)]
    offset: Option<[f64; 2]>,
}

fn default_from_port() -> String {
    "out".to_string()
}

fn default_to_port() -> String {
    "in".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectPathArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipCreateArgs {
    track_id: String,
    #[serde(default)]
    name: Option<String>,
    start: f64,
    duration: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipPathArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipUpdateArgs {
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    start: Option<f64>,
    #[serde(default)]
    duration: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipMoveArgs {
    path: String,
    track_id: String,
    #[serde(default)]
    start: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteAddArgs {
    path: String,
    pitch: u8,
    start: f64,
    duration: f64,
    #[serde(default)]
    velocity: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteUpdateArgs {
    path: String,
    #[serde(default)]
    pitch: Option<u8>,
    #[serde(default)]
    start: Option<f64>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    velocity: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotePathArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotePathsArgs {
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteQuantizeArgs {
    path: String,
    grid: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteQuantizeManyArgs {
    paths: Vec<String>,
    grid: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteDraft {
    pitch: u8,
    start: f64,
    duration: f64,
    #[serde(default)]
    velocity: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotePasteArgs {
    path: String,
    notes: Vec<NoteDraft>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipPasteArgs {
    track_id: String,
    #[serde(default)]
    name: Option<String>,
    start: f64,
    duration: f64,
    notes: Vec<NoteDraft>,
}
