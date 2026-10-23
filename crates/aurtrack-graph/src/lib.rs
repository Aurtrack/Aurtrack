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
            Self::AmpAdsr => "amp.adsr",
            Self::FilterLowpass => "filter.lowpass",
            Self::Gain => "gain",
            Self::Mix => "mix",
            Self::Plugin => "plugin",
            Self::Output => "output",
        }
    }

    pub const fn is_instrument(self) -> bool {
        matches!(self, Self::InstrumentSine | Self::InstrumentSilence)
    }

    pub const fn is_device(self) -> bool {
        matches!(
            self,
            Self::AmpAdsr | Self::FilterLowpass | Self::Gain | Self::Mix | Self::Plugin
        )
    }

    pub const fn inputs(self) -> &'static [&'static str] {
        match self {
            Self::InstrumentSine | Self::InstrumentSilence => &[],
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
            Self::InstrumentSilence | Self::Mix | Self::Plugin | Self::Output => &[],
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

/// DAG 编译结果；索引 0 固定是声部输入。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledGraph {
    pub source_frequency: Option<f64>,
    pub adsr: Option<AdsrSettings>,
    pub nodes: Vec<CompiledNode>,
    pub output: usize,
    pub plugin: Option<PluginConfig>,
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
        let instrument = Node::new(NodeId::fixed("instrument"), NodeKind::InstrumentSine);
        let envelope = Node::new(NodeId::fixed("envelope"), NodeKind::AmpAdsr);
        let gain = Node::new(NodeId::fixed("gain"), NodeKind::Gain);
        let output = Node::new(NodeId::fixed("output"), NodeKind::Output);
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
        let instrument = Node::new(NodeId::fixed("instrument"), instrument);
        let output = Node::new(NodeId::fixed("output"), NodeKind::Output);
        let mut nodes = BTreeMap::new();
        nodes.insert(instrument.id().clone(), instrument);
        nodes.insert(output.id().clone(), output);
        let connections = vec![Connection::new(
            PortRef::fixed(NodeId::fixed("instrument"), PORT_OUT),
            PortRef::fixed(NodeId::fixed("output"), PORT_IN),
        )];
        Ok(Self { nodes, connections })
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
                NodeKind::InstrumentSilence => {
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
                NodeKind::InstrumentSine | NodeKind::InstrumentSilence | NodeKind::AmpAdsr => {
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
}
