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
    #[serde(rename = "amp.adsr")]
    AmpAdsr,
    #[serde(rename = "gain")]
    Gain,
    #[serde(rename = "output")]
    Output,
}

impl NodeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InstrumentSine => "instrument.sine",
            Self::AmpAdsr => "amp.adsr",
            Self::Gain => "gain",
            Self::Output => "output",
        }
    }

    pub const fn inputs(self) -> &'static [&'static str] {
        match self {
            Self::InstrumentSine => &[],
            Self::AmpAdsr | Self::Gain | Self::Output => &[PORT_IN],
        }
    }

    pub const fn outputs(self) -> &'static [&'static str] {
        match self {
            Self::Output => &[],
            Self::InstrumentSine | Self::AmpAdsr | Self::Gain => &[PORT_OUT],
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
            Self::AmpAdsr => &ADSR_PARAMS,
            Self::Gain => &GAIN_PARAMS,
            Self::Output => &[],
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
}

impl Node {
    pub fn new(id: NodeId, kind: NodeKind) -> Self {
        Self {
            id,
            kind,
            params: kind.default_params(),
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

    pub fn param(&self, name: &str) -> Option<f64> {
        self.params.get(name).copied()
    }

    pub fn set_param(&mut self, name: &str, value: f64) -> Result<(), GraphError> {
        self.validate_param(name, value)?;
        self.params.insert(name.to_string(), value);
        Ok(())
    }

    fn validate(&self) -> Result<(), GraphError> {
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
    pub gain: f64,
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

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
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
        self.connections.push(connection);
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
            .filter(|node| node.kind() == NodeKind::InstrumentSine)
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

        let mut outgoing = BTreeSet::new();
        let mut incoming = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for connection in &self.connections {
            self.validate_connection(connection)?;
            let from = (connection.from.node.clone(), connection.from.port.clone());
            let to = (connection.to.node.clone(), connection.to.port.clone());
            if !seen.insert((from.clone(), to.clone())) {
                return Err(GraphError::DuplicateConnection);
            }
            if !outgoing.insert(from) {
                return Err(GraphError::NotLinear);
            }
            if !incoming.insert(to) {
                return Err(GraphError::NotLinear);
            }
        }

        let order = self.topological_order()?;
        if order.len() != self.nodes.len() {
            return Err(GraphError::Cycle);
        }
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
            .find(|node| node.kind() == NodeKind::InstrumentSine)
            .ok_or(GraphError::InstrumentCount(0))?;
        let mut current = instrument.id().clone();
        let mut visited = BTreeSet::new();
        let mut source_frequency = None;
        let mut adsr = None;
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
                NodeKind::AmpAdsr => {
                    adsr = Some(AdsrSettings {
                        attack: node.param("attack").unwrap_or(0.01),
                        decay: node.param("decay").unwrap_or(0.12),
                        sustain: node.param("sustain").unwrap_or(0.75),
                        release: node.param("release").unwrap_or(0.18),
                    });
                },
                NodeKind::Gain => {
                    gain *= node.param("gain").unwrap_or(1.0);
                },
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
            gain,
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
    #[error("图存在环")]
    Cycle,
    #[error("节点没有接上输出：{0}")]
    MissingOutput(NodeId),
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
        graph
            .connect(
                PortRef::new(NodeId::new("right").unwrap(), PORT_OUT),
                PortRef::new(NodeId::new("left").unwrap(), PORT_IN),
            )
            .unwrap();
        let error = graph.topological_order().unwrap_err();
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
    fn graph_round_trips_through_json() {
        let graph = Graph::linear_track();
        let encoded = serde_json::to_string(&graph).unwrap();
        let decoded: Graph = serde_json::from_str(&encoded).unwrap();
        assert_eq!(graph, decoded);
    }
}
