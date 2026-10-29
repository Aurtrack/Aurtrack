//! 职责：
//! - 定义与 UI、IO 无关的节点、端口和连接
//! - 校验线性节点链，并编译成无分配的参数摘要
//!
//! 禁止：
//! - 依赖音频设备、宿、脚本或 UI
//! - 在音频线程里做图遍历或分配

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::needless_pass_by_value,
    clippy::missing_const_for_fn
)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod audio;
pub use audio::{
    AUDIO_NODE_TYPES, AudioConnection, AudioGraph, AudioGroup, AudioNode, AudioNodeType, AudioPort,
    AudioSocketSpec, audio_multi_input, audio_node_type,
};

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 节点的音频输入端口名。
pub const PORT_IN: &str = "in";
/// 节点的音频输出端口名。
pub const PORT_OUT: &str = "out";

/// 稳定的节点 ID。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(String);

impl NodeId {
    pub fn new(value: impl Into<String>) -> Result<Self, GraphError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            });
        if !valid {
            return Err(GraphError::InvalidNodeId(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn fixed(value: &'static str) -> Self {
        Self(value.to_string())
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 当前支持的节点类型。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    #[serde(rename = "instrument.sine")]
    InstrumentSine,
    #[serde(rename = "instrument.silence")]
    InstrumentSilence,
    #[serde(rename = "instrument.piano")]
    InstrumentPiano,
    #[serde(rename = "voice.group")]
    VoiceGroup,
    #[serde(rename = "amp.adsr")]
    AmpAdsr,
    #[serde(rename = "filter.lowpass")]
    FilterLowpass,
    #[serde(rename = "gain")]
    Gain,
    #[serde(rename = "mix")]
    Mix,
    #[serde(rename = "plugin")]
    Plugin,
    #[serde(rename = "output")]
    Output,
}

impl NodeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InstrumentSine => "instrument.sine",
            Self::InstrumentSilence => "instrument.silence",
            Self::InstrumentPiano => "instrument.piano",
            Self::VoiceGroup => "voice.group",
            Self::AmpAdsr => "amp.adsr",
            Self::FilterLowpass => "filter.lowpass",
            Self::Gain => "gain",
            Self::Mix => "mix",
            Self::Plugin => "plugin",
            Self::Output => "output",
        }
    }

    pub const fn is_instrument(self) -> bool {
        matches!(
            self,
            Self::InstrumentSine
                | Self::InstrumentSilence
                | Self::InstrumentPiano
                | Self::VoiceGroup
        )
    }

    pub const fn is_device(self) -> bool {
        matches!(
            self,
            Self::AmpAdsr | Self::FilterLowpass | Self::Gain | Self::Mix | Self::Plugin
        )
    }

    pub const fn inputs(self) -> &'static [&'static str] {
        match self {
            Self::InstrumentSine
            | Self::InstrumentSilence
            | Self::InstrumentPiano
            | Self::VoiceGroup => &[],
            Self::Mix => &["in1", "in2"],
            Self::AmpAdsr | Self::FilterLowpass | Self::Gain | Self::Plugin | Self::Output => {
                &[PORT_IN]
            },
        }
    }

    pub const fn outputs(self) -> &'static [&'static str] {
        match self {
            Self::Output => &[],
            Self::InstrumentSine
            | Self::InstrumentSilence
            | Self::InstrumentPiano
            | Self::VoiceGroup
            | Self::AmpAdsr
            | Self::FilterLowpass
            | Self::Gain
            | Self::Mix
            | Self::Plugin => &[PORT_OUT],
        }
    }

    pub fn has_input(self, port: &str) -> bool {
        self.inputs().contains(&port)
    }

    pub fn has_output(self, port: &str) -> bool {
        self.outputs().contains(&port)
    }

    pub fn param_specs(self) -> &'static [ParamSpec] {
        match self {
            Self::InstrumentSine => &SINE_PARAMS,
            Self::InstrumentPiano => &PIANO_PARAMS,
            Self::InstrumentSilence
            | Self::VoiceGroup
            | Self::Mix
            | Self::Plugin
            | Self::Output => &[],
            Self::AmpAdsr => &ADSR_PARAMS,
            Self::FilterLowpass => &FILTER_PARAMS,
            Self::Gain => &GAIN_PARAMS,
        }
    }

    pub fn default_params(self) -> BTreeMap<String, f64> {
        self.param_specs()
            .iter()
            .map(|spec| (spec.name.to_string(), spec.default))
            .collect()
    }
}

/// 节点参数的范围与默认值。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub default: f64,
    pub minimum: f64,
    pub maximum: f64,
}

const SINE_PARAMS: [ParamSpec; 1] = [ParamSpec {
    name: "frequency",
    default: 440.0,
    minimum: 20.0,
    maximum: 20_000.0,
}];

const PIANO_PARAMS: [ParamSpec; 4] = [
    ParamSpec {
        name: "partials",
        default: 10.0,
        minimum: 1.0,
        maximum: 16.0,
    },
    ParamSpec {
        name: "decay",
        default: 1.2,
        minimum: 0.1,
        maximum: 8.0,
    },
    ParamSpec {
        name: "brightness",
        default: 0.5,
        minimum: 0.0,
        maximum: 1.0,
    },
    ParamSpec {
        name: "hammer",
        default: 0.2,
        minimum: 0.0,
        maximum: 1.0,
    },
];

const ADSR_PARAMS: [ParamSpec; 4] = [
    ParamSpec {
        name: "attack",
        default: 0.01,
        minimum: 0.001,
        maximum: 10.0,
    },
    ParamSpec {
        name: "decay",
        default: 0.12,
        minimum: 0.0,
        maximum: 10.0,
    },
    ParamSpec {
        name: "sustain",
        default: 0.75,
        minimum: 0.0,
        maximum: 1.0,
    },
    ParamSpec {
        name: "release",
        default: 0.18,
        minimum: 0.001,
        maximum: 10.0,
    },
];

const FILTER_PARAMS: [ParamSpec; 2] = [
    ParamSpec {
        name: "cutoff",
        default: 1200.0,
        minimum: 20.0,
        maximum: 20_000.0,
    },
    ParamSpec {
        name: "resonance",
        default: 0.2,
        minimum: 0.0,
        maximum: 0.95,
    },
];

const GAIN_PARAMS: [ParamSpec; 1] = [ParamSpec {
    name: "gain",
    default: 1.0,
    minimum: 0.0,
    maximum: 4.0,
}];

/// 一个节点。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    id: NodeId,
    kind: NodeKind,
    params: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin: Option<PluginConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<String>,
}

/// 通用节点组数据块。具体域只替换 `graph` 的类型。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeGroupData<G> {
    pub id: String,
    pub name: String,
    pub interface: NodeGroupInterface,
    pub graph: G,
}

impl<G> NodeGroupData<G> {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        interface: NodeGroupInterface,
        graph: G,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            interface,
            graph,
        }
    }
}

/// 工程级 Voice 节点组。
pub type NodeGroup = NodeGroupData<VoiceGraph>;

impl NodeGroup {
    pub fn piano_default() -> Self {
        Self {
            id: "piano.voice".to_string(),
            name: "Piano Voice".to_string(),
            interface: NodeGroupInterface::piano(),
            graph: VoiceGraph::piano_default(),
        }
    }

    pub fn partial_default() -> Self {
        Self {
            id: "piano.partial".to_string(),
            name: "Piano Partial".to_string(),
            interface: NodeGroupInterface::partial(),
            graph: VoiceGraph::partial_default(),
        }
    }

    pub fn empty(id: impl Into<String>, name: impl Into<String>) -> Self {
        let interface = NodeGroupInterface::piano();
        Self {
            id: id.into(),
            name: name.into(),
            graph: VoiceGraph::shell(&interface),
            interface,
        }
    }

    pub fn validate(&self) -> Result<(), GraphError> {
        self.validate_with_groups(&BTreeMap::new())
    }

    pub fn validate_with_groups(
        &self,
        groups: &BTreeMap<String, NodeGroup>,
    ) -> Result<(), GraphError> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Err(GraphError::EmptyGroupId);
        }
        let mut ids = BTreeSet::new();
        for socket in self.interface.inputs.iter().chain(&self.interface.outputs) {
            if socket.id.trim().is_empty() || !ids.insert(socket.id.as_str()) {
                return Err(GraphError::DuplicateGroupSocket(socket.id.clone()));
            }
            validate_group_socket(socket)?;
        }
        if self
            .interface
            .outputs
            .iter()
            .all(|socket| socket.ty != NodeGroupSocketType::Audio)
        {
            return Err(GraphError::GroupAudioOutput);
        }
        self.graph.validate_with_groups(&self.interface, groups)
    }
}

/// 运行时端口：来自节点声明或组接口。
#[derive(Debug, Clone, PartialEq)]
pub struct VoicePort {
    pub id: String,
    pub name: String,
    pub ty: SocketType,
    pub default: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub step: f64,
    pub multi: bool,
}

fn interface_ports(sockets: &[NodeGroupSocket]) -> Vec<VoicePort> {
    sockets
        .iter()
        .map(|socket| VoicePort {
            id: socket.id.clone(),
            name: socket.name.clone(),
            ty: socket.ty,
            default: socket.effective_default().unwrap_or(0.0),
            minimum: socket.minimum.unwrap_or(f64::NEG_INFINITY),
            maximum: socket.maximum.unwrap_or(f64::INFINITY),
            step: socket.step.unwrap_or(0.01),
            multi: false,
        })
        .collect()
}

fn static_ports(sockets: &[VoiceSocketSpec]) -> Vec<VoicePort> {
    sockets
        .iter()
        .map(|socket| VoicePort {
            id: socket.id.to_string(),
            name: socket.name.to_string(),
            ty: socket.ty,
            default: socket.default,
            minimum: socket.minimum,
            maximum: socket.maximum,
            step: socket.step,
            multi: socket.multi,
        })
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn index_switch_ports(node: &VoiceNode, ty: SocketType) -> Vec<VoicePort> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = (node.params.get("count").copied().unwrap_or(4.0).round() as usize).clamp(1, 16);
    let mut ports = vec![VoicePort {
        id: "index".to_string(),
        name: "Index".to_string(),
        ty: SocketType::Float,
        default: 0.0,
        minimum: 0.0,
        maximum: (count - 1) as f64,
        step: 1.0,
        multi: false,
    }];
    for index in 0..count {
        ports.push(VoicePort {
            id: index.to_string(),
            name: index.to_string(),
            ty,
            default: 0.0,
            minimum: f64::NEG_INFINITY,
            maximum: f64::INFINITY,
            step: 0.01,
            multi: false,
        });
    }
    ports
}

pub fn node_inputs(
    node: &VoiceNode,
    interface: &NodeGroupInterface,
    groups: &BTreeMap<String, NodeGroup>,
) -> Result<Vec<VoicePort>, GraphError> {
    match node.type_id.as_str() {
        "voice.index_switch" => Ok(index_switch_ports(node, SocketType::Audio)),
        "voice.index_switch_float" => Ok(index_switch_ports(node, SocketType::Float)),
        "group.input" => Ok(Vec::new()),
        "group.output" => Ok(interface_ports(&interface.outputs)),
        "voice.group" => {
            let group_id = node.group.as_deref().ok_or(GraphError::EmptyGroupId)?;
            let group = groups
                .get(group_id)
                .ok_or_else(|| GraphError::GroupMissing(group_id.to_string()))?;
            Ok(interface_ports(&group.interface.inputs))
        },
        other => voice_node_type(other)
            .map(|node_type| static_ports(node_type.inputs))
            .ok_or_else(|| GraphError::UnknownVoiceNodeType(other.to_string())),
    }
}

pub fn node_outputs(
    node: &VoiceNode,
    interface: &NodeGroupInterface,
    groups: &BTreeMap<String, NodeGroup>,
) -> Result<Vec<VoicePort>, GraphError> {
    match node.type_id.as_str() {
        "group.input" => Ok(interface_ports(&interface.inputs)),
        "group.output" => Ok(Vec::new()),
        "voice.group" => {
            let group_id = node.group.as_deref().ok_or(GraphError::EmptyGroupId)?;
            let group = groups
                .get(group_id)
                .ok_or_else(|| GraphError::GroupMissing(group_id.to_string()))?;
            Ok(interface_ports(&group.interface.outputs))
        },
        other => voice_node_type(other)
            .map(|node_type| static_ports(node_type.outputs))
            .ok_or_else(|| GraphError::UnknownVoiceNodeType(other.to_string())),
    }
}

/// 节点组对外暴露的接口。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeGroupInterface {
    pub inputs: Vec<NodeGroupSocket>,
    pub outputs: Vec<NodeGroupSocket>,
}

impl NodeGroupInterface {
    /// 空接口，没有输入或输出 socket。
    pub fn empty() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub fn piano() -> Self {
        Self {
            inputs: vec![NodeGroupSocket::new(
                "note",
                "Note",
                NodeGroupSocketType::Note,
            )],
            outputs: vec![NodeGroupSocket::new(
                "audio",
                "Audio",
                NodeGroupSocketType::Audio,
            )],
        }
    }

    pub fn partial() -> Self {
        Self {
            inputs: vec![NodeGroupSocket::new(
                "in",
                "Audio",
                NodeGroupSocketType::Audio,
            )],
            outputs: vec![NodeGroupSocket::new(
                "audio",
                "Audio",
                NodeGroupSocketType::Audio,
            )],
        }
    }
}

/// 组接口上的一个 socket。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeGroupSocket {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: NodeGroupSocketType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl NodeGroupSocket {
    pub fn new(id: &str, name: &str, ty: NodeGroupSocketType) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            ty,
            default: None,
            minimum: None,
            maximum: None,
            step: None,
            panel: None,
            description: None,
        }
    }

    #[must_use]
    pub fn with_default(mut self, default: f64) -> Self {
        self.default = Some(default);
        self
    }

    #[must_use]
    pub fn with_range(mut self, minimum: f64, maximum: f64) -> Self {
        self.minimum = Some(minimum);
        self.maximum = Some(maximum);
        self
    }

    #[must_use]
    pub fn with_step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    #[must_use]
    pub fn with_panel(mut self, panel: impl Into<String>) -> Self {
        self.panel = Some(panel.into());
        self
    }

    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 未连接时使用的数值。Audio 和 Note socket 没有数值默认值。
    pub fn effective_default(&self) -> Option<f64> {
        self.default.or(match self.ty {
            NodeGroupSocketType::Float | NodeGroupSocketType::Gate => Some(0.0),
            NodeGroupSocketType::Audio | NodeGroupSocketType::Note => None,
        })
    }
}

fn validate_group_socket(socket: &NodeGroupSocket) -> Result<(), GraphError> {
    let invalid = |reason: &str| GraphError::InvalidGroupSocket {
        socket: socket.id.clone(),
        reason: reason.to_string(),
    };
    if matches!(
        socket.ty,
        NodeGroupSocketType::Audio | NodeGroupSocketType::Note
    ) && (socket.default.is_some()
        || socket.minimum.is_some()
        || socket.maximum.is_some()
        || socket.step.is_some())
    {
        return Err(invalid("audio/note socket 不能有数值默认值或范围"));
    }
    for value in [socket.default, socket.minimum, socket.maximum, socket.step]
        .into_iter()
        .flatten()
    {
        if !value.is_finite() {
            return Err(invalid("数值必须是有限数"));
        }
    }
    if let (Some(minimum), Some(maximum)) = (socket.minimum, socket.maximum) {
        if minimum > maximum {
            return Err(invalid("最小值不能大于最大值"));
        }
    }
    if let Some(step) = socket.step {
        if step <= 0.0 {
            return Err(invalid("步进必须大于 0"));
        }
    }
    if let Some(default) = socket.default {
        if socket.minimum.is_some_and(|minimum| default < minimum)
            || socket.maximum.is_some_and(|maximum| default > maximum)
        {
            return Err(invalid("默认值超出范围"));
        }
    }
    Ok(())
}

/// 组接口 socket 类型。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeGroupSocketType {
    #[serde(rename = "float")]
    Float,
    #[serde(rename = "gate")]
    Gate,
    #[serde(rename = "audio")]
    Audio,
    #[serde(rename = "note")]
    Note,
}

/// 节点 socket 的数据类型。
pub type SocketType = NodeGroupSocketType;

/// 节点类型的一个端口声明。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceSocketSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub ty: SocketType,
    pub default: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub step: f64,
    pub multi: bool,
}

impl VoiceSocketSpec {
    pub const fn new(id: &'static str, name: &'static str, ty: SocketType) -> Self {
        Self {
            id,
            name,
            ty,
            default: 0.0,
            minimum: f64::NEG_INFINITY,
            maximum: f64::INFINITY,
            step: 0.01,
            multi: false,
        }
    }

    #[must_use]
    pub const fn multi(mut self) -> Self {
        self.multi = true;
        self
    }

    #[must_use]
    pub const fn with_default(mut self, default: f64) -> Self {
        self.default = default;
        self
    }

    #[must_use]
    pub const fn with_range(mut self, minimum: f64, maximum: f64) -> Self {
        self.minimum = minimum;
        self.maximum = maximum;
        self
    }

    #[must_use]
    pub const fn with_step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }
}

/// 可注册的 Voice 节点类型。
#[derive(Debug, Clone, Copy)]
pub struct VoiceNodeType {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub inputs: &'static [VoiceSocketSpec],
    pub outputs: &'static [VoiceSocketSpec],
    pub params: &'static [ParamSpec],
}

const MATH_PARAMS: [ParamSpec; 1] = [ParamSpec {
    name: "value",
    default: 0.0,
    minimum: -1000.0,
    maximum: 1000.0,
}];

const NOTE_IN: [VoiceSocketSpec; 1] = [VoiceSocketSpec::new("note", "Note", SocketType::Note)];

const ATTACK_INPUTS: [VoiceSocketSpec; 2] = [
    VoiceSocketSpec::new("time", "Time", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 1000.0)
        .with_step(0.001),
    VoiceSocketSpec::new("attack", "Attack", SocketType::Float)
        .with_default(0.004)
        .with_range(0.0, 1.0)
        .with_step(0.001),
];
const DECAY_INPUTS: [VoiceSocketSpec; 2] = [
    VoiceSocketSpec::new("time", "Time", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 1000.0)
        .with_step(0.001),
    VoiceSocketSpec::new("decay", "Decay", SocketType::Float)
        .with_default(1.2)
        .with_range(0.05, 8.0)
        .with_step(0.001),
];

const NOISE_SOURCE_INPUTS: [VoiceSocketSpec; 2] = [
    VoiceSocketSpec::new("time", "Time", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 1000.0)
        .with_step(0.001),
    VoiceSocketSpec::new("seed", "Seed", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 1.0)
        .with_step(0.001),
];

const VCA_INPUTS: [VoiceSocketSpec; 2] = [
    VoiceSocketSpec::new("in", "Audio", SocketType::Audio),
    VoiceSocketSpec::new("gain", "Gain", SocketType::Float)
        .with_default(1.0)
        .with_range(0.0, 2.0)
        .with_step(0.01),
];
const PARTIAL_INPUTS: [VoiceSocketSpec; 7] = [
    VoiceSocketSpec::new("pitch", "Pitch", SocketType::Float)
        .with_default(261.63)
        .with_range(20.0, 20_000.0)
        .with_step(0.01),
    VoiceSocketSpec::new("velocity", "Velocity", SocketType::Float)
        .with_default(0.8)
        .with_range(0.0, 1.0)
        .with_step(0.01),
    VoiceSocketSpec::new("time", "Time", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 1000.0)
        .with_step(0.001),
    VoiceSocketSpec::new("index", "Index", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 63.0)
        .with_step(1.0),
    VoiceSocketSpec::new("inharmonicity", "Inharmonicity", SocketType::Float)
        .with_default(0.0004)
        .with_range(0.0, 0.01)
        .with_step(0.0001),
    VoiceSocketSpec::new("brightness", "Brightness", SocketType::Float)
        .with_default(0.5)
        .with_range(0.0, 1.0)
        .with_step(0.01),
    VoiceSocketSpec::new("decay", "Decay", SocketType::Float)
        .with_default(1.2)
        .with_range(0.05, 8.0)
        .with_step(0.01),
];
const REPEAT_INPUTS: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("count", "Count", SocketType::Float)
        .with_default(10.0)
        .with_range(1.0, 64.0)
        .with_step(1.0)];
const ZONE_INPUT_OUTPUTS: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("out", "Audio", SocketType::Audio)];
const ZONE_OUTPUT_INPUTS: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("in", "Audio", SocketType::Audio)];
const CONTEXT_PITCH_OUT: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("out", "Pitch", SocketType::Float)];
const CONTEXT_VELOCITY_OUT: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("out", "Velocity", SocketType::Float)];
const CONTEXT_ITERATION_OUT: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("out", "Iteration", SocketType::Float)];
const SAMPLE_HOLD_INPUTS: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("next", "Next", SocketType::Float)];
const BROADCAST_INPUTS: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("value", "Value", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 1.0)
        .with_step(0.01)];
const SUM_INPUTS: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("in", "Audio", SocketType::Audio).multi()];
const SUM_FLOAT_INPUTS: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("in", "Value", SocketType::Float).multi()];
const VECTOR_ADD_INPUTS: [VoiceSocketSpec; 6] = [
    VoiceSocketSpec::new("x1", "X1", SocketType::Float),
    VoiceSocketSpec::new("y1", "Y1", SocketType::Float),
    VoiceSocketSpec::new("z1", "Z1", SocketType::Float),
    VoiceSocketSpec::new("x2", "X2", SocketType::Float),
    VoiceSocketSpec::new("y2", "Y2", SocketType::Float),
    VoiceSocketSpec::new("z2", "Z2", SocketType::Float),
];
const VECTOR_OUT: [VoiceSocketSpec; 3] = [
    VoiceSocketSpec::new("x", "X", SocketType::Float),
    VoiceSocketSpec::new("y", "Y", SocketType::Float),
    VoiceSocketSpec::new("z", "Z", SocketType::Float),
];
const INDEX_SWITCH_PARAMS: [ParamSpec; 1] = [ParamSpec {
    name: "count",
    default: 4.0,
    minimum: 1.0,
    maximum: 16.0,
}];
const SWITCH_INPUTS: [VoiceSocketSpec; 3] = [
    VoiceSocketSpec::new("switch", "Switch", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 1.0)
        .with_step(1.0),
    VoiceSocketSpec::new("false", "False", SocketType::Audio),
    VoiceSocketSpec::new("true", "True", SocketType::Audio),
];

const AUDIO_OUT: [VoiceSocketSpec; 1] = [VoiceSocketSpec::new("out", "Audio", SocketType::Audio)];
const FLOAT_OUT: [VoiceSocketSpec; 1] = [VoiceSocketSpec::new("out", "Value", SocketType::Float)];
const VOICE_ADD_INPUTS: [VoiceSocketSpec; 2] = [
    VoiceSocketSpec::new("in1", "A", SocketType::Audio),
    VoiceSocketSpec::new("in2", "B", SocketType::Audio),
];
const VOICE_MIX_INPUTS: [VoiceSocketSpec; 3] = [
    VoiceSocketSpec::new("factor", "Factor", SocketType::Float)
        .with_default(0.5)
        .with_range(0.0, 1.0)
        .with_step(0.01),
    VoiceSocketSpec::new("in1", "A", SocketType::Audio),
    VoiceSocketSpec::new("in2", "B", SocketType::Audio),
];
const VOICE_GATE_INPUTS: [VoiceSocketSpec; 2] = [
    VoiceSocketSpec::new("in", "Audio", SocketType::Audio),
    VoiceSocketSpec::new("gate", "Gate", SocketType::Gate)
        .with_default(0.0)
        .with_range(0.0, 1.0)
        .with_step(1.0),
];
const VOICE_TIME_OUT: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("out", "Time", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 1000.0)
        .with_step(0.001)];
const WHITE_NOISE_INPUTS: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("w", "W", SocketType::Float)
        .with_default(0.0)
        .with_range(-1000.0, 1000.0)
        .with_step(0.01)];
const WHITE_NOISE_OUT: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("out", "Value", SocketType::Float)
        .with_default(0.0)
        .with_range(0.0, 1.0)
        .with_step(0.001)];
const TWO_FLOAT_IN: [VoiceSocketSpec; 2] = [
    VoiceSocketSpec::new("a", "A", SocketType::Float)
        .with_default(0.0)
        .with_range(-1000.0, 1000.0),
    VoiceSocketSpec::new("b", "B", SocketType::Float)
        .with_default(0.0)
        .with_range(-1000.0, 1000.0),
];
const REROUTE_FLOAT_IN: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("in", "Value", SocketType::Float)];
const REROUTE_GATE_IN: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("in", "Gate", SocketType::Gate)];
const REROUTE_AUDIO_IN: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("in", "Audio", SocketType::Audio)];
const REROUTE_FLOAT_OUT: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("out", "Value", SocketType::Float)];
const REROUTE_GATE_OUT: [VoiceSocketSpec; 1] =
    [VoiceSocketSpec::new("out", "Gate", SocketType::Gate)];

const MAP_RANGE_IN: [VoiceSocketSpec; 5] = [
    VoiceSocketSpec::new("value", "Value", SocketType::Float)
        .with_default(0.0)
        .with_range(-1000.0, 1000.0),
    VoiceSocketSpec::new("from_min", "From Min", SocketType::Float)
        .with_default(0.0)
        .with_range(-1000.0, 1000.0),
    VoiceSocketSpec::new("from_max", "From Max", SocketType::Float)
        .with_default(1.0)
        .with_range(-1000.0, 1000.0),
    VoiceSocketSpec::new("to_min", "To Min", SocketType::Float)
        .with_default(0.0)
        .with_range(-1000.0, 1000.0),
    VoiceSocketSpec::new("to_max", "To Max", SocketType::Float)
        .with_default(1.0)
        .with_range(-1000.0, 1000.0),
];

const CLAMP_INPUTS: [VoiceSocketSpec; 3] = [
    VoiceSocketSpec::new("value", "Value", SocketType::Float)
        .with_default(0.0)
        .with_range(-1000.0, 1000.0),
    VoiceSocketSpec::new("min", "Min", SocketType::Float)
        .with_default(0.0)
        .with_range(-1000.0, 1000.0),
    VoiceSocketSpec::new("max", "Max", SocketType::Float)
        .with_default(1.0)
        .with_range(-1000.0, 1000.0),
];

/// 所有可用 Voice 节点类型。UI 和命令面都从这里生成。
pub const VOICE_NODE_TYPES: &[VoiceNodeType] = &[
    VoiceNodeType {
        id: "group.input",
        name: "Group Input",
        category: "Input",
        inputs: &[],
        outputs: &[],
        params: &[],
    },
    VoiceNodeType {
        id: "group.output",
        name: "Group Output",
        category: "Output",
        inputs: &[],
        outputs: &[],
        params: &[],
    },
    VoiceNodeType {
        id: "note.separate",
        name: "Separate Note",
        category: "Note",
        inputs: &NOTE_IN,
        outputs: &[
            VoiceSocketSpec::new("pitch", "Pitch", SocketType::Float),
            VoiceSocketSpec::new("velocity", "Velocity", SocketType::Float),
            VoiceSocketSpec::new("gate", "Gate", SocketType::Gate),
            VoiceSocketSpec::new("index", "Index", SocketType::Float)
                .with_default(0.0)
                .with_range(0.0, 1_000_000.0)
                .with_step(1.0),
        ],
        params: &[],
    },
    VoiceNodeType {
        id: "voice.group",
        name: "Group",
        category: "Group",
        inputs: &[],
        outputs: &[],
        params: &[],
    },
    VoiceNodeType {
        id: "voice.partial",
        name: "Partial",
        category: "Generator",
        inputs: &PARTIAL_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.switch",
        name: "Switch",
        category: "Utility",
        inputs: &SWITCH_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.sum",
        name: "Sum",
        category: "Math",
        inputs: &SUM_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.sum_float",
        name: "Sum Float",
        category: "Math",
        inputs: &SUM_FLOAT_INPUTS,
        outputs: &FLOAT_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.vector_add",
        name: "Vector Add",
        category: "Math",
        inputs: &VECTOR_ADD_INPUTS,
        outputs: &VECTOR_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.index_switch",
        name: "Index Switch",
        category: "Utility",
        inputs: &[],
        outputs: &AUDIO_OUT,
        params: &INDEX_SWITCH_PARAMS,
    },
    VoiceNodeType {
        id: "voice.index_switch_float",
        name: "Index Switch Float",
        category: "Utility",
        inputs: &[],
        outputs: &FLOAT_OUT,
        params: &INDEX_SWITCH_PARAMS,
    },
    VoiceNodeType {
        id: "voice.broadcast",
        name: "Broadcast",
        category: "Utility",
        inputs: &BROADCAST_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.sample_hold",
        name: "Sample Hold",
        category: "Utility",
        inputs: &SAMPLE_HOLD_INPUTS,
        outputs: &FLOAT_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.repeat",
        name: "Repeat",
        category: "Group",
        inputs: &REPEAT_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.zone.repeat",
        name: "Repeat Zone",
        category: "Group",
        inputs: &REPEAT_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.zone.input",
        name: "Zone Input",
        category: "Group",
        inputs: &[],
        outputs: &ZONE_INPUT_OUTPUTS,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.zone.output",
        name: "Zone Output",
        category: "Group",
        inputs: &ZONE_OUTPUT_INPUTS,
        outputs: &[],
        params: &[],
    },
    VoiceNodeType {
        id: "voice.iteration",
        name: "Iteration",
        category: "Input",
        inputs: &[],
        outputs: &CONTEXT_ITERATION_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.pitch",
        name: "Pitch",
        category: "Input",
        inputs: &[],
        outputs: &CONTEXT_PITCH_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.velocity",
        name: "Velocity",
        category: "Input",
        inputs: &[],
        outputs: &CONTEXT_VELOCITY_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.attack",
        name: "Attack",
        category: "Envelope",
        inputs: &ATTACK_INPUTS,
        outputs: &FLOAT_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.decay",
        name: "Decay",
        category: "Envelope",
        inputs: &DECAY_INPUTS,
        outputs: &FLOAT_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.noise",
        name: "Noise",
        category: "Generator",
        inputs: &NOISE_SOURCE_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.vca",
        name: "VCA",
        category: "Audio",
        inputs: &VCA_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.add",
        name: "Add",
        category: "Audio",
        inputs: &VOICE_ADD_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.mix",
        name: "Mix",
        category: "Audio",
        inputs: &VOICE_MIX_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.gate",
        name: "Gate",
        category: "Audio",
        inputs: &VOICE_GATE_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.time",
        name: "Time",
        category: "Input",
        inputs: &[],
        outputs: &VOICE_TIME_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "voice.white_noise",
        name: "White Noise",
        category: "Generator",
        inputs: &WHITE_NOISE_INPUTS,
        outputs: &WHITE_NOISE_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "math.add",
        name: "Add",
        category: "Math",
        inputs: &TWO_FLOAT_IN,
        outputs: &FLOAT_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "math.multiply",
        name: "Multiply",
        category: "Math",
        inputs: &TWO_FLOAT_IN,
        outputs: &FLOAT_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "math.map_range",
        name: "Map Range",
        category: "Math",
        inputs: &MAP_RANGE_IN,
        outputs: &FLOAT_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "math.clamp",
        name: "Clamp",
        category: "Math",
        inputs: &CLAMP_INPUTS,
        outputs: &FLOAT_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "value.float",
        name: "Value",
        category: "Input",
        inputs: &[],
        outputs: &FLOAT_OUT,
        params: &MATH_PARAMS,
    },
    VoiceNodeType {
        id: "reroute.float",
        name: "Reroute Float",
        category: "Layout",
        inputs: &REROUTE_FLOAT_IN,
        outputs: &REROUTE_FLOAT_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "reroute.gate",
        name: "Reroute Gate",
        category: "Layout",
        inputs: &REROUTE_GATE_IN,
        outputs: &REROUTE_GATE_OUT,
        params: &[],
    },
    VoiceNodeType {
        id: "reroute.audio",
        name: "Reroute Audio",
        category: "Layout",
        inputs: &REROUTE_AUDIO_IN,
        outputs: &AUDIO_OUT,
        params: &[],
    },
];

pub fn voice_node_type(id: &str) -> Option<&'static VoiceNodeType> {
    VOICE_NODE_TYPES.iter().find(|node_type| node_type.id == id)
}

/// 通用节点树存储。域图只决定节点和连接类型，不复制容器结构。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeTree<N, C> {
    pub nodes: BTreeMap<NodeId, N>,
    pub connections: Vec<C>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub frames: BTreeMap<String, VoiceFrame>,
}

/// 钢琴内部的 Voice 子图。每条连接表示一个音频或控制值。
pub type VoiceGraph = NodeTree<VoiceNode, VoiceConnection>;

/// 通用节点树连接必须暴露起点和终点，供拓扑排序和编辑器共用。
pub trait TreeConnection {
    fn from(&self) -> &PortRef;
    fn to(&self) -> &PortRef;
    fn muted(&self) -> bool {
        false
    }
}

impl TreeConnection for VoiceConnection {
    fn from(&self) -> &PortRef {
        self.from()
    }

    fn to(&self) -> &PortRef {
        self.to()
    }

    fn muted(&self) -> bool {
        self.muted
    }
}

impl TreeConnection for AudioConnection {
    fn from(&self) -> &PortRef {
        &self.from
    }

    fn to(&self) -> &PortRef {
        &self.to
    }
}

impl<N, C> NodeTree<N, C> {
    pub fn node(&self, id: &NodeId) -> Option<&N> {
        self.nodes.get(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &N> {
        self.nodes.values()
    }

    pub fn connections(&self) -> &[C] {
        &self.connections
    }

    pub fn connections_mut(&mut self) -> &mut Vec<C> {
        &mut self.connections
    }
}

impl<N, C: TreeConnection> NodeTree<N, C> {
    pub fn topological_order(&self) -> Result<Vec<NodeId>, GraphError> {
        let mut incoming = BTreeMap::<NodeId, usize>::new();
        let mut outgoing = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for id in self.nodes.keys() {
            incoming.insert(id.clone(), 0);
        }
        for connection in &self.connections {
            if connection.muted() {
                continue;
            }
            *incoming
                .get_mut(connection.to().node())
                .ok_or_else(|| GraphError::UnknownNode(connection.to().node().clone()))? += 1;
            outgoing
                .entry(connection.from().node().clone())
                .or_default()
                .push(connection.to().node().clone());
        }
        let mut queue: VecDeque<NodeId> = incoming
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(id, _)| id.clone())
            .collect();
        let mut order = Vec::new();
        while let Some(id) = queue.pop_front() {
            order.push(id.clone());
            for next in outgoing.get(&id).into_iter().flatten() {
                let count = incoming
                    .get_mut(next)
                    .ok_or_else(|| GraphError::UnknownNode(next.clone()))?;
                *count = count.saturating_sub(1);
                if *count == 0 {
                    queue.push_back(next.clone());
                }
            }
        }
        if order.len() != self.nodes.len() {
            return Err(GraphError::Cycle);
        }
        Ok(order)
    }
}

/// Voice 图里的背景框，只影响编辑器的组织方式。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceFrame {
    pub id: String,
    pub name: String,
    pub position: [f64; 2],
    pub size: [f64; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl Default for VoiceGraph {
    fn default() -> Self {
        Self::piano_default()
    }
}

impl VoiceGraph {
    pub fn shell(interface: &NodeGroupInterface) -> Self {
        let input = VoiceNode::new(NodeId::fixed("voice-in"), "group.input");
        let output = VoiceNode::new(NodeId::fixed("out"), "group.output");
        let mut nodes = BTreeMap::new();
        nodes.insert(input.id.clone(), input);
        nodes.insert(output.id.clone(), output);
        if let Some(node) = nodes.get_mut(&NodeId::fixed("voice-in")) {
            node.position = Some([40.0, 100.0]);
        }
        if let Some(node) = nodes.get_mut(&NodeId::fixed("out")) {
            node.position = Some([360.0, 100.0]);
        }
        let _ = interface;
        Self {
            nodes,
            connections: Vec::new(),
            frames: BTreeMap::new(),
        }
    }

    pub fn partial_default() -> Self {
        let interface = NodeGroupInterface::partial();
        let mut graph = Self::shell(&interface);
        let pitch = VoiceNode::new(NodeId::fixed("pitch"), "voice.pitch");
        let velocity = VoiceNode::new(NodeId::fixed("velocity"), "voice.velocity");
        let time = VoiceNode::new(NodeId::fixed("time"), "voice.time");
        let index = VoiceNode::new(NodeId::fixed("index"), "voice.iteration");
        let partial = VoiceNode::new(NodeId::fixed("partial"), "voice.partial");
        let add = VoiceNode::new(NodeId::fixed("add"), "voice.add");
        for (node, position) in [
            (pitch, [40.0, 220.0]),
            (velocity, [40.0, 300.0]),
            (time, [40.0, 380.0]),
            (index, [40.0, 460.0]),
            (partial, [300.0, 180.0]),
            (add, [600.0, 100.0]),
        ] {
            let mut node = node;
            node.position = Some(position);
            graph.nodes.insert(node.id.clone(), node);
        }
        if let Some(output) = graph.nodes.get_mut(&NodeId::fixed("out")) {
            output.position = Some([850.0, 100.0]);
        }
        graph.connections = vec![
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("voice-in"), "in"),
                PortRef::fixed(NodeId::fixed("add"), "in1"),
            ),
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("pitch"), "out"),
                PortRef::fixed(NodeId::fixed("partial"), "pitch"),
            ),
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("velocity"), "out"),
                PortRef::fixed(NodeId::fixed("partial"), "velocity"),
            ),
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("time"), "out"),
                PortRef::fixed(NodeId::fixed("partial"), "time"),
            ),
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("index"), "out"),
                PortRef::fixed(NodeId::fixed("partial"), "index"),
            ),
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("partial"), "out"),
                PortRef::fixed(NodeId::fixed("add"), "in2"),
            ),
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("add"), "out"),
                PortRef::fixed(NodeId::fixed("out"), "audio"),
            ),
        ];
        graph
    }

    #[allow(clippy::too_many_lines)]
    pub fn piano_default() -> Self {
        let input = VoiceNode::new(NodeId::fixed("voice-in"), "group.input");
        let separate = VoiceNode::new(NodeId::fixed("note-separate"), "note.separate");
        let mut partials = VoiceNode::new(NodeId::fixed("partials"), "voice.zone.repeat");
        partials.params.insert("count".to_string(), 10.0);
        let mut zone_in = VoiceNode::new(NodeId::fixed("p-in"), "voice.zone.input");
        let mut zone_out = VoiceNode::new(NodeId::fixed("p-out"), "voice.zone.output");
        let mut partial = VoiceNode::new(NodeId::fixed("p-partial"), "voice.partial");
        let mut partial_add = VoiceNode::new(NodeId::fixed("p-add"), "voice.add");
        let mut partial_pitch = VoiceNode::new(NodeId::fixed("p-pitch"), "voice.pitch");
        let mut partial_velocity = VoiceNode::new(NodeId::fixed("p-velocity"), "voice.velocity");
        let mut partial_time = VoiceNode::new(NodeId::fixed("p-time"), "voice.time");
        let mut partial_index = VoiceNode::new(NodeId::fixed("p-index"), "voice.iteration");
        for node in [
            &mut zone_in,
            &mut zone_out,
            &mut partial,
            &mut partial_add,
            &mut partial_pitch,
            &mut partial_velocity,
            &mut partial_time,
            &mut partial_index,
        ] {
            node.zone = Some("partials".to_string());
        }
        let env_attack = VoiceNode::new(NodeId::fixed("env-attack"), "voice.attack");
        let env_decay = VoiceNode::new(NodeId::fixed("env-decay"), "voice.decay");
        let env_gain = VoiceNode::new(NodeId::fixed("env-gain"), "math.multiply");
        let osc_vca = VoiceNode::new(NodeId::fixed("osc-vca"), "voice.vca");
        let noise = VoiceNode::new(NodeId::fixed("noise"), "voice.noise");
        let mut noise_env_attack =
            VoiceNode::new(NodeId::fixed("noise-env-attack"), "voice.attack");
        noise_env_attack.params.insert("attack".to_string(), 0.001);
        let mut noise_env_decay = VoiceNode::new(NodeId::fixed("noise-env-decay"), "voice.decay");
        noise_env_decay.params.insert("decay".to_string(), 0.008);
        let noise_env_gain = VoiceNode::new(NodeId::fixed("noise-env-gain"), "math.multiply");
        let noise_vca = VoiceNode::new(NodeId::fixed("noise-vca"), "voice.vca");
        let time = VoiceNode::new(NodeId::fixed("time"), "voice.time");
        let white_noise = VoiceNode::new(NodeId::fixed("white-noise"), "voice.white_noise");
        let vel_level = VoiceNode::new(NodeId::fixed("vel-level"), "math.multiply");
        let noise_gain = VoiceNode::new(NodeId::fixed("noise-gain"), "math.multiply");
        let mut level = VoiceNode::new(NodeId::fixed("level"), "value.float");
        level.params.insert("value".to_string(), 0.2);
        let add = VoiceNode::new(NodeId::fixed("add"), "voice.add");
        let gate = VoiceNode::new(NodeId::fixed("gate"), "voice.gate");
        let output = VoiceNode::new(NodeId::fixed("out"), "group.output");
        let mut nodes = BTreeMap::new();
        for node in [
            input,
            separate,
            partials,
            env_attack,
            env_decay,
            env_gain,
            osc_vca,
            noise,
            noise_env_attack,
            noise_env_decay,
            noise_env_gain,
            noise_vca,
            time,
            white_noise,
            vel_level,
            noise_gain,
            level,
            add,
            gate,
            output,
            zone_in,
            zone_out,
            partial,
            partial_add,
            partial_pitch,
            partial_velocity,
            partial_time,
            partial_index,
        ] {
            nodes.insert(node.id.clone(), node);
        }
        let positions = [
            ("voice-in", [30.0, 140.0]),
            ("note-separate", [280.0, 140.0]),
            ("partials", [500.0, 40.0]),
            ("env-attack", [720.0, 20.0]),
            ("env-decay", [720.0, 110.0]),
            ("env-gain", [940.0, 65.0]),
            ("osc-vca", [1140.0, 40.0]),
            ("noise", [500.0, 250.0]),
            ("noise-env-attack", [720.0, 230.0]),
            ("noise-env-decay", [720.0, 320.0]),
            ("noise-env-gain", [940.0, 275.0]),
            ("noise-vca", [1140.0, 230.0]),
            ("time", [30.0, 700.0]),
            ("white-noise", [280.0, 360.0]),
            ("vel-level", [500.0, 560.0]),
            ("level", [280.0, 560.0]),
            ("noise-gain", [720.0, 560.0]),
            ("add", [1360.0, 150.0]),
            ("gate", [1540.0, 150.0]),
            ("out", [1720.0, 150.0]),
            ("partials", [470.0, 760.0]),
            ("p-in", [500.0, 820.0]),
            ("p-partial", [700.0, 820.0]),
            ("p-add", [940.0, 820.0]),
            ("p-out", [1140.0, 820.0]),
            ("p-pitch", [500.0, 940.0]),
            ("p-velocity", [500.0, 1020.0]),
            ("p-time", [500.0, 1100.0]),
            ("p-index", [500.0, 1180.0]),
        ];
        for (id, position) in positions {
            if let Some(node) = nodes.get_mut(&NodeId::fixed(id)) {
                node.position = Some(position);
            }
        }
        Self {
            nodes,
            connections: vec![
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("voice-in"), "note"),
                    PortRef::fixed(NodeId::fixed("note-separate"), "note"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("note-separate"), "velocity"),
                    PortRef::fixed(NodeId::fixed("vel-level"), "a"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("level"), "out"),
                    PortRef::fixed(NodeId::fixed("vel-level"), "b"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("vel-level"), "out"),
                    PortRef::fixed(NodeId::fixed("noise-gain"), "a"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("noise-env-gain"), "out"),
                    PortRef::fixed(NodeId::fixed("noise-gain"), "b"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("noise-gain"), "out"),
                    PortRef::fixed(NodeId::fixed("noise-vca"), "gain"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("noise"), "out"),
                    PortRef::fixed(NodeId::fixed("noise-vca"), "in"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("noise-vca"), "out"),
                    PortRef::fixed(NodeId::fixed("add"), "in2"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("partials"), "out"),
                    PortRef::fixed(NodeId::fixed("osc-vca"), "in"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("env-gain"), "out"),
                    PortRef::fixed(NodeId::fixed("osc-vca"), "gain"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("osc-vca"), "out"),
                    PortRef::fixed(NodeId::fixed("add"), "in1"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("time"), "out"),
                    PortRef::fixed(NodeId::fixed("env-attack"), "time"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("time"), "out"),
                    PortRef::fixed(NodeId::fixed("env-decay"), "time"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("env-attack"), "out"),
                    PortRef::fixed(NodeId::fixed("env-gain"), "a"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("env-decay"), "out"),
                    PortRef::fixed(NodeId::fixed("env-gain"), "b"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("time"), "out"),
                    PortRef::fixed(NodeId::fixed("noise-env-attack"), "time"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("time"), "out"),
                    PortRef::fixed(NodeId::fixed("noise-env-decay"), "time"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("noise-env-attack"), "out"),
                    PortRef::fixed(NodeId::fixed("noise-env-gain"), "a"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("noise-env-decay"), "out"),
                    PortRef::fixed(NodeId::fixed("noise-env-gain"), "b"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("time"), "out"),
                    PortRef::fixed(NodeId::fixed("noise"), "time"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("note-separate"), "index"),
                    PortRef::fixed(NodeId::fixed("white-noise"), "w"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("white-noise"), "out"),
                    PortRef::fixed(NodeId::fixed("noise"), "seed"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("note-separate"), "gate"),
                    PortRef::fixed(NodeId::fixed("gate"), "gate"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("add"), "out"),
                    PortRef::fixed(NodeId::fixed("gate"), "in"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("gate"), "out"),
                    PortRef::fixed(NodeId::fixed("out"), "audio"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("p-in"), "out"),
                    PortRef::fixed(NodeId::fixed("p-add"), "in1"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("p-pitch"), "out"),
                    PortRef::fixed(NodeId::fixed("p-partial"), "pitch"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("p-velocity"), "out"),
                    PortRef::fixed(NodeId::fixed("p-partial"), "velocity"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("p-time"), "out"),
                    PortRef::fixed(NodeId::fixed("p-partial"), "time"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("p-index"), "out"),
                    PortRef::fixed(NodeId::fixed("p-partial"), "index"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("p-partial"), "out"),
                    PortRef::fixed(NodeId::fixed("p-add"), "in2"),
                ),
                VoiceConnection::new(
                    PortRef::fixed(NodeId::fixed("p-add"), "out"),
                    PortRef::fixed(NodeId::fixed("p-out"), "in"),
                ),
            ],
            frames: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<(), GraphError> {
        self.validate_with_interface(&NodeGroupInterface::piano())
    }

    pub fn validate_with_interface(
        &self,
        interface: &NodeGroupInterface,
    ) -> Result<(), GraphError> {
        self.validate_with_groups(interface, &BTreeMap::new())
    }

    pub fn zone_headers(&self) -> Vec<&VoiceNode> {
        self.nodes
            .values()
            .filter(|node| node.type_id == "voice.zone.repeat")
            .collect()
    }

    fn zone_members(&self, zone_id: &str, seen: &mut BTreeSet<NodeId>) -> BTreeSet<NodeId> {
        let mut members = BTreeSet::new();
        for node in self
            .nodes
            .values()
            .filter(|node| node.zone.as_deref() == Some(zone_id))
        {
            if !seen.insert(node.id.clone()) {
                continue;
            }
            members.insert(node.id.clone());
            if node.type_id == "voice.zone.repeat" {
                members.extend(self.zone_members(node.id.as_str(), seen));
            }
        }
        members
    }

    pub fn zone_body_ids(&self) -> BTreeSet<NodeId> {
        let mut ids = BTreeSet::new();
        for header in self.zone_headers() {
            ids.extend(self.zone_members(header.id.as_str(), &mut BTreeSet::new()));
        }
        ids
    }

    pub fn zone_body(
        &self,
        header: &VoiceNode,
    ) -> Result<(VoiceGraph, NodeGroupInterface), GraphError> {
        let ids = self.zone_members(header.id.as_str(), &mut BTreeSet::new());
        let mut nodes = BTreeMap::new();
        let mut inputs = 0;
        let mut outputs = 0;
        let mut input_node = None;
        let mut output_node = None;
        for id in &ids {
            let Some(node) = self.nodes.get(id) else {
                continue;
            };
            let mut node = node.clone();
            if node.type_id == "voice.zone.input" {
                node.type_id = "group.input".to_string();
                inputs += 1;
                input_node = Some(id.clone());
            } else if node.type_id == "voice.zone.output" {
                node.type_id = "group.output".to_string();
                outputs += 1;
                output_node = Some(id.clone());
            }
            nodes.insert(id.clone(), node);
        }
        if inputs != 1 || outputs != 1 {
            return Err(GraphError::ZoneTerminals);
        }
        let input_node = input_node.ok_or(GraphError::ZoneTerminals)?;
        let output_node = output_node.ok_or(GraphError::ZoneTerminals)?;
        let mut connections: Vec<VoiceConnection> = self
            .connections
            .iter()
            .filter(|connection| {
                ids.contains(&connection.from.node) && ids.contains(&connection.to.node)
            })
            .cloned()
            .collect();
        for connection in &mut connections {
            if connection.from.node == input_node && connection.from.port == "out" {
                connection.from.port = "in".to_string();
            }
            if connection.to.node == output_node && connection.to.port == "in" {
                connection.to.port = "audio".to_string();
            }
        }
        let interface = NodeGroupInterface {
            inputs: vec![NodeGroupSocket::new(
                "in",
                "Audio",
                NodeGroupSocketType::Audio,
            )],
            outputs: vec![NodeGroupSocket::new(
                "audio",
                "Audio",
                NodeGroupSocketType::Audio,
            )],
        };
        Ok((
            VoiceGraph {
                nodes,
                connections,
                frames: BTreeMap::new(),
            },
            interface,
        ))
    }

    pub fn validate_with_groups(
        &self,
        interface: &NodeGroupInterface,
        groups: &BTreeMap<String, NodeGroup>,
    ) -> Result<(), GraphError> {
        if self.nodes.is_empty() {
            return Err(GraphError::EmptyVoiceGraph);
        }
        let body_ids = self.zone_body_ids();
        let inputs = self
            .nodes
            .values()
            .filter(|node| !body_ids.contains(&node.id) && node.type_id == "group.input")
            .count();
        let outputs = self
            .nodes
            .values()
            .filter(|node| !body_ids.contains(&node.id) && node.type_id == "group.output")
            .count();
        if inputs != 1 || outputs != 1 {
            return Err(GraphError::VoiceTerminals);
        }
        for node in self
            .nodes
            .values()
            .filter(|node| !body_ids.contains(&node.id))
        {
            if !matches!(
                node.type_id.as_str(),
                "group.input" | "group.output" | "voice.group"
            ) && voice_node_type(&node.type_id).is_none()
            {
                return Err(GraphError::UnknownVoiceNodeType(node.type_id.clone()));
            }
            if node.type_id == "voice.group" && node.group.is_none() {
                return Err(GraphError::EmptyGroupId);
            }
        }
        let mut occupied = BTreeSet::new();
        for connection in &self.connections {
            let from_body = body_ids.contains(&connection.from.node);
            let to_body = body_ids.contains(&connection.to.node);
            if from_body && to_body {
                continue;
            }
            if from_body || to_body {
                return Err(GraphError::ZoneLeak(if from_body {
                    connection.from.node.clone()
                } else {
                    connection.to.node.clone()
                }));
            }
            let source = self
                .nodes
                .get(&connection.from.node)
                .ok_or(GraphError::VoiceUnknownNode)?;
            let target = self
                .nodes
                .get(&connection.to.node)
                .ok_or(GraphError::VoiceUnknownNode)?;
            let source_ports = node_outputs(source, interface, groups)?;
            let target_ports = node_inputs(target, interface, groups)?;
            let Some(source_port) = source_ports
                .iter()
                .find(|port| port.id == connection.from.port)
            else {
                return Err(GraphError::VoiceUnknownPort(connection.from.port.clone()));
            };
            let Some(target_port) = target_ports
                .iter()
                .find(|port| port.id == connection.to.port)
            else {
                return Err(GraphError::VoiceUnknownPort(connection.to.port.clone()));
            };
            if source_port.ty != target_port.ty {
                return Err(GraphError::VoiceTypeMismatch {
                    from: source_port.ty,
                    to: target_port.ty,
                });
            }
            if connection.muted || target_port.multi {
                continue;
            }
            let key = (connection.to.node.clone(), connection.to.port.clone());
            if !occupied.insert(key) {
                return Err(GraphError::PortOccupied);
            }
        }
        for header in self.zone_headers() {
            let (body, interface) = self.zone_body(header)?;
            body.validate_with_groups(&interface, groups)?;
        }
        self.topological_order()?;
        Ok(())
    }
}

/// Voice 子图节点。节点行为由 `VoiceNodeType` 注册表声明，实例只保存类型 ID。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceNode {
    pub id: NodeId,
    pub type_id: String,
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl VoiceNode {
    /// 按节点类型创建节点并填入参数默认值。
    pub fn new(id: NodeId, type_id: &str) -> Self {
        let params = voice_node_type(type_id)
            .map(|node_type| {
                node_type
                    .params
                    .iter()
                    .map(|param| (param.name.to_string(), param.default))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            id,
            type_id: type_id.to_string(),
            params,
            position: None,
            group: None,
            frame: None,
            zone: None,
            muted: false,
            collapsed: false,
            label: None,
            color: None,
            width: None,
        }
    }

    pub fn node_type(&self) -> Option<&'static VoiceNodeType> {
        voice_node_type(&self.type_id)
    }
}

/// Voice 子图连接。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceConnection {
    pub from: PortRef,
    pub to: PortRef,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
}

impl VoiceConnection {
    /// 创建一条 Voice 连接。
    pub fn new(from: PortRef, to: PortRef) -> Self {
        Self {
            from,
            to,
            muted: false,
        }
    }

    /// 连接的来源端口。
    pub fn from(&self) -> &PortRef {
        &self.from
    }

    /// 连接的目标端口。
    pub fn to(&self) -> &PortRef {
        &self.to
    }
}

/// 插件节点引用的 CLAP 插件和可选状态。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginConfig {
    pub path: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

impl PluginConfig {
    pub fn new(path: String, id: String, state: Option<String>) -> Self {
        Self { path, id, state }
    }
}

impl Node {
    pub fn new(id: NodeId, kind: NodeKind) -> Self {
        Self {
            id,
            kind,
            params: kind.default_params(),
            position: None,
            plugin: None,
            group: None,
        }
    }

    pub fn id(&self) -> &NodeId {
        &self.id
    }

    pub fn kind(&self) -> NodeKind {
        self.kind
    }

    pub fn params(&self) -> &BTreeMap<String, f64> {
        &self.params
    }

    pub fn position(&self) -> Option<[f64; 2]> {
        self.position
    }

    pub fn plugin(&self) -> Option<&PluginConfig> {
        self.plugin.as_ref()
    }

    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    pub fn set_group(&mut self, group: impl Into<String>) -> Result<(), GraphError> {
        if self.kind != NodeKind::VoiceGroup {
            return Err(GraphError::NotGroup(self.kind));
        }
        let group = group.into();
        if group.trim().is_empty() {
            return Err(GraphError::EmptyGroupId);
        }
        self.group = Some(group);
        Ok(())
    }

    pub fn set_plugin(&mut self, plugin: Option<PluginConfig>) -> Result<(), GraphError> {
        if self.kind != NodeKind::Plugin {
            return Err(GraphError::NotPlugin(self.kind));
        }
        if let Some(plugin) = &plugin {
            if plugin.path.trim().is_empty() || plugin.id.trim().is_empty() {
                return Err(GraphError::InvalidPlugin);
            }
        }
        self.plugin = plugin;
        Ok(())
    }

    pub fn set_position(&mut self, x: f64, y: f64) -> Result<(), GraphError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(GraphError::InvalidPosition);
        }
        self.position = Some([x, y]);
        Ok(())
    }

    pub fn param(&self, name: &str) -> Option<f64> {
        self.params.get(name).copied()
    }

    pub fn set_param(&mut self, name: &str, value: f64) -> Result<(), GraphError> {
        self.validate_param(name, value)?;
        self.params.insert(name.to_string(), value);
        Ok(())
    }

    fn validate(&self) -> Result<(), GraphError> {
        if self.kind == NodeKind::Plugin && self.plugin.is_none() {
            return Err(GraphError::InvalidPlugin);
        }
        if self.kind != NodeKind::Plugin && self.plugin.is_some() {
            return Err(GraphError::NotPlugin(self.kind));
        }
        if self.kind == NodeKind::VoiceGroup && self.group.is_none() {
            return Err(GraphError::EmptyGroupId);
        }
        for (name, value) in &self.params {
            self.validate_param(name, *value)?;
        }
        Ok(())
    }

    fn validate_param(&self, name: &str, value: f64) -> Result<(), GraphError> {
        let spec = self
            .kind
            .param_specs()
            .iter()
            .find(|spec| spec.name == name)
            .ok_or_else(|| GraphError::UnknownParam(name.to_string()))?;
        if !value.is_finite() || value < spec.minimum || value > spec.maximum {
            return Err(GraphError::InvalidParam {
                name: name.to_string(),
                value,
                minimum: spec.minimum,
                maximum: spec.maximum,
            });
        }
        Ok(())
    }
}

/// 一个端口引用。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRef {
    node: NodeId,
    port: String,
}

impl PortRef {
    pub fn new(node: NodeId, port: impl Into<String>) -> Self {
        Self {
            node,
            port: port.into(),
        }
    }

    pub fn node(&self) -> &NodeId {
        &self.node
    }

    pub fn port(&self) -> &str {
        &self.port
    }

    fn fixed(node: NodeId, port: &'static str) -> Self {
        Self {
            node,
            port: port.to_string(),
        }
    }
}

/// 一条有向连接。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    from: PortRef,
    to: PortRef,
}

impl Connection {
    pub fn new(from: PortRef, to: PortRef) -> Self {
        Self { from, to }
    }

    pub fn from(&self) -> &PortRef {
        &self.from
    }

    pub fn to(&self) -> &PortRef {
        &self.to
    }
}

/// 线性图编译出的轨道摘要。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledChain {
    pub source_frequency: Option<f64>,
    pub adsr: Option<AdsrSettings>,
    pub filters: Vec<FilterSettings>,
    pub gain: f64,
}

/// 低通滤波器参数。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterSettings {
    pub cutoff: f64,
    pub resonance: f64,
}

/// 编译器看到的 ADSR 参数。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsrSettings {
    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
}

impl AdsrSettings {
    pub fn new(attack: f64, decay: f64, sustain: f64, release: f64) -> Self {
        Self {
            attack,
            decay,
            sustain,
            release,
        }
    }
}

/// 加法钢琴音色参数。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PianoSettings {
    pub partials: usize,
    pub decay: f64,
    pub brightness: f64,
    pub hammer: f64,
}

/// DAG 编译结果；索引 0 固定是声部输入。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledGraph {
    pub source_frequency: Option<f64>,
    pub adsr: Option<AdsrSettings>,
    pub nodes: Vec<CompiledNode>,
    pub output: usize,
    pub plugin: Option<PluginConfig>,
    pub group: Option<String>,
}

/// DAG 中的一个处理节点。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledNode {
    pub op: CompiledOp,
    pub inputs: Vec<usize>,
}

/// DAG 节点操作。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum CompiledOp {
    Voice,
    Filter(FilterSettings),
    Gain(f64),
    Mix,
    Plugin,
    Output,
}

impl CompiledOp {
    pub const fn is_plugin(&self) -> bool {
        matches!(self, Self::Plugin)
    }
}

/// 节点、端口和连接。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    nodes: BTreeMap<NodeId, Node>,
    connections: Vec<Connection>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// 默认轨道链：`instrument.sine -> amp.adsr -> gain -> output`。
    pub fn linear_track() -> Self {
        let mut instrument = Node::new(NodeId::fixed("instrument"), NodeKind::InstrumentSine);
        instrument.position = Some([60.0, 180.0]);
        let mut envelope = Node::new(NodeId::fixed("envelope"), NodeKind::AmpAdsr);
        envelope.position = Some([360.0, 180.0]);
        let mut gain = Node::new(NodeId::fixed("gain"), NodeKind::Gain);
        gain.position = Some([660.0, 180.0]);
        let mut output = Node::new(NodeId::fixed("output"), NodeKind::Output);
        output.position = Some([940.0, 180.0]);
        let mut nodes = BTreeMap::new();
        nodes.insert(instrument.id().clone(), instrument);
        nodes.insert(envelope.id().clone(), envelope);
        nodes.insert(gain.id().clone(), gain);
        nodes.insert(output.id().clone(), output);
        let connections = vec![
            Connection::new(
                PortRef::fixed(NodeId::fixed("instrument"), PORT_OUT),
                PortRef::fixed(NodeId::fixed("envelope"), PORT_IN),
            ),
            Connection::new(
                PortRef::fixed(NodeId::fixed("envelope"), PORT_OUT),
                PortRef::fixed(NodeId::fixed("gain"), PORT_IN),
            ),
            Connection::new(
                PortRef::fixed(NodeId::fixed("gain"), PORT_OUT),
                PortRef::fixed(NodeId::fixed("output"), PORT_IN),
            ),
        ];
        Self { nodes, connections }
    }

    /// 空轨道链：一个乐器节点直接接 `output`。
    pub fn empty_track(instrument: NodeKind) -> Result<Self, GraphError> {
        if !instrument.is_instrument() {
            return Err(GraphError::NotInstrument(instrument));
        }
        let mut instrument = Node::new(NodeId::fixed("instrument"), instrument);
        instrument.position = Some([60.0, 220.0]);
        let mut output = Node::new(NodeId::fixed("output"), NodeKind::Output);
        output.position = Some([940.0, 220.0]);
        let mut nodes = BTreeMap::new();
        nodes.insert(instrument.id().clone(), instrument);
        nodes.insert(output.id().clone(), output);
        let connections = vec![Connection::new(
            PortRef::fixed(NodeId::fixed("instrument"), PORT_OUT),
            PortRef::fixed(NodeId::fixed("output"), PORT_IN),
        )];
        Ok(Self { nodes, connections })
    }

    pub fn group_track(group: &str) -> Result<Self, GraphError> {
        let mut graph = Self::empty_track(NodeKind::VoiceGroup)?;
        graph.set_group(group)?;
        Ok(graph)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// 按链顺序返回设备节点，不含乐器和输出。
    pub fn device_nodes(&self) -> Result<Vec<&Node>, GraphError> {
        let order = self.topological_order()?;
        Ok(order
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter(|node| node.kind().is_device())
            .collect())
    }

    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn set_group(&mut self, group: &str) -> Result<(), GraphError> {
        let node = self
            .nodes
            .values_mut()
            .find(|node| node.kind() == NodeKind::VoiceGroup)
            .ok_or(GraphError::NotGroup(NodeKind::VoiceGroup))?;
        node.set_group(group)
    }

    pub fn connections(&self) -> &[Connection] {
        &self.connections
    }

    pub fn add_node(&mut self, node: Node) -> Result<(), GraphError> {
        if self.nodes.contains_key(node.id()) {
            return Err(GraphError::DuplicateNode(node.id().clone()));
        }
        self.nodes.insert(node.id().clone(), node);
        Ok(())
    }

    pub fn set_param(&mut self, id: &NodeId, name: &str, value: f64) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| GraphError::UnknownNode(id.clone()))?;
        node.set_param(name, value)
    }

    pub fn connect(&mut self, from: PortRef, to: PortRef) -> Result<(), GraphError> {
        let connection = Connection::new(from, to);
        self.validate_connection(&connection)?;
        if self.connections.contains(&connection) {
            return Err(GraphError::DuplicateConnection);
        }
        if self
            .connections
            .iter()
            .any(|existing| existing.to == connection.to)
        {
            return Err(GraphError::PortOccupied);
        }
        self.connections.push(connection);
        if self.topological_order().is_err() {
            self.connections.pop();
            return Err(GraphError::Cycle);
        }
        Ok(())
    }

    pub fn disconnect(&mut self, from: &PortRef, to: &PortRef) -> Result<(), GraphError> {
        let index = self
            .connections
            .iter()
            .position(|connection| &connection.from == from && &connection.to == to)
            .ok_or(GraphError::MissingConnection)?;
        self.connections.remove(index);
        Ok(())
    }

    /// 把设备节点插到 `output` 之前。
    pub fn insert_before_output(&mut self, node: Node) -> Result<(), GraphError> {
        let output_id = self
            .nodes
            .values()
            .find(|candidate| candidate.kind() == NodeKind::Output)
            .map(|candidate| candidate.id().clone())
            .ok_or(GraphError::OutputCount(0))?;
        self.insert_before(node, &output_id)
    }

    /// 把设备节点插到目标节点之前。
    pub fn insert_before(&mut self, node: Node, target: &NodeId) -> Result<(), GraphError> {
        if !node.kind().is_device() {
            return Err(GraphError::NotDevice(node.kind()));
        }
        if self.nodes.contains_key(node.id()) {
            return Err(GraphError::DuplicateNode(node.id().clone()));
        }
        let target_node = self
            .nodes
            .get(target)
            .ok_or_else(|| GraphError::UnknownNode(target.clone()))?;
        if !target_node.kind().has_input(PORT_IN) {
            return Err(GraphError::UnknownPort {
                node: target.clone(),
                port: PORT_IN.to_string(),
            });
        }
        let incoming = self
            .connections
            .iter()
            .find(|connection| connection.to.node == *target && connection.to.port == PORT_IN)
            .cloned()
            .ok_or_else(|| GraphError::MissingInput(target.clone()))?;
        let predecessor = incoming.from.node.clone();
        let node_id = node.id().clone();
        let node_input = node
            .kind()
            .inputs()
            .first()
            .copied()
            .ok_or(GraphError::MissingInput(node_id.clone()))?;
        self.nodes.insert(node_id.clone(), node);
        self.connections
            .retain(|connection| connection != &incoming);
        self.connect(
            PortRef::fixed(predecessor, PORT_OUT),
            PortRef::fixed(node_id.clone(), node_input),
        )?;
        self.connect(
            PortRef::fixed(node_id, PORT_OUT),
            PortRef::fixed(target.clone(), PORT_IN),
        )?;
        Ok(())
    }

    /// 记录节点在画布上的位置；位置只是编辑器元数据，不参与音频编译。
    pub fn set_position(&mut self, id: &NodeId, x: f64, y: f64) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| GraphError::UnknownNode(id.clone()))?;
        node.set_position(x, y)
    }

    /// 复制一批节点；只保留两端都在复制集合里的内部连线。
    pub fn duplicate_nodes(
        &mut self,
        ids: &[NodeId],
        new_ids: &[NodeId],
        offset: [f64; 2],
    ) -> Result<Vec<(NodeId, NodeId)>, GraphError> {
        if ids.len() != new_ids.len() {
            return Err(GraphError::DuplicateCount);
        }
        let mut mapping = BTreeMap::new();
        for (old, new) in ids.iter().zip(new_ids) {
            if self.nodes.contains_key(new) {
                return Err(GraphError::DuplicateNode(new.clone()));
            }
            let node = self
                .nodes
                .get(old)
                .ok_or_else(|| GraphError::UnknownNode(old.clone()))?;
            if node.kind().is_instrument()
                || node.kind() == NodeKind::Output
                || node.kind() == NodeKind::AmpAdsr
            {
                return Err(GraphError::NotDevice(node.kind()));
            }
            mapping.insert(old.clone(), new.clone());
        }
        for (old, new) in ids.iter().zip(new_ids) {
            let original = self
                .nodes
                .get(old)
                .ok_or_else(|| GraphError::UnknownNode(old.clone()))?;
            let mut node = Node::new(new.clone(), original.kind());
            for (name, value) in original.params() {
                node.set_param(name, *value)?;
            }
            if let Some([x, y]) = original.position() {
                node.set_position(x + offset[0], y + offset[1])?;
            }
            self.add_node(node)?;
        }
        let original_connections = self.connections.clone();
        for connection in original_connections {
            let Some(from) = mapping.get(&connection.from.node) else {
                continue;
            };
            let Some(to) = mapping.get(&connection.to.node) else {
                continue;
            };
            self.connect(
                PortRef::new(from.clone(), connection.from.port),
                PortRef::new(to.clone(), connection.to.port),
            )?;
        }
        Ok(mapping.into_iter().collect())
    }

    /// 删除节点及所有相关连线；用于无法保持链式语义的混合节点。
    pub fn remove_node(&mut self, id: &NodeId) -> Result<(), GraphError> {
        if self.nodes.remove(id).is_none() {
            return Err(GraphError::UnknownNode(id.clone()));
        }
        self.connections
            .retain(|connection| connection.from.node != *id && connection.to.node != *id);
        Ok(())
    }

    /// 把设备节点移动到目标节点之前。
    pub fn move_before(&mut self, id: &NodeId, target: &NodeId) -> Result<(), GraphError> {
        if id == target {
            return Ok(());
        }
        let target_node = self
            .nodes
            .get(target)
            .ok_or_else(|| GraphError::UnknownNode(target.clone()))?;
        if !target_node.kind().has_input(PORT_IN) {
            return Err(GraphError::UnknownPort {
                node: target.clone(),
                port: PORT_IN.to_string(),
            });
        }
        let node = self
            .nodes
            .get(id)
            .cloned()
            .ok_or_else(|| GraphError::UnknownNode(id.clone()))?;
        self.remove_chain_node(id)?;
        self.insert_before(node, target)
    }

    /// 从链中删掉一个设备节点，并把前驱直接接到后继。
    pub fn remove_chain_node(&mut self, id: &NodeId) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get(id)
            .ok_or_else(|| GraphError::UnknownNode(id.clone()))?;
        if !node.kind().is_device() || node.kind() == NodeKind::Mix {
            return Err(GraphError::NotDevice(node.kind()));
        }
        let incoming_count = self
            .connections
            .iter()
            .filter(|connection| connection.to.node == *id)
            .count();
        let outgoing_count = self
            .connections
            .iter()
            .filter(|connection| connection.from.node == *id)
            .count();
        if incoming_count != 1 || outgoing_count != 1 {
            return Err(GraphError::BranchNode(id.clone()));
        }
        let incoming = self
            .connections
            .iter()
            .find(|connection| connection.to.node == *id && connection.to.port == PORT_IN)
            .cloned()
            .ok_or_else(|| GraphError::MissingInput(id.clone()))?;
        let outgoing = self
            .connections
            .iter()
            .find(|connection| connection.from.node == *id && connection.from.port == PORT_OUT)
            .cloned()
            .ok_or_else(|| GraphError::MissingOutput(id.clone()))?;
        let predecessor = incoming.from.node.clone();
        let successor = outgoing.to.node.clone();
        self.connections
            .retain(|connection| connection != &incoming && connection != &outgoing);
        self.nodes.remove(id);
        self.connect(
            PortRef::fixed(predecessor, PORT_OUT),
            PortRef::new(successor, outgoing.to.port.clone()),
        )?;
        Ok(())
    }

    /// 校验节点参数、端口方向、扇入扇出和环。
    pub fn validate(&self) -> Result<(), GraphError> {
        if self.nodes.is_empty() {
            return Err(GraphError::EmptyGraph);
        }
        for node in self.nodes.values() {
            node.validate()?;
        }
        let instruments = self
            .nodes
            .values()
            .filter(|node| node.kind().is_instrument())
            .count();
        if instruments != 1 {
            return Err(GraphError::InstrumentCount(instruments));
        }
        let outputs = self
            .nodes
            .values()
            .filter(|node| node.kind() == NodeKind::Output)
            .count();
        if outputs != 1 {
            return Err(GraphError::OutputCount(outputs));
        }
        let envelopes = self
            .nodes
            .values()
            .filter(|node| node.kind() == NodeKind::AmpAdsr)
            .count();
        if envelopes > 1 {
            return Err(GraphError::AdsrCount(envelopes));
        }

        let mut seen = BTreeSet::new();
        let mut inputs = BTreeSet::new();
        for connection in &self.connections {
            self.validate_connection(connection)?;
            let from = (connection.from.node.clone(), connection.from.port.clone());
            let to = (connection.to.node.clone(), connection.to.port.clone());
            if !seen.insert((from, to.clone())) {
                return Err(GraphError::DuplicateConnection);
            }
            if !inputs.insert(to) {
                return Err(GraphError::PortOccupied);
            }
        }

        self.topological_order()?;
        Ok(())
    }

    /// Kahn 拓扑排序；有环时返回 `Cycle`。
    pub fn topological_order(&self) -> Result<Vec<NodeId>, GraphError> {
        let mut indegree: BTreeMap<NodeId, usize> =
            self.nodes.keys().map(|id| (id.clone(), 0)).collect();
        let mut adjacency: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
        for connection in &self.connections {
            if !self.nodes.contains_key(&connection.from.node) {
                return Err(GraphError::UnknownNode(connection.from.node.clone()));
            }
            if !self.nodes.contains_key(&connection.to.node) {
                return Err(GraphError::UnknownNode(connection.to.node.clone()));
            }
            *indegree
                .get_mut(&connection.to.node)
                .ok_or_else(|| GraphError::UnknownNode(connection.to.node.clone()))? += 1;
            adjacency
                .entry(connection.from.node.clone())
                .or_default()
                .push(connection.to.node.clone());
        }
        let mut ready: VecDeque<NodeId> = indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(id, _)| id.clone())
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(id) = ready.pop_front() {
            order.push(id.clone());
            if let Some(targets) = adjacency.get(&id) {
                for target in targets {
                    if let Some(degree) = indegree.get_mut(target) {
                        *degree -= 1;
                        if *degree == 0 {
                            ready.push_back(target.clone());
                        }
                    }
                }
            }
        }
        if order.len() != self.nodes.len() {
            return Err(GraphError::Cycle);
        }
        Ok(order)
    }

    /// 只接受当前线性链；返回编译后的参数摘要。
    pub fn compile_linear(&self) -> Result<CompiledChain, GraphError> {
        self.validate()?;
        let instrument = self
            .nodes
            .values()
            .find(|node| node.kind().is_instrument())
            .ok_or(GraphError::InstrumentCount(0))?;
        let mut current = instrument.id().clone();
        let mut visited = BTreeSet::new();
        let mut source_frequency = None;
        let mut adsr = None;
        let mut filters = Vec::new();
        let mut gain = 1.0;
        loop {
            if !visited.insert(current.clone()) {
                return Err(GraphError::Cycle);
            }
            let node = self
                .nodes
                .get(&current)
                .ok_or_else(|| GraphError::UnknownNode(current.clone()))?;
            match node.kind() {
                NodeKind::InstrumentSine => {
                    source_frequency = node.param("frequency");
                },
                NodeKind::InstrumentSilence | NodeKind::InstrumentPiano | NodeKind::VoiceGroup => {
                    source_frequency = None;
                },
                NodeKind::AmpAdsr => {
                    adsr = Some(AdsrSettings {
                        attack: node.param("attack").unwrap_or(0.01),
                        decay: node.param("decay").unwrap_or(0.12),
                        sustain: node.param("sustain").unwrap_or(0.75),
                        release: node.param("release").unwrap_or(0.18),
                    });
                },
                NodeKind::FilterLowpass => {
                    filters.push(FilterSettings {
                        cutoff: node.param("cutoff").unwrap_or(1200.0),
                        resonance: node.param("resonance").unwrap_or(0.2),
                    });
                },
                NodeKind::Gain => {
                    gain *= node.param("gain").unwrap_or(1.0);
                },
                NodeKind::Mix => return Err(GraphError::NotLinear),
                NodeKind::Plugin => {},
                NodeKind::Output => break,
            }
            let next = self
                .connections
                .iter()
                .find(|connection| connection.from.node == current)
                .map(|connection| connection.to.node.clone())
                .ok_or_else(|| GraphError::MissingOutput(current.clone()))?;
            current = next;
        }
        if visited.len() != self.nodes.len() {
            return Err(GraphError::NotLinear);
        }
        Ok(CompiledChain {
            source_frequency,
            adsr,
            filters,
            gain,
        })
    }

    /// 编译成可求值的 DAG；ADSR 必须位于分支之前。
    #[allow(clippy::too_many_lines)]
    pub fn compile_graph(&self) -> Result<CompiledGraph, GraphError> {
        self.validate()?;
        let instrument = self
            .nodes
            .values()
            .find(|node| node.kind().is_instrument())
            .ok_or(GraphError::InstrumentCount(0))?;
        let adsr = self
            .nodes
            .values()
            .find(|node| node.kind() == NodeKind::AmpAdsr);
        if let Some(adsr) = adsr {
            let outgoing: Vec<&Connection> = self
                .connections
                .iter()
                .filter(|connection| connection.from.node == *instrument.id())
                .collect();
            if outgoing.len() != 1 || outgoing[0].to.node != *adsr.id() {
                return Err(GraphError::AdsrOrder);
            }
        }
        let start = adsr.map_or_else(|| instrument.id().clone(), |adsr| adsr.id().clone());
        let mut reachable = BTreeSet::new();
        let mut queue = VecDeque::from([start.clone()]);
        while let Some(id) = queue.pop_front() {
            if !reachable.insert(id.clone()) {
                continue;
            }
            for connection in self
                .connections
                .iter()
                .filter(|connection| connection.from.node == id)
            {
                let target = &connection.to.node;
                let target_kind = self
                    .nodes
                    .get(target)
                    .ok_or_else(|| GraphError::UnknownNode(target.clone()))?
                    .kind();
                if target_kind.is_instrument() || target_kind == NodeKind::AmpAdsr {
                    return Err(GraphError::AdsrOrder);
                }
                queue.push_back(target.clone());
            }
        }
        let output_id = self
            .nodes
            .values()
            .find(|node| node.kind() == NodeKind::Output)
            .map(|node| node.id().clone())
            .ok_or(GraphError::OutputCount(0))?;
        if !reachable.contains(&output_id) {
            return Err(GraphError::NotConnected);
        }
        let mut useful = BTreeSet::new();
        let mut reverse_queue = VecDeque::from([output_id]);
        while let Some(id) = reverse_queue.pop_front() {
            if !useful.insert(id.clone()) {
                continue;
            }
            for connection in self
                .connections
                .iter()
                .filter(|connection| connection.to.node == id)
            {
                reverse_queue.push_back(connection.from.node.clone());
            }
        }
        let order = self.topological_order()?;
        let audio_ids: Vec<NodeId> = order
            .iter()
            .filter(|id| {
                reachable.contains(*id)
                    && useful.contains(*id)
                    && self.nodes.get(*id).is_some_and(|node| {
                        !node.kind().is_instrument() && node.kind() != NodeKind::AmpAdsr
                    })
            })
            .cloned()
            .collect();
        let source_frequency = match instrument.kind() {
            NodeKind::InstrumentSine => instrument.param("frequency"),
            _ => None,
        };
        let adsr = adsr.map(|adsr| AdsrSettings {
            attack: adsr.param("attack").unwrap_or(0.01),
            decay: adsr.param("decay").unwrap_or(0.12),
            sustain: adsr.param("sustain").unwrap_or(0.75),
            release: adsr.param("release").unwrap_or(0.18),
        });
        let mut indices = BTreeMap::new();
        indices.insert(start.clone(), 0);
        let mut nodes = vec![CompiledNode {
            op: CompiledOp::Voice,
            inputs: Vec::new(),
        }];
        let mut plugin: Option<PluginConfig> = None;
        let group = instrument.group().map(str::to_string);
        let mut output = 0;
        for id in audio_ids {
            let node = self
                .nodes
                .get(&id)
                .ok_or_else(|| GraphError::UnknownNode(id.clone()))?;
            let mut incoming: Vec<&Connection> = self
                .connections
                .iter()
                .filter(|connection| connection.to.node == id)
                .collect();
            incoming.sort_by(|left, right| left.to.port.cmp(&right.to.port));
            let inputs: Vec<usize> = incoming
                .iter()
                .map(|connection| {
                    indices
                        .get(&connection.from.node)
                        .copied()
                        .ok_or(GraphError::NotConnected)
                })
                .collect::<Result<_, _>>()?;
            let op = match node.kind() {
                NodeKind::FilterLowpass => CompiledOp::Filter(FilterSettings {
                    cutoff: node.param("cutoff").unwrap_or(1200.0),
                    resonance: node.param("resonance").unwrap_or(0.2),
                }),
                NodeKind::Gain => CompiledOp::Gain(node.param("gain").unwrap_or(1.0)),
                NodeKind::Mix => CompiledOp::Mix,
                NodeKind::Plugin => {
                    if let Some(config) = node.plugin() {
                        if plugin.is_some() {
                            return Err(GraphError::MultiplePlugins);
                        }
                        plugin = Some(config.clone());
                    }
                    CompiledOp::Plugin
                },
                NodeKind::Output => CompiledOp::Output,
                NodeKind::InstrumentSine
                | NodeKind::InstrumentSilence
                | NodeKind::InstrumentPiano
                | NodeKind::VoiceGroup
                | NodeKind::AmpAdsr => {
                    return Err(GraphError::AdsrOrder);
                },
            };
            if op == CompiledOp::Mix && inputs.len() != 2 {
                return Err(GraphError::MixInputs(id.clone()));
            }
            if op != CompiledOp::Mix && inputs.len() != 1 {
                return Err(GraphError::MissingInput(id.clone()));
            }
            if op == CompiledOp::Output {
                output = nodes.len();
            }
            indices.insert(id, nodes.len());
            nodes.push(CompiledNode { op, inputs });
        }
        if output == 0 {
            return Err(GraphError::OutputCount(0));
        }
        Ok(CompiledGraph {
            source_frequency,
            adsr,
            nodes,
            output,
            plugin,
            group,
        })
    }

    fn validate_connection(&self, connection: &Connection) -> Result<(), GraphError> {
        let from = self
            .nodes
            .get(&connection.from.node)
            .ok_or_else(|| GraphError::UnknownNode(connection.from.node.clone()))?;
        let to = self
            .nodes
            .get(&connection.to.node)
            .ok_or_else(|| GraphError::UnknownNode(connection.to.node.clone()))?;
        if !from.kind().has_output(&connection.from.port) {
            return Err(GraphError::UnknownPort {
                node: from.id().clone(),
                port: connection.from.port.clone(),
            });
        }
        if !to.kind().has_input(&connection.to.port) {
            return Err(GraphError::UnknownPort {
                node: to.id().clone(),
                port: connection.to.port.clone(),
            });
        }
        Ok(())
    }
}

/// 图校验与编译错误。
#[non_exhaustive]
#[derive(Debug, Error, PartialEq)]
pub enum GraphError {
    #[error("节点 ID 不合法：{0}")]
    InvalidNodeId(String),
    #[error("节点已存在：{0}")]
    DuplicateNode(NodeId),
    #[error("批量复制的源和目标数量不一致")]
    DuplicateCount,
    #[error("找不到节点：{0}")]
    UnknownNode(NodeId),
    #[error("找不到节点框：{0}")]
    FrameMissing(String),
    #[error("找不到端口：{node}.{port}")]
    UnknownPort { node: NodeId, port: String },
    #[error("找不到参数：{0}")]
    UnknownParam(String),
    #[error("参数 {name} 超出范围：{value}，允许 {minimum} 到 {maximum}")]
    InvalidParam {
        name: String,
        value: f64,
        minimum: f64,
        maximum: f64,
    },
    #[error("空图")]
    EmptyGraph,
    #[error("图必须恰好有一个乐器节点，当前 {0} 个")]
    InstrumentCount(usize),
    #[error("图必须恰好有一个输出节点，当前 {0} 个")]
    OutputCount(usize),
    #[error("图最多只能有一个 ADSR，当前 {0} 个")]
    AdsrCount(usize),
    #[error("重复连接")]
    DuplicateConnection,
    #[error("图不是线性链")]
    NotLinear,
    #[error("ADSR 必须紧跟在乐器之后，且位于分支之前")]
    AdsrOrder,
    #[error("图中有节点没有接到主信号链")]
    NotConnected,
    #[error("mix 节点必须恰好有两个输入：{0}")]
    MixInputs(NodeId),
    #[error("目标输入端口已经被占用")]
    PortOccupied,
    #[error("节点位于分支上，不能按线性链删除：{0}")]
    BranchNode(NodeId),
    #[error("节点太多，当前上限是 {0}")]
    TooManyNodes(usize),
    #[error("节点坐标必须是有限数")]
    InvalidPosition,
    #[error("找不到这条连接")]
    MissingConnection,
    #[error("Repeat Zone 必须恰好有一个 Zone Input 和一个 Zone Output")]
    ZoneTerminals,
    #[error("Repeat Zone 内部节点不能连到外部：{0}")]
    ZoneLeak(NodeId),
    #[error("图存在环")]
    Cycle,
    #[error("节点没有接上输出：{0}")]
    MissingOutput(NodeId),
    #[error("节点没有输入：{0}")]
    MissingInput(NodeId),
    #[error("节点不是乐器：{0:?}")]
    NotInstrument(NodeKind),
    #[error("节点不是设备：{0:?}")]
    NotDevice(NodeKind),
    #[error("节点不是插件节点：{0:?}")]
    NotPlugin(NodeKind),
    #[error("插件路径和 ID 不能为空")]
    InvalidPlugin,
    #[error("一个轨道最多只能有一个插件节点")]
    MultiplePlugins,
    #[error("Voice 子图不能为空")]
    EmptyVoiceGraph,
    #[error("Voice 子图必须恰好有一个 voice.input 和一个 voice.output")]
    VoiceTerminals,
    #[error("Voice 子图引用了不存在的节点")]
    VoiceUnknownNode,
    #[error("Voice 子图端口不存在：{0}")]
    VoiceUnknownPort(String),
    #[error("未知的 Voice 节点类型：{0}")]
    UnknownVoiceNodeType(String),
    #[error("Voice 端口类型不匹配：{from:?} → {to:?}")]
    VoiceTypeMismatch { from: SocketType, to: SocketType },
    #[error("节点不是钢琴：{0:?}")]
    NotPiano(NodeKind),
    #[error("节点不是组节点：{0:?}")]
    NotGroup(NodeKind),
    #[error("组节点必须引用一个节点组")]
    EmptyGroupId,
    #[error("找不到节点组：{0}")]
    GroupMissing(String),
    #[error("组接口 socket 重复：{0}")]
    DuplicateGroupSocket(String),
    #[error("组接口 socket 无效：{socket}，{reason}")]
    InvalidGroupSocket { socket: String, reason: String },
    #[error("节点组必须有一个 audio 输出")]
    GroupAudioOutput,
    #[error("节点组循环引用：{0}")]
    GroupCycle(String),
    #[error("组输入/输出节点只能出现在组内部")]
    GroupBoundaryOutsideGroup,
    #[error("Audio 组必须恰好有一个组输入和一个组输出，当前 {inputs} 个输入、{outputs} 个输出")]
    GroupBoundaryCount { inputs: usize, outputs: usize },
    #[error("未知的 Audio 节点类型：{0}")]
    UnknownAudioNodeType(String),
    #[error("Audio 图必须恰好有一个 audio.output，当前 {0} 个")]
    AudioOutputCount(usize),
    #[error("Audio 端口不存在：{0}")]
    AudioUnknownPort(String),
    #[error("Audio 端口类型不匹配：{from:?} → {to:?}")]
    AudioTypeMismatch { from: SocketType, to: SocketType },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_linear_track_validates_and_compiles() {
        let graph = Graph::linear_track();
        graph.validate().unwrap();
        let compiled = graph.compile_linear().unwrap();
        assert!((compiled.source_frequency.unwrap() - 440.0).abs() < 1e-9);
        assert!((compiled.gain - 1.0).abs() < 1e-9);
        assert!((compiled.adsr.unwrap().sustain - 0.75).abs() < 1e-9);
        assert_eq!(graph.topological_order().unwrap().len(), 4);
    }

    #[test]
    fn voice_links_are_type_checked() {
        let interface = NodeGroupInterface::piano();
        let mut graph = VoiceGraph::shell(&interface);
        let id = NodeId::new("add").unwrap();
        graph
            .nodes
            .insert(id.clone(), VoiceNode::new(id.clone(), "math.add"));
        graph.connections.push(VoiceConnection::new(
            PortRef::fixed(NodeId::fixed("voice-in"), "note"),
            PortRef::new(id.clone(), "a"),
        ));
        graph.connections.push(VoiceConnection::new(
            PortRef::new(id, "out"),
            PortRef::fixed(NodeId::fixed("out"), "audio"),
        ));
        let error = graph
            .validate_with_groups(&interface, &BTreeMap::new())
            .unwrap_err();
        assert!(matches!(error, GraphError::VoiceTypeMismatch { .. }));
    }

    #[test]
    fn multi_input_accepts_multiple_audio_links() {
        let interface = NodeGroupInterface::piano();
        let mut graph = VoiceGraph::shell(&interface);
        for id in ["noise-a", "noise-b", "sum"] {
            let node_id = NodeId::new(id).unwrap();
            let type_id = if id == "sum" {
                "voice.sum"
            } else {
                "voice.noise"
            };
            graph
                .nodes
                .insert(node_id.clone(), VoiceNode::new(node_id, type_id));
        }
        graph.connections = vec![
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("noise-a"), "out"),
                PortRef::fixed(NodeId::fixed("sum"), "in"),
            ),
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("noise-b"), "out"),
                PortRef::fixed(NodeId::fixed("sum"), "in"),
            ),
            VoiceConnection::new(
                PortRef::fixed(NodeId::fixed("sum"), "out"),
                PortRef::fixed(NodeId::fixed("out"), "audio"),
            ),
        ];
        graph
            .validate_with_groups(&interface, &BTreeMap::new())
            .unwrap();
    }

    #[test]
    fn muted_voice_link_does_not_occupy_input() {
        let mut graph = VoiceGraph::piano_default();
        let muted = graph
            .connections
            .iter_mut()
            .find(|connection| {
                connection.from.node == NodeId::fixed("env-gain")
                    && connection.to.node == NodeId::fixed("osc-vca")
            })
            .unwrap();
        muted.muted = true;
        graph.connections.push(VoiceConnection::new(
            PortRef::fixed(NodeId::fixed("time"), "out"),
            PortRef::fixed(NodeId::fixed("osc-vca"), "gain"),
        ));
        let groups = BTreeMap::from([("piano.partial".to_string(), NodeGroup::partial_default())]);
        graph
            .validate_with_groups(&NodeGroupInterface::piano(), &groups)
            .unwrap();
    }

    #[test]
    fn nested_group_must_exist() {
        let interface = NodeGroupInterface::piano();
        let mut graph = VoiceGraph::shell(&interface);
        let id = NodeId::new("nested").unwrap();
        let mut nested = VoiceNode::new(id.clone(), "voice.group");
        nested.group = Some("missing.voice".to_string());
        graph.nodes.insert(id.clone(), nested);
        graph.connections.push(VoiceConnection::new(
            PortRef::fixed(NodeId::fixed("voice-in"), "note"),
            PortRef::new(id, "note"),
        ));
        let error = graph
            .validate_with_groups(&interface, &BTreeMap::new())
            .unwrap_err();
        assert!(matches!(error, GraphError::GroupMissing(_)));
    }

    #[test]
    fn voice_parameters_are_input_sockets() {
        let node_type = voice_node_type("voice.partial").unwrap();
        assert_eq!(node_type.inputs.len(), 7);
        let inharmonicity = node_type
            .inputs
            .iter()
            .find(|socket| socket.id == "inharmonicity")
            .unwrap();
        assert!((inharmonicity.default - 0.0004).abs() < 1e-9);
        assert!(node_type.params.is_empty());
    }

    #[test]
    fn inline_zone_requires_exactly_one_boundary_pair() {
        let mut graph = VoiceGraph::piano_default();
        graph.nodes.remove(&NodeId::fixed("p-out"));
        graph.connections.retain(|connection| {
            connection.from.node != NodeId::fixed("p-out")
                && connection.to.node != NodeId::fixed("p-out")
        });
        assert!(matches!(graph.validate(), Err(GraphError::ZoneTerminals)));
    }

    #[test]
    fn inline_zone_body_cannot_connect_outside() {
        let mut graph = VoiceGraph::piano_default();
        graph.connections.push(VoiceConnection::new(
            PortRef::fixed(NodeId::fixed("p-add"), "out"),
            PortRef::fixed(NodeId::fixed("out"), "audio"),
        ));
        assert!(matches!(graph.validate(), Err(GraphError::ZoneLeak(_))));
    }

    #[test]
    fn piano_group_node_references_voice_group() {
        let graph = Graph::group_track("piano.voice").unwrap();
        graph.validate().unwrap();
        let compiled = graph.compile_graph().unwrap();
        assert_eq!(compiled.group.as_deref(), Some("piano.voice"));
        let group = NodeGroup::piano_default();
        assert_eq!(group.interface.inputs.len(), 1);
        assert_eq!(group.interface.inputs[0].ty, NodeGroupSocketType::Note);
        assert_eq!(group.graph.nodes.len(), 28);
    }

    #[test]
    fn plugin_node_compiles_with_config() {
        let mut graph = Graph::empty_track(NodeKind::InstrumentSine).unwrap();
        let instrument = NodeId::fixed("instrument");
        let output = NodeId::fixed("output");
        graph
            .disconnect(
                &PortRef::fixed(instrument.clone(), PORT_OUT),
                &PortRef::fixed(output.clone(), PORT_IN),
            )
            .unwrap();
        let mut plugin = Node::new(NodeId::fixed("plugin"), NodeKind::Plugin);
        plugin
            .set_plugin(Some(PluginConfig::new(
                "D:/Plugins/effect.clap".to_string(),
                "com.example.effect".to_string(),
                None,
            )))
            .unwrap();
        graph.add_node(plugin).unwrap();
        graph
            .connect(
                PortRef::fixed(instrument, PORT_OUT),
                PortRef::fixed(NodeId::fixed("plugin"), PORT_IN),
            )
            .unwrap();
        graph
            .connect(
                PortRef::fixed(NodeId::fixed("plugin"), PORT_OUT),
                PortRef::fixed(output, PORT_IN),
            )
            .unwrap();
        let compiled = graph.compile_graph().unwrap();
        assert_eq!(compiled.plugin.unwrap().id, "com.example.effect");
    }

    #[test]
    fn unknown_connection_is_rejected() {
        let mut graph = Graph::linear_track();
        let error = graph
            .connect(
                PortRef::new(NodeId::new("missing").unwrap(), PORT_OUT),
                PortRef::new(NodeId::new("gain").unwrap(), PORT_IN),
            )
            .unwrap_err();
        assert!(matches!(error, GraphError::UnknownNode(_)));
    }

    #[test]
    fn cycle_is_rejected() {
        let mut graph = Graph::new();
        graph
            .add_node(Node::new(NodeId::new("left").unwrap(), NodeKind::Gain))
            .unwrap();
        graph
            .add_node(Node::new(NodeId::new("right").unwrap(), NodeKind::Gain))
            .unwrap();
        graph
            .connect(
                PortRef::new(NodeId::new("left").unwrap(), PORT_OUT),
                PortRef::new(NodeId::new("right").unwrap(), PORT_IN),
            )
            .unwrap();
        let error = graph
            .connect(
                PortRef::new(NodeId::new("right").unwrap(), PORT_OUT),
                PortRef::new(NodeId::new("left").unwrap(), PORT_IN),
            )
            .unwrap_err();
        assert!(matches!(error, GraphError::Cycle));
    }

    #[test]
    fn params_are_validated() {
        let mut graph = Graph::linear_track();
        let error = graph
            .set_param(&NodeId::new("gain").unwrap(), "gain", 5.0)
            .unwrap_err();
        assert!(matches!(error, GraphError::InvalidParam { .. }));
        let error = graph
            .set_param(&NodeId::new("gain").unwrap(), "unknown", 1.0)
            .unwrap_err();
        assert!(matches!(error, GraphError::UnknownParam(_)));
    }

    #[test]
    fn empty_track_and_insert_remove_chain() {
        let mut graph = Graph::empty_track(NodeKind::InstrumentSilence).unwrap();
        graph.validate().unwrap();
        assert!(graph.compile_linear().unwrap().source_frequency.is_none());

        graph
            .insert_before_output(Node::new(
                NodeId::new("device-1").unwrap(),
                NodeKind::AmpAdsr,
            ))
            .unwrap();
        graph
            .insert_before_output(Node::new(NodeId::new("device-2").unwrap(), NodeKind::Gain))
            .unwrap();
        let devices = graph.device_nodes().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].kind(), NodeKind::AmpAdsr);
        assert_eq!(devices[1].kind(), NodeKind::Gain);
        let compiled = graph.compile_linear().unwrap();
        assert!(compiled.adsr.is_some());

        graph
            .remove_chain_node(&NodeId::new("device-1").unwrap())
            .unwrap();
        graph.validate().unwrap();
        assert_eq!(graph.device_nodes().unwrap().len(), 1);
        assert!(graph.compile_linear().unwrap().adsr.is_none());
    }

    #[test]
    fn lowpass_node_is_compiled_in_order() {
        let mut graph = Graph::linear_track();
        graph
            .insert_before_output(Node::new(
                NodeId::new("device-1").unwrap(),
                NodeKind::FilterLowpass,
            ))
            .unwrap();
        let compiled = graph.compile_linear().unwrap();
        assert_eq!(compiled.filters.len(), 1);
        assert!((compiled.filters[0].cutoff - 1200.0).abs() < 1e-9);
    }

    #[test]
    fn connect_rejects_cycles_and_occupied_inputs() {
        let mut graph = Graph::empty_track(NodeKind::InstrumentSilence).unwrap();
        graph
            .insert_before_output(Node::new(
                NodeId::new("filter").unwrap(),
                NodeKind::FilterLowpass,
            ))
            .unwrap();
        let from = PortRef::new(NodeId::new("instrument").unwrap(), PORT_OUT);
        let to = PortRef::new(NodeId::new("filter").unwrap(), PORT_IN);
        graph.disconnect(&from, &to).unwrap();
        let error = graph
            .connect(
                PortRef::new(NodeId::new("filter").unwrap(), PORT_OUT),
                PortRef::new(NodeId::new("filter").unwrap(), PORT_IN),
            )
            .unwrap_err();
        assert!(matches!(error, GraphError::Cycle));
        graph.connect(from, to).unwrap();
        let error = graph
            .connect(
                PortRef::new(NodeId::new("filter").unwrap(), PORT_OUT),
                PortRef::new(NodeId::new("filter").unwrap(), PORT_IN),
            )
            .unwrap_err();
        assert!(matches!(error, GraphError::PortOccupied));
    }

    #[test]
    fn unconnected_devices_are_ignored_by_compiler() {
        let mut graph = Graph::linear_track();
        graph
            .add_node(Node::new(
                NodeId::new("floating").unwrap(),
                NodeKind::FilterLowpass,
            ))
            .unwrap();
        graph
            .add_node(Node::new(
                NodeId::new("floating-mix").unwrap(),
                NodeKind::Mix,
            ))
            .unwrap();
        graph.validate().unwrap();
        let compiled = graph.compile_graph().unwrap();
        assert!(compiled.nodes.iter().all(|node| node.op != CompiledOp::Mix));
    }

    #[test]
    fn mix_branch_compiles_to_dag() {
        let mut graph = Graph::empty_track(NodeKind::InstrumentSilence).unwrap();
        graph
            .insert_before_output(Node::new(
                NodeId::new("filter").unwrap(),
                NodeKind::FilterLowpass,
            ))
            .unwrap();
        graph
            .insert_before_output(Node::new(NodeId::new("mix").unwrap(), NodeKind::Mix))
            .unwrap();
        graph
            .connect(
                PortRef::new(NodeId::new("instrument").unwrap(), PORT_OUT),
                PortRef::new(NodeId::new("mix").unwrap(), "in2"),
            )
            .unwrap();
        graph.validate().unwrap();
        let compiled = graph.compile_graph().unwrap();
        assert!(compiled.nodes.iter().any(|node| node.op == CompiledOp::Mix));
        assert_eq!(compiled.nodes.len(), 4);
        assert_eq!(compiled.source_frequency, None);
        graph
            .remove_chain_node(&NodeId::new("filter").unwrap())
            .unwrap();
        graph.validate().unwrap();
    }

    #[test]
    fn graph_round_trips_through_json() {
        let graph = Graph::linear_track();
        let encoded = serde_json::to_string(&graph).unwrap();
        let decoded: Graph = serde_json::from_str(&encoded).unwrap();
        assert_eq!(graph, decoded);
    }

    #[test]
    fn voice_metadata_round_trips_through_json() {
        let mut graph = VoiceGraph::piano_default();
        graph
            .nodes
            .get_mut(&NodeId::fixed("osc-vca"))
            .unwrap()
            .label = Some("Amp".to_string());
        graph
            .nodes
            .get_mut(&NodeId::fixed("osc-vca"))
            .unwrap()
            .color = Some("#336699".to_string());
        graph
            .nodes
            .get_mut(&NodeId::fixed("osc-vca"))
            .unwrap()
            .width = Some(260.0);
        graph.frames.insert(
            "frame-test".to_string(),
            VoiceFrame {
                id: "frame-test".to_string(),
                name: "Test".to_string(),
                position: [0.0, 0.0],
                size: [100.0, 100.0],
                color: Some("#884422".to_string()),
            },
        );
        graph.connections[0].muted = true;
        let encoded = serde_json::to_string(&graph).unwrap();
        let decoded: VoiceGraph = serde_json::from_str(&encoded).unwrap();
        assert_eq!(graph, decoded);
    }
}
