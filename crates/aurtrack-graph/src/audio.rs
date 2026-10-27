//! 职责：
//! - 声明 Audio 域的节点类型、端口类型和默认值
//! - 校验类型化连线和 DAG
//!
//! 禁止：
//! - 依赖 IO、宿主或 UI
//! - 把节点行为写成固定枚举分支

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{
    GraphError, NodeGroupInterface, NodeId, NodeKind, ParamSpec, PluginConfig, PortRef, SocketType,
    VoiceFrame,
};

/// Audio 节点端口声明。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioSocketSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub ty: SocketType,
    pub default: f64,
}

/// Audio 节点的运行时端口。静态节点从类型声明转换，组实例从接口生成。
#[derive(Debug, Clone, PartialEq)]
pub struct AudioPort {
    pub id: String,
    pub name: String,
    pub ty: SocketType,
    pub default: f64,
}

fn audio_port_from_socket(socket: &crate::NodeGroupSocket) -> AudioPort {
    AudioPort {
        id: socket.id.clone(),
        name: socket.name.clone(),
        ty: socket.ty,
        default: socket.effective_default().unwrap_or(0.0),
    }
}

impl From<AudioSocketSpec> for AudioPort {
    fn from(spec: AudioSocketSpec) -> Self {
        Self {
            id: spec.id.to_string(),
            name: spec.name.to_string(),
            ty: spec.ty,
            default: spec.default,
        }
    }
}

/// Audio 节点类型声明。
#[derive(Debug, Clone, Copy)]
pub struct AudioNodeType {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub inputs: &'static [AudioSocketSpec],
    pub outputs: &'static [AudioSocketSpec],
    pub params: &'static [ParamSpec],
}

const AUDIO_OUT: [AudioSocketSpec; 1] = [AudioSocketSpec {
    id: "out",
    name: "Audio",
    ty: SocketType::Audio,
    default: 0.0,
}];
const FLOAT_OUT: [AudioSocketSpec; 1] = [AudioSocketSpec {
    id: "out",
    name: "Value",
    ty: SocketType::Float,
    default: 0.0,
}];
const GAIN_PARAMS: [ParamSpec; 1] = [ParamSpec {
    name: "gain",
    default: 1.0,
    minimum: 0.0,
    maximum: 4.0,
}];
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
        maximum: 1.0,
    },
];
const DRIVE_PARAMS: [ParamSpec; 1] = [ParamSpec {
    name: "drive",
    default: 1.0,
    minimum: 0.0,
    maximum: 10.0,
}];
const DELAY_PARAMS: [ParamSpec; 2] = [
    ParamSpec {
        name: "time",
        default: 0.25,
        minimum: 0.001,
        maximum: 2.0,
    },
    ParamSpec {
        name: "feedback",
        default: 0.2,
        minimum: 0.0,
        maximum: 0.95,
    },
];

const GAIN_INPUTS: [AudioSocketSpec; 2] = [
    AudioSocketSpec {
        id: "in",
        name: "Audio",
        ty: SocketType::Audio,
        default: 0.0,
    },
    AudioSocketSpec {
        id: "gain",
        name: "Gain",
        ty: SocketType::Float,
        default: 1.0,
    },
];
const FILTER_INPUTS: [AudioSocketSpec; 3] = [
    AudioSocketSpec {
        id: "in",
        name: "Audio",
        ty: SocketType::Audio,
        default: 0.0,
    },
    AudioSocketSpec {
        id: "cutoff",
        name: "Cutoff",
        ty: SocketType::Float,
        default: 1200.0,
    },
    AudioSocketSpec {
        id: "resonance",
        name: "Resonance",
        ty: SocketType::Float,
        default: 0.2,
    },
];
const MIX_INPUTS: [AudioSocketSpec; 1] = [AudioSocketSpec {
    id: "in",
    name: "Audio",
    ty: SocketType::Audio,
    default: 0.0,
}];

/// Audio 域节点类型注册表。
pub const AUDIO_NODE_TYPES: &[AudioNodeType] = &[
    AudioNodeType {
        id: "audio.group.instance",
        name: "Group",
        category: "Group",
        inputs: &[],
        outputs: &[],
        params: &[],
    },
    AudioNodeType {
        id: "audio.group.input",
        name: "Group Input",
        category: "Interface",
        inputs: &[],
        outputs: &[],
        params: &[],
    },
    AudioNodeType {
        id: "audio.group.output",
        name: "Group Output",
        category: "Interface",
        inputs: &[],
        outputs: &[],
        params: &[],
    },
    AudioNodeType {
        id: "audio.group",
        name: "Voice Group",
        category: "Instrument",
        inputs: &[],
        outputs: &AUDIO_OUT,
        params: &[],
    },
    AudioNodeType {
        id: "audio.instrument.sine",
        name: "Sine",
        category: "Instrument",
        inputs: &[],
        outputs: &AUDIO_OUT,
        params: &[ParamSpec {
            name: "frequency",
            default: 440.0,
            minimum: 20.0,
            maximum: 20_000.0,
        }],
    },
    AudioNodeType {
        id: "audio.instrument.silence",
        name: "Silence",
        category: "Instrument",
        inputs: &[],
        outputs: &AUDIO_OUT,
        params: &[],
    },
    AudioNodeType {
        id: "audio.adsr",
        name: "ADSR",
        category: "Envelope",
        inputs: &[AudioSocketSpec {
            id: "in",
            name: "Audio",
            ty: SocketType::Audio,
            default: 0.0,
        }],
        outputs: &AUDIO_OUT,
        params: &[
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
        ],
    },
    AudioNodeType {
        id: "audio.plugin",
        name: "Plugin",
        category: "Plugin",
        inputs: &[AudioSocketSpec {
            id: "in",
            name: "Audio",
            ty: SocketType::Audio,
            default: 0.0,
        }],
        outputs: &AUDIO_OUT,
        params: &[],
    },
    AudioNodeType {
        id: "audio.output",
        name: "Output",
        category: "Output",
        inputs: &[AudioSocketSpec {
            id: "in",
            name: "Audio",
            ty: SocketType::Audio,
            default: 0.0,
        }],
        outputs: &[],
        params: &[],
    },
    AudioNodeType {
        id: "audio.gain",
        name: "Gain",
        category: "Audio",
        inputs: &GAIN_INPUTS,
        outputs: &AUDIO_OUT,
        params: &GAIN_PARAMS,
    },
    AudioNodeType {
        id: "audio.mix",
        name: "Mix",
        category: "Audio",
        inputs: &MIX_INPUTS,
        outputs: &AUDIO_OUT,
        params: &[],
    },
    AudioNodeType {
        id: "audio.filter.lowpass",
        name: "Lowpass",
        category: "Filter",
        inputs: &FILTER_INPUTS,
        outputs: &AUDIO_OUT,
        params: &FILTER_PARAMS,
    },
    AudioNodeType {
        id: "audio.filter.highpass",
        name: "Highpass",
        category: "Filter",
        inputs: &FILTER_INPUTS,
        outputs: &AUDIO_OUT,
        params: &FILTER_PARAMS,
    },
    AudioNodeType {
        id: "audio.drive",
        name: "Drive",
        category: "Audio",
        inputs: &[
            AudioSocketSpec {
                id: "in",
                name: "Audio",
                ty: SocketType::Audio,
                default: 0.0,
            },
            AudioSocketSpec {
                id: "drive",
                name: "Drive",
                ty: SocketType::Float,
                default: 1.0,
            },
        ],
        outputs: &AUDIO_OUT,
        params: &DRIVE_PARAMS,
    },
    AudioNodeType {
        id: "audio.delay",
        name: "Delay",
        category: "Audio",
        inputs: &[
            AudioSocketSpec {
                id: "in",
                name: "Audio",
                ty: SocketType::Audio,
                default: 0.0,
            },
            AudioSocketSpec {
                id: "time",
                name: "Time",
                ty: SocketType::Float,
                default: 0.25,
            },
            AudioSocketSpec {
                id: "feedback",
                name: "Feedback",
                ty: SocketType::Float,
                default: 0.2,
            },
        ],
        outputs: &AUDIO_OUT,
        params: &DELAY_PARAMS,
    },
    AudioNodeType {
        id: "audio.value",
        name: "Value",
        category: "Input",
        inputs: &[],
        outputs: &FLOAT_OUT,
        params: &[ParamSpec {
            name: "value",
            default: 0.0,
            minimum: -1000.0,
            maximum: 1000.0,
        }],
    },
    AudioNodeType {
        id: "audio.lfo",
        name: "LFO",
        category: "Control",
        inputs: &[],
        outputs: &FLOAT_OUT,
        params: &[
            ParamSpec {
                name: "rate",
                default: 1.0,
                minimum: 0.01,
                maximum: 40.0,
            },
            ParamSpec {
                name: "depth",
                default: 0.5,
                minimum: 0.0,
                maximum: 10.0,
            },
            ParamSpec {
                name: "offset",
                default: 0.0,
                minimum: -10.0,
                maximum: 10.0,
            },
        ],
    },
    AudioNodeType {
        id: "audio.math.add",
        name: "Add",
        category: "Control",
        inputs: &[
            AudioSocketSpec {
                id: "a",
                name: "A",
                ty: SocketType::Float,
                default: 0.0,
            },
            AudioSocketSpec {
                id: "b",
                name: "B",
                ty: SocketType::Float,
                default: 0.0,
            },
        ],
        outputs: &FLOAT_OUT,
        params: &[],
    },
    AudioNodeType {
        id: "audio.math.multiply",
        name: "Multiply",
        category: "Control",
        inputs: &[
            AudioSocketSpec {
                id: "a",
                name: "A",
                ty: SocketType::Float,
                default: 1.0,
            },
            AudioSocketSpec {
                id: "b",
                name: "B",
                ty: SocketType::Float,
                default: 1.0,
            },
        ],
        outputs: &FLOAT_OUT,
        params: &[],
    },
    AudioNodeType {
        id: "audio.math.map_range",
        name: "Map Range",
        category: "Control",
        inputs: &[
            AudioSocketSpec {
                id: "value",
                name: "Value",
                ty: SocketType::Float,
                default: 0.0,
            },
            AudioSocketSpec {
                id: "from_min",
                name: "From Min",
                ty: SocketType::Float,
                default: 0.0,
            },
            AudioSocketSpec {
                id: "from_max",
                name: "From Max",
                ty: SocketType::Float,
                default: 1.0,
            },
            AudioSocketSpec {
                id: "to_min",
                name: "To Min",
                ty: SocketType::Float,
                default: 0.0,
            },
            AudioSocketSpec {
                id: "to_max",
                name: "To Max",
                ty: SocketType::Float,
                default: 1.0,
            },
        ],
        outputs: &FLOAT_OUT,
        params: &[],
    },
    AudioNodeType {
        id: "audio.math.clamp",
        name: "Clamp",
        category: "Control",
        inputs: &[
            AudioSocketSpec {
                id: "value",
                name: "Value",
                ty: SocketType::Float,
                default: 0.0,
            },
            AudioSocketSpec {
                id: "min",
                name: "Min",
                ty: SocketType::Float,
                default: 0.0,
            },
            AudioSocketSpec {
                id: "max",
                name: "Max",
                ty: SocketType::Float,
                default: 1.0,
            },
        ],
        outputs: &FLOAT_OUT,
        params: &[],
    },
];

pub fn audio_node_type(id: &str) -> Option<&'static AudioNodeType> {
    AUDIO_NODE_TYPES.iter().find(|node_type| node_type.id == id)
}

/// 多输入节点允许同一个 socket 接多条连线。
pub fn audio_multi_input(id: &str) -> bool {
    id == "audio.mix"
}

/// Audio 图中的一个节点。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioNode {
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
    pub plugin: Option<PluginConfig>,
    #[serde(default, skip_serializing_if = "crate::is_false")]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "crate::is_false")]
    pub collapsed: bool,
}

impl AudioNode {
    pub fn new(id: NodeId, type_id: &str) -> Self {
        let params = audio_node_type(type_id)
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
            plugin: None,
            muted: false,
            collapsed: false,
        }
    }

    pub fn id(&self) -> &NodeId {
        &self.id
    }

    pub fn type_id(&self) -> &str {
        &self.type_id
    }

    pub fn params(&self) -> &BTreeMap<String, f64> {
        &self.params
    }

    pub fn position(&self) -> Option<[f64; 2]> {
        self.position
    }

    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    pub fn plugin(&self) -> Option<&PluginConfig> {
        self.plugin.as_ref()
    }

    pub fn is_instrument(&self) -> bool {
        matches!(
            self.type_id.as_str(),
            "audio.instrument.sine" | "audio.instrument.silence" | "audio.group"
        )
    }

    pub fn kind(&self) -> NodeKind {
        match self.type_id.as_str() {
            "audio.instrument.sine" => NodeKind::InstrumentSine,
            "audio.instrument.silence" => NodeKind::InstrumentSilence,
            "audio.group" => NodeKind::VoiceGroup,
            "audio.adsr" => NodeKind::AmpAdsr,
            "audio.gain" | "audio.drive" | "audio.delay" => NodeKind::Gain,
            "audio.mix" => NodeKind::Mix,
            "audio.filter.lowpass" | "audio.filter.highpass" => NodeKind::FilterLowpass,
            "audio.plugin" => NodeKind::Plugin,
            _ => NodeKind::Output,
        }
    }

    pub fn is_device(&self) -> bool {
        matches!(
            self.type_id.as_str(),
            "audio.adsr"
                | "audio.gain"
                | "audio.mix"
                | "audio.filter.lowpass"
                | "audio.filter.highpass"
                | "audio.drive"
                | "audio.delay"
                | "audio.plugin"
        )
    }

    pub fn param(&self, name: &str) -> Option<f64> {
        self.params.get(name).copied()
    }

    pub fn set_param(&mut self, name: &str, value: f64) -> Result<(), GraphError> {
        let node_type = audio_node_type(&self.type_id)
            .ok_or_else(|| GraphError::UnknownAudioNodeType(self.type_id.clone()))?;
        let spec = node_type
            .params
            .iter()
            .find(|param| param.name == name)
            .ok_or_else(|| GraphError::UnknownParam(name.to_string()))?;
        if !value.is_finite() || value < spec.minimum || value > spec.maximum {
            return Err(GraphError::InvalidParam {
                name: name.to_string(),
                value,
                minimum: spec.minimum,
                maximum: spec.maximum,
            });
        }
        self.params.insert(name.to_string(), value);
        Ok(())
    }

    pub fn set_position(&mut self, x: f64, y: f64) -> Result<(), GraphError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(GraphError::InvalidPosition);
        }
        self.position = Some([x, y]);
        Ok(())
    }

    pub fn set_plugin(&mut self, plugin: Option<PluginConfig>) -> Result<(), GraphError> {
        if self.type_id != "audio.plugin" {
            return Err(GraphError::NotPlugin(self.kind()));
        }
        self.plugin = plugin;
        Ok(())
    }
}

/// Audio 图连接。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioConnection {
    pub from: PortRef,
    pub to: PortRef,
}

impl AudioConnection {
    pub fn new(from: PortRef, to: PortRef) -> Self {
        Self { from, to }
    }
}

/// 类型化 Audio 图。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioGraph {
    pub nodes: BTreeMap<NodeId, AudioNode>,
    pub connections: Vec<AudioConnection>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub frames: BTreeMap<String, VoiceFrame>,
}

/// 工程级 Audio 节点组。接口和 Audio 图分开存放。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioGroup {
    pub id: String,
    pub name: String,
    pub interface: NodeGroupInterface,
    pub graph: AudioGraph,
}

impl AudioGroup {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        interface: NodeGroupInterface,
        graph: AudioGraph,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            interface,
            graph,
        }
    }

    /// 空 Audio 组：只有组输入/输出边界，接口默认是单音频直通。
    pub fn empty(id: impl Into<String>, name: impl Into<String>) -> Self {
        let interface = NodeGroupInterface {
            inputs: vec![crate::NodeGroupSocket::new(
                "in",
                "Audio",
                SocketType::Audio,
            )],
            outputs: vec![crate::NodeGroupSocket::new(
                "out",
                "Audio",
                SocketType::Audio,
            )],
        };
        let input = NodeId::fixed("group-input");
        let output = NodeId::fixed("group-output");
        let mut nodes = BTreeMap::new();
        nodes.insert(
            input.clone(),
            AudioNode::new(input.clone(), "audio.group.input"),
        );
        nodes.insert(
            output.clone(),
            AudioNode::new(output.clone(), "audio.group.output"),
        );
        let graph = AudioGraph {
            nodes,
            connections: vec![AudioConnection::new(
                PortRef::new(input, "in"),
                PortRef::new(output, "out"),
            )],
            frames: BTreeMap::new(),
        };
        Self {
            id: id.into(),
            name: name.into(),
            interface,
            graph,
        }
    }
}

impl AudioGraph {
    pub fn empty_track(group: &str) -> Self {
        let mut instrument = AudioNode::new(NodeId::fixed("instrument"), "audio.group");
        instrument.group = Some(group.to_string());
        instrument.position = Some([60.0, 220.0]);
        let mut output = AudioNode::new(NodeId::fixed("output"), "audio.output");
        output.position = Some([940.0, 220.0]);
        let mut nodes = BTreeMap::new();
        nodes.insert(instrument.id.clone(), instrument);
        nodes.insert(output.id.clone(), output);
        Self {
            nodes,
            connections: vec![AudioConnection {
                from: PortRef::fixed(NodeId::fixed("instrument"), "out"),
                to: PortRef::fixed(NodeId::fixed("output"), "in"),
            }],
            frames: BTreeMap::new(),
        }
    }

    pub fn track(instrument: &str) -> Self {
        let mut node = AudioNode::new(NodeId::fixed("instrument"), instrument);
        node.position = Some([60.0, 220.0]);
        let mut output = AudioNode::new(NodeId::fixed("output"), "audio.output");
        output.position = Some([940.0, 220.0]);
        let mut nodes = BTreeMap::new();
        nodes.insert(node.id.clone(), node);
        nodes.insert(output.id.clone(), output);
        Self {
            nodes,
            connections: vec![AudioConnection {
                from: PortRef::fixed(NodeId::fixed("instrument"), "out"),
                to: PortRef::fixed(NodeId::fixed("output"), "in"),
            }],
            frames: BTreeMap::new(),
        }
    }

    pub fn plugin(&self) -> Option<&PluginConfig> {
        self.nodes
            .values()
            .find(|node| node.type_id == "audio.plugin")
            .and_then(|node| node.plugin())
    }

    pub fn node(&self, id: &NodeId) -> Option<&AudioNode> {
        self.nodes.get(id)
    }

    /// 替换节点框，并按需更新节点归属。
    pub fn set_frames(
        &mut self,
        frames: BTreeMap<String, VoiceFrame>,
        assignments: BTreeMap<String, Option<String>>,
    ) -> Result<(), GraphError> {
        for (id, frame) in &frames {
            NodeId::new(id.clone())?;
            if frame.id != *id
                || !frame.size[0].is_finite()
                || !frame.size[1].is_finite()
                || frame.size[0] <= 0.0
                || frame.size[1] <= 0.0
                || !frame.position[0].is_finite()
                || !frame.position[1].is_finite()
            {
                return Err(GraphError::InvalidPosition);
            }
        }
        for (node, frame) in &assignments {
            let id = NodeId::new(node.clone())?;
            if !self.nodes.contains_key(&id) {
                return Err(GraphError::UnknownNode(id));
            }
            if let Some(frame) = frame {
                if !frames.contains_key(frame) {
                    return Err(GraphError::FrameMissing(frame.clone()));
                }
            }
        }
        self.frames = frames;
        for (node, frame) in assignments {
            let id = NodeId::new(node)?;
            if let Some(node) = self.nodes.get_mut(&id) {
                node.frame = frame;
            }
        }
        let frames = &self.frames;
        for node in self.nodes.values_mut() {
            if node
                .frame
                .as_ref()
                .is_some_and(|frame| !frames.contains_key(frame))
            {
                node.frame = None;
            }
        }
        Ok(())
    }

    /// 修改节点的显示状态。
    pub fn set_flags(
        &mut self,
        id: &NodeId,
        muted: Option<bool>,
        collapsed: Option<bool>,
    ) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| GraphError::UnknownNode(id.clone()))?;
        if let Some(muted) = muted {
            node.muted = muted;
        }
        if let Some(collapsed) = collapsed {
            node.collapsed = collapsed;
        }
        Ok(())
    }

    pub fn nodes(&self) -> impl Iterator<Item = &AudioNode> {
        self.nodes.values()
    }

    pub fn device_nodes(&self) -> Result<Vec<&AudioNode>, GraphError> {
        let order = self.topological_order()?;
        Ok(order
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter(|node| node.is_device())
            .collect())
    }

    pub fn add_node(&mut self, node: AudioNode) -> Result<(), GraphError> {
        if self.nodes.contains_key(&node.id) {
            return Err(GraphError::DuplicateNode(node.id));
        }
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn insert_before_output(&mut self, node: AudioNode) -> Result<(), GraphError> {
        let node_id = node.id.clone();
        let input_port = Self::inputs(&node)?
            .into_iter()
            .find(|port| port.ty == SocketType::Audio)
            .ok_or_else(|| GraphError::AudioUnknownPort("in".to_string()))?
            .id;
        self.add_node(node)?;
        let output_id = NodeId::fixed("output");
        let previous = self
            .connections
            .iter()
            .find(|connection| connection.to.node == output_id)
            .map(|connection| connection.from.clone());
        self.connections
            .retain(|connection| connection.to.node != output_id);
        if let Some(previous) = previous {
            self.connections.push(AudioConnection::new(
                previous,
                PortRef::new(node_id.clone(), input_port),
            ));
        }
        self.connections.push(AudioConnection::new(
            PortRef::fixed(node_id, "out"),
            PortRef::fixed(output_id, "in"),
        ));
        Ok(())
    }

    pub fn remove_chain_node(&mut self, id: &NodeId) -> Result<(), GraphError> {
        let previous = self
            .connections
            .iter()
            .find(|connection| &connection.to.node == id)
            .map(|connection| connection.from.clone());
        let next = self
            .connections
            .iter()
            .find(|connection| &connection.from.node == id)
            .map(|connection| connection.to.clone());
        self.remove_node(id)?;
        if let (Some(previous), Some(next)) = (previous, next) {
            self.connections.push(AudioConnection::new(previous, next));
        }
        Ok(())
    }

    pub fn move_before(&mut self, id: &NodeId, target: &NodeId) -> Result<(), GraphError> {
        if id == target {
            return Ok(());
        }
        let previous = self
            .connections
            .iter()
            .find(|connection| &connection.to.node == id)
            .map(|connection| connection.from.clone());
        let next = self
            .connections
            .iter()
            .find(|connection| &connection.from.node == id)
            .map(|connection| connection.to.clone());
        let node = self
            .nodes
            .remove(id)
            .ok_or_else(|| GraphError::UnknownNode(id.clone()))?;
        self.connections
            .retain(|connection| &connection.from.node != id && &connection.to.node != id);
        if let (Some(previous), Some(next)) = (previous, next) {
            self.connections.push(AudioConnection::new(previous, next));
        }
        let target_input = self
            .connections
            .iter()
            .find(|connection| &connection.to.node == target)
            .map(|connection| connection.from.clone())
            .ok_or(GraphError::NotConnected)?;
        self.connections
            .retain(|connection| connection.to.node != *target);
        self.connections.push(AudioConnection::new(
            target_input,
            PortRef::fixed(id.clone(), "in"),
        ));
        self.connections.push(AudioConnection::new(
            PortRef::fixed(id.clone(), "out"),
            PortRef::fixed(target.clone(), "in"),
        ));
        self.nodes.insert(id.clone(), node);
        Ok(())
    }

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
            let mut node = self
                .nodes
                .get(old)
                .ok_or_else(|| GraphError::UnknownNode(old.clone()))?
                .clone();
            node.id = new.clone();
            if let Some(position) = node.position {
                node.position = Some([position[0] + offset[0], position[1] + offset[1]]);
            }
            mapping.insert(old.clone(), new.clone());
            self.nodes.insert(new.clone(), node);
        }
        for connection in self.connections.clone() {
            let Some(from) = mapping.get(&connection.from.node) else {
                continue;
            };
            let Some(to) = mapping.get(&connection.to.node) else {
                continue;
            };
            self.connections.push(AudioConnection {
                from: PortRef::new(from.clone(), connection.from.port.clone()),
                to: PortRef::new(to.clone(), connection.to.port.clone()),
            });
        }
        Ok(mapping.into_iter().collect())
    }

    pub fn remove_node(&mut self, id: &NodeId) -> Result<(), GraphError> {
        if self.nodes.remove(id).is_none() {
            return Err(GraphError::UnknownNode(id.clone()));
        }
        self.connections
            .retain(|connection| &connection.from.node != id && &connection.to.node != id);
        Ok(())
    }

    pub fn set_param(&mut self, id: &NodeId, name: &str, value: f64) -> Result<(), GraphError> {
        self.nodes
            .get_mut(id)
            .ok_or_else(|| GraphError::UnknownNode(id.clone()))?
            .set_param(name, value)
    }

    pub fn set_position(&mut self, id: &NodeId, x: f64, y: f64) -> Result<(), GraphError> {
        self.nodes
            .get_mut(id)
            .ok_or_else(|| GraphError::UnknownNode(id.clone()))?
            .set_position(x, y)
    }

    pub fn connect(&mut self, from: PortRef, to: PortRef) -> Result<(), GraphError> {
        if !self.nodes.contains_key(&from.node) {
            return Err(GraphError::UnknownNode(from.node.clone()));
        }
        if !self.nodes.contains_key(&to.node) {
            return Err(GraphError::UnknownNode(to.node.clone()));
        }
        self.connections.push(AudioConnection::new(from, to));
        Ok(())
    }

    pub fn disconnect(&mut self, from: &PortRef, to: &PortRef) -> Result<(), GraphError> {
        let before = self.connections.len();
        self.connections
            .retain(|connection| &connection.from != from || &connection.to != to);
        if self.connections.len() == before {
            return Err(GraphError::NotConnected);
        }
        Ok(())
    }

    pub fn inputs(node: &AudioNode) -> Result<Vec<AudioPort>, GraphError> {
        audio_node_type(&node.type_id)
            .map(|node_type| {
                node_type
                    .inputs
                    .iter()
                    .copied()
                    .map(AudioPort::from)
                    .collect()
            })
            .ok_or_else(|| GraphError::UnknownAudioNodeType(node.type_id.clone()))
    }

    pub fn outputs(node: &AudioNode) -> Result<Vec<AudioPort>, GraphError> {
        audio_node_type(&node.type_id)
            .map(|node_type| {
                node_type
                    .outputs
                    .iter()
                    .copied()
                    .map(AudioPort::from)
                    .collect()
            })
            .ok_or_else(|| GraphError::UnknownAudioNodeType(node.type_id.clone()))
    }

    /// 解析 Audio 端口，组实例和组边界按接口动态生成。
    pub fn inputs_with_groups(
        node: &AudioNode,
        groups: &BTreeMap<String, AudioGroup>,
        interface: Option<&NodeGroupInterface>,
    ) -> Result<Vec<AudioPort>, GraphError> {
        match node.type_id.as_str() {
            "audio.group.instance" => {
                let group = groups
                    .get(node.group.as_deref().ok_or(GraphError::EmptyGroupId)?)
                    .ok_or_else(|| {
                        GraphError::GroupMissing(node.group.clone().unwrap_or_default())
                    })?;
                Ok(group
                    .interface
                    .inputs
                    .iter()
                    .map(audio_port_from_socket)
                    .collect())
            },
            "audio.group.input" => Ok(Vec::new()),
            "audio.group.output" => Ok(interface
                .ok_or(GraphError::GroupAudioOutput)?
                .outputs
                .iter()
                .map(audio_port_from_socket)
                .collect()),
            _ => Self::inputs(node),
        }
    }

    /// 解析 Audio 输出端口，组实例和组边界按接口动态生成。
    pub fn outputs_with_groups(
        node: &AudioNode,
        groups: &BTreeMap<String, AudioGroup>,
        interface: Option<&NodeGroupInterface>,
    ) -> Result<Vec<AudioPort>, GraphError> {
        match node.type_id.as_str() {
            "audio.group.instance" => {
                let group = groups
                    .get(node.group.as_deref().ok_or(GraphError::EmptyGroupId)?)
                    .ok_or_else(|| {
                        GraphError::GroupMissing(node.group.clone().unwrap_or_default())
                    })?;
                Ok(group
                    .interface
                    .outputs
                    .iter()
                    .map(audio_port_from_socket)
                    .collect())
            },
            "audio.group.input" => Ok(interface
                .ok_or(GraphError::GroupAudioOutput)?
                .inputs
                .iter()
                .map(audio_port_from_socket)
                .collect()),
            "audio.group.output" => Ok(Vec::new()),
            _ => Self::outputs(node),
        }
    }

    pub fn validate(&self, groups: &BTreeMap<String, crate::NodeGroup>) -> Result<(), GraphError> {
        self.validate_inner(groups, &BTreeMap::new(), None, &mut Vec::new())
    }

    pub fn validate_with_audio_groups(
        &self,
        voice_groups: &BTreeMap<String, crate::NodeGroup>,
        audio_groups: &BTreeMap<String, AudioGroup>,
    ) -> Result<(), GraphError> {
        self.validate_inner(voice_groups, audio_groups, None, &mut Vec::new())
    }

    pub fn validate_with_audio_interface(
        &self,
        voice_groups: &BTreeMap<String, crate::NodeGroup>,
        audio_groups: &BTreeMap<String, AudioGroup>,
        interface: &NodeGroupInterface,
    ) -> Result<(), GraphError> {
        self.validate_inner(voice_groups, audio_groups, Some(interface), &mut Vec::new())
    }

    #[allow(clippy::too_many_lines)]
    fn validate_inner(
        &self,
        voice_groups: &BTreeMap<String, crate::NodeGroup>,
        audio_groups: &BTreeMap<String, AudioGroup>,
        interface: Option<&NodeGroupInterface>,
        stack: &mut Vec<String>,
    ) -> Result<(), GraphError> {
        let mut outputs = 0;
        let mut group_inputs = 0;
        let mut group_outputs = 0;
        for node in self.nodes.values() {
            let node_type = audio_node_type(&node.type_id)
                .ok_or_else(|| GraphError::UnknownAudioNodeType(node.type_id.clone()))?;
            if node.type_id == "audio.group" {
                let group = node.group.as_deref().ok_or(GraphError::EmptyGroupId)?;
                if !voice_groups.contains_key(group) {
                    return Err(GraphError::GroupMissing(group.to_string()));
                }
            }
            if node.type_id == "audio.group.instance" {
                let group_id = node.group.as_deref().ok_or(GraphError::EmptyGroupId)?;
                if stack.iter().any(|item| item == group_id) {
                    return Err(GraphError::GroupCycle(group_id.to_string()));
                }
                let group = audio_groups
                    .get(group_id)
                    .ok_or_else(|| GraphError::GroupMissing(group_id.to_string()))?;
                stack.push(group_id.to_string());
                group.graph.validate_inner(
                    voice_groups,
                    audio_groups,
                    Some(&group.interface),
                    stack,
                )?;
                stack.pop();
            }
            if matches!(
                node.type_id.as_str(),
                "audio.group.input" | "audio.group.output"
            ) && interface.is_none()
            {
                return Err(GraphError::GroupBoundaryOutsideGroup);
            }
            match node.type_id.as_str() {
                "audio.group.input" => group_inputs += 1,
                "audio.group.output" => group_outputs += 1,
                _ => {},
            }
            for param in node_type.params {
                if let Some(value) = node.params.get(param.name) {
                    if !value.is_finite() || *value < param.minimum || *value > param.maximum {
                        return Err(GraphError::InvalidParam {
                            name: param.name.to_string(),
                            value: *value,
                            minimum: param.minimum,
                            maximum: param.maximum,
                        });
                    }
                }
            }
            if node.type_id == "audio.output" {
                outputs += 1;
            }
        }
        if interface.is_none() && outputs != 1 {
            return Err(GraphError::AudioOutputCount(outputs));
        }
        if interface.is_some() && (group_inputs != 1 || group_outputs != 1) {
            return Err(GraphError::GroupBoundaryCount {
                inputs: group_inputs,
                outputs: group_outputs,
            });
        }
        let mut occupied = BTreeSet::new();
        for connection in &self.connections {
            let source = self
                .nodes
                .get(&connection.from.node)
                .ok_or_else(|| GraphError::UnknownNode(connection.from.node.clone()))?;
            let target = self
                .nodes
                .get(&connection.to.node)
                .ok_or_else(|| GraphError::UnknownNode(connection.to.node.clone()))?;
            let source_ports = Self::outputs_with_groups(source, audio_groups, interface)?;
            let target_ports = Self::inputs_with_groups(target, audio_groups, interface)?;
            let Some(source_port) = source_ports
                .iter()
                .find(|port| port.id == connection.from.port)
            else {
                return Err(GraphError::AudioUnknownPort(connection.from.port.clone()));
            };
            let Some(target_port) = target_ports
                .iter()
                .find(|port| port.id == connection.to.port)
            else {
                return Err(GraphError::AudioUnknownPort(connection.to.port.clone()));
            };
            if source_port.ty != target_port.ty {
                return Err(GraphError::AudioTypeMismatch {
                    from: source_port.ty,
                    to: target_port.ty,
                });
            }
            if !audio_multi_input(&target.type_id) {
                let key = (connection.to.node.clone(), connection.to.port.clone());
                if !occupied.insert(key) {
                    return Err(GraphError::PortOccupied);
                }
            }
        }
        self.topological_order()?;
        Ok(())
    }

    pub fn topological_order(&self) -> Result<Vec<NodeId>, GraphError> {
        let mut incoming = BTreeMap::<NodeId, usize>::new();
        let mut outgoing = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for id in self.nodes.keys() {
            incoming.insert(id.clone(), 0);
        }
        for connection in &self.connections {
            *incoming
                .get_mut(&connection.to.node)
                .ok_or_else(|| GraphError::UnknownNode(connection.to.node.clone()))? += 1;
            outgoing
                .entry(connection.from.node.clone())
                .or_default()
                .push(connection.to.node.clone());
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
                *count -= 1;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeGroup;

    fn groups() -> BTreeMap<String, NodeGroup> {
        BTreeMap::from([("piano.voice".to_string(), NodeGroup::piano_default())])
    }

    #[test]
    fn default_audio_graph_validates() {
        let graph = AudioGraph::empty_track("piano.voice");
        graph.validate(&groups()).unwrap();
        assert_eq!(graph.topological_order().unwrap().len(), 2);
    }

    #[test]
    fn empty_audio_group_validates_and_passes_audio_through() {
        let group = AudioGroup::empty("aux.audio", "Aux");
        let groups = BTreeMap::from([(group.id.clone(), group.clone())]);
        group
            .graph
            .validate_with_audio_interface(&BTreeMap::new(), &groups, &group.interface)
            .unwrap();
        assert_eq!(group.interface.inputs[0].id, "in");
        assert_eq!(group.interface.outputs[0].id, "out");
    }

    #[test]
    fn audio_links_are_type_checked() {
        let mut graph = AudioGraph::empty_track("piano.voice");
        let value = NodeId::new("value").unwrap();
        graph
            .nodes
            .insert(value.clone(), AudioNode::new(value.clone(), "audio.value"));
        graph.connections.push(AudioConnection {
            from: PortRef::new(value, "out"),
            to: PortRef::fixed(NodeId::fixed("output"), "in"),
        });
        let error = graph.validate(&groups()).unwrap_err();
        assert!(matches!(error, GraphError::AudioTypeMismatch { .. }));
    }
}
