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
use std::sync::Arc;

use aurtrack_command::{CommandError, CommandRequest, CommandResponse, registry};
use aurtrack_engine::{
    DelayPlan, FilterPlan, FilterSettings, GraphNodePlan, GraphOp, GraphPlan, HighpassPlan,
    LfoPlan, MAX_GRAPH_NODES, NotePlan, PluginPlan, ResonatorPlan, VOICE_DELAY_SAMPLES,
    VoiceNodePlan, VoiceOp, VoicePlan, midi_to_hz,
};
use aurtrack_graph::{
    AudioConnection, AudioGraph, AudioGroup, AudioNode, GraphError, NodeGroup, NodeGroupInterface,
    NodeGroupSocket, NodeGroupSocketType, NodeId, PortRef, VoiceConnection, VoiceFrame, VoiceGraph,
    VoiceNode, audio_node_type, node_inputs, node_outputs,
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub audio_groups: BTreeMap<String, aurtrack_graph::AudioGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_plugin: Option<MasterPlugin>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buffers: BTreeMap<String, AudioBuffer>,
}

/// 工程内采样数据；导入时解码一次，播放计划只持有 `Arc`。
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioBuffer {
    pub id: String,
    pub name: String,
    pub sample_rate: f64,
    pub samples: Vec<f32>,
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
    pub graph: aurtrack_graph::AudioGraph,
    pub clips: Vec<Clip>,
}

#[derive(Clone, Copy, Debug)]
enum AudioOutputRef {
    /// 引用父图已有的计划索引，不需要偏移。
    Parent(usize),
    /// 引用本图层新增节点的索引，合并时需要加偏移。
    Local(usize),
}

#[derive(Default)]
struct CompiledAudio {
    nodes: Vec<GraphNodePlan>,
    output: Option<usize>,
    outputs: BTreeMap<String, AudioOutputRef>,
}

fn constant_index(nodes: &mut Vec<GraphNodePlan>, value: f64) -> usize {
    let index = nodes.len();
    nodes.push(GraphNodePlan {
        op: GraphOp::Constant(value),
        inputs: Vec::new(),
        controls: Vec::new(),
    });
    index
}

fn incoming_index(
    graph: &AudioGraph,
    indices: &BTreeMap<(NodeId, String), usize>,
    node: &NodeId,
    port: &str,
) -> Option<usize> {
    graph
        .connections
        .iter()
        .find(|connection| connection.to.node() == node && connection.to.port() == port)
        .and_then(|connection| {
            indices
                .get(&(
                    connection.from.node().clone(),
                    connection.from.port().to_string(),
                ))
                .copied()
        })
}

fn incoming_ref(
    graph: &AudioGraph,
    origins: &BTreeMap<(NodeId, String), AudioOutputRef>,
    node: &NodeId,
    port: &str,
) -> Option<AudioOutputRef> {
    graph
        .connections
        .iter()
        .find(|connection| connection.to.node() == node && connection.to.port() == port)
        .and_then(|connection| {
            origins
                .get(&(
                    connection.from.node().clone(),
                    connection.from.port().to_string(),
                ))
                .copied()
        })
}

fn compile_audio_inputs(
    graph: &AudioGraph,
    node: &AudioNode,
    voice_groups: &BTreeMap<String, NodeGroup>,
    audio_groups: &BTreeMap<String, AudioGroup>,
    interface: Option<&NodeGroupInterface>,
    indices: &BTreeMap<(NodeId, String), usize>,
    nodes: &mut Vec<GraphNodePlan>,
) -> Result<Vec<usize>, GraphError> {
    let ports = AudioGraph::inputs_with_all_groups(node, voice_groups, audio_groups, interface)?;
    let mut inputs = Vec::new();
    for port in ports
        .iter()
        .filter(|port| port.ty == NodeGroupSocketType::Audio)
    {
        let connections: Vec<_> = graph
            .connections
            .iter()
            .filter(|connection| {
                connection.to.node() == &node.id && connection.to.port() == port.id
            })
            .collect();
        if connections.is_empty() {
            inputs.push(constant_index(nodes, 0.0));
            continue;
        }
        for connection in connections {
            inputs.push(
                indices
                    .get(&(
                        connection.from.node().clone(),
                        connection.from.port().to_string(),
                    ))
                    .copied()
                    .ok_or(GraphError::NotConnected)?,
            );
        }
    }
    Ok(inputs)
}

fn compile_audio_controls(
    graph: &AudioGraph,
    node: &AudioNode,
    voice_groups: &BTreeMap<String, NodeGroup>,
    audio_groups: &BTreeMap<String, AudioGroup>,
    interface: Option<&NodeGroupInterface>,
    indices: &BTreeMap<(NodeId, String), usize>,
    nodes: &mut Vec<GraphNodePlan>,
) -> Result<Vec<usize>, GraphError> {
    let ports = AudioGraph::inputs_with_all_groups(node, voice_groups, audio_groups, interface)?;
    let mut controls = Vec::new();
    for port in ports
        .iter()
        .filter(|port| port.ty == NodeGroupSocketType::Float)
    {
        let Some(connection) = graph
            .connections
            .iter()
            .find(|connection| connection.to.node() == &node.id && connection.to.port() == port.id)
        else {
            let value = node
                .port_value(&port.id)
                .or_else(|| node.param(&port.id))
                .unwrap_or(port.default);
            controls.push(constant_index(nodes, value));
            continue;
        };
        controls.push(
            indices
                .get(&(
                    connection.from.node().clone(),
                    connection.from.port().to_string(),
                ))
                .copied()
                .ok_or(GraphError::NotConnected)?,
        );
    }
    Ok(controls)
}

fn compile_audio_graph(
    graph: &AudioGraph,
    voice_groups: &BTreeMap<String, NodeGroup>,
    audio_groups: &BTreeMap<String, AudioGroup>,
    interface: Option<&NodeGroupInterface>,
    input_bindings: Option<&BTreeMap<String, usize>>,
    voice_sources: &BTreeMap<NodeId, usize>,
    stack: &mut Vec<String>,
) -> Result<CompiledAudio, GraphError> {
    let mut indices = BTreeMap::<(NodeId, String), usize>::new();
    let mut origins = BTreeMap::<(NodeId, String), AudioOutputRef>::new();
    let mut nodes = Vec::new();
    let mut compiled = CompiledAudio::default();
    for id in graph.topological_order()? {
        let node = graph
            .node(&id)
            .ok_or_else(|| GraphError::UnknownNode(id.clone()))?;
        if node.type_id == "audio.note" {
            continue;
        }
        match node.type_id.as_str() {
            "audio.group.input" => {
                if interface.is_none() {
                    return Err(GraphError::GroupBoundaryOutsideGroup);
                }
                let ports = AudioGraph::outputs_with_all_groups(
                    node,
                    voice_groups,
                    audio_groups,
                    interface,
                )?;
                for port in ports {
                    let index = input_bindings
                        .and_then(|bindings| bindings.get(&port.id))
                        .copied()
                        .unwrap_or_else(|| constant_index(&mut nodes, port.default));
                    indices.insert((id.clone(), port.id.clone()), index);
                    origins.insert((id.clone(), port.id), AudioOutputRef::Parent(index));
                }
            },
            "audio.group.output" => {
                if interface.is_none() {
                    return Err(GraphError::GroupBoundaryOutsideGroup);
                }
                let ports = AudioGraph::inputs_with_all_groups(
                    node,
                    voice_groups,
                    audio_groups,
                    interface,
                )?;
                for port in ports {
                    if let Some(reference) = incoming_ref(graph, &origins, &id, &port.id) {
                        compiled.outputs.insert(port.id, reference);
                    }
                }
            },
            "audio.group.instance" => {
                let group_id = node.group.as_deref().ok_or(GraphError::EmptyGroupId)?;
                if stack.iter().any(|item| item == group_id) {
                    return Err(GraphError::GroupCycle(group_id.to_string()));
                }
                let group = audio_groups
                    .get(group_id)
                    .ok_or_else(|| GraphError::GroupMissing(group_id.to_string()))?;
                if node.muted {
                    for socket in &group.interface.outputs {
                        let index = constant_index(&mut nodes, 0.0);
                        indices.insert((id.clone(), socket.id.clone()), index);
                        origins.insert(
                            (id.clone(), socket.id.clone()),
                            AudioOutputRef::Local(index),
                        );
                    }
                    continue;
                }
                let mut bindings = BTreeMap::new();
                let ports = AudioGraph::inputs_with_all_groups(
                    node,
                    voice_groups,
                    audio_groups,
                    interface,
                )?;
                for port in ports {
                    let index =
                        incoming_index(graph, &indices, &id, &port.id).unwrap_or_else(|| {
                            constant_index(
                                &mut nodes,
                                node.port_value(&port.id).unwrap_or(port.default),
                            )
                        });
                    bindings.insert(port.id, index);
                }
                stack.push(group_id.to_string());
                let child = compile_audio_graph(
                    &group.graph,
                    voice_groups,
                    audio_groups,
                    Some(&group.interface),
                    Some(&bindings),
                    &BTreeMap::new(),
                    stack,
                );
                stack.pop();
                let child = child?;
                let mut child_outputs = BTreeMap::new();
                for socket in &group.interface.outputs {
                    child_outputs.insert(
                        socket.id.clone(),
                        *child
                            .outputs
                            .get(&socket.id)
                            .ok_or(GraphError::NotConnected)?,
                    );
                }
                let offset = nodes.len();
                nodes.extend(child.nodes);
                for (socket_id, reference) in child_outputs {
                    let mapped = match reference {
                        AudioOutputRef::Parent(index) => AudioOutputRef::Parent(index),
                        AudioOutputRef::Local(index) => AudioOutputRef::Local(offset + index),
                    };
                    indices.insert(
                        (id.clone(), socket_id.clone()),
                        match mapped {
                            AudioOutputRef::Parent(index) | AudioOutputRef::Local(index) => index,
                        },
                    );
                    origins.insert((id.clone(), socket_id), mapped);
                }
            },
            "audio.output" => {
                let inputs = compile_audio_inputs(
                    graph,
                    node,
                    voice_groups,
                    audio_groups,
                    interface,
                    &indices,
                    &mut nodes,
                )?;
                let input = inputs
                    .first()
                    .copied()
                    .unwrap_or_else(|| constant_index(&mut nodes, 0.0));
                compiled.output = Some(nodes.len());
                nodes.push(GraphNodePlan {
                    op: GraphOp::Output,
                    inputs: vec![input],
                    controls: Vec::new(),
                });
            },
            _ => {
                let inputs = compile_audio_inputs(
                    graph,
                    node,
                    voice_groups,
                    audio_groups,
                    interface,
                    &indices,
                    &mut nodes,
                )?;
                if node.muted {
                    let ports = AudioGraph::inputs_with_all_groups(
                        node,
                        voice_groups,
                        audio_groups,
                        interface,
                    )?;
                    let reference = ports
                        .iter()
                        .find(|port| port.ty == NodeGroupSocketType::Audio)
                        .and_then(|port| incoming_ref(graph, &origins, &id, &port.id))
                        .unwrap_or_else(|| AudioOutputRef::Local(constant_index(&mut nodes, 0.0)));
                    let target = match reference {
                        AudioOutputRef::Parent(index) | AudioOutputRef::Local(index) => index,
                    };
                    let outputs = AudioGraph::outputs_with_all_groups(
                        node,
                        voice_groups,
                        audio_groups,
                        interface,
                    )?;
                    for port in outputs {
                        indices.insert((id.clone(), port.id.clone()), target);
                        origins.insert((id.clone(), port.id), reference);
                    }
                    continue;
                }
                let controls = compile_audio_controls(
                    graph,
                    node,
                    voice_groups,
                    audio_groups,
                    interface,
                    &indices,
                    &mut nodes,
                )?;
                let op = match node.type_id.as_str() {
                    "audio.group" => {
                        if let Some(index) = voice_sources.get(node.id()) {
                            GraphOp::Voice { index: *index }
                        } else {
                            GraphOp::Constant(0.0)
                        }
                    },
                    "audio.instrument.silence" => GraphOp::Constant(0.0),
                    "audio.instrument.sine" => GraphOp::Sine,
                    "audio.value" => GraphOp::Constant(node.param("value").unwrap_or(0.0)),
                    "audio.lfo" => GraphOp::Lfo(LfoPlan::new(
                        node.param("rate").unwrap_or(1.0),
                        node.param("depth").unwrap_or(0.5),
                        node.param("offset").unwrap_or(0.0),
                    )),
                    "audio.math.add" => GraphOp::Add,
                    "audio.math.multiply" => GraphOp::Multiply,
                    "audio.math.map_range" => GraphOp::MapRange,
                    "audio.math.clamp" => GraphOp::Clamp,
                    "audio.gain" | "audio.adsr" => GraphOp::Gain,
                    "audio.mix" => GraphOp::Mix,
                    "audio.filter.lowpass" => {
                        GraphOp::Filter(FilterPlan::new(FilterSettings::new(
                            node.param("cutoff").unwrap_or(1200.0),
                            node.param("resonance").unwrap_or(0.2),
                        )))
                    },
                    "audio.filter.highpass" => GraphOp::Highpass(HighpassPlan::new(
                        node.param("cutoff").unwrap_or(1200.0),
                        node.param("resonance").unwrap_or(0.2),
                    )),
                    "audio.drive" => GraphOp::Drive,
                    "audio.soundboard" => {
                        GraphOp::Soundboard(aurtrack_engine::SoundboardPlan::new())
                    },
                    "audio.resonator" => GraphOp::Resonator(ResonatorPlan::new(
                        node.param("frequency").unwrap_or(440.0),
                        node.param("q").unwrap_or(6.0),
                        node.param("gain").unwrap_or(0.1),
                    )),
                    "audio.delay" => GraphOp::Delay(DelayPlan::new(
                        node.param("time").unwrap_or(0.25),
                        node.param("feedback").unwrap_or(0.2),
                    )),
                    "audio.plugin" => GraphOp::Plugin,
                    other => {
                        return Err(GraphError::UnknownAudioNodeType(other.to_string()));
                    },
                };
                let index = nodes.len();
                let outputs = AudioGraph::outputs_with_groups(node, audio_groups, interface)?;
                for port in outputs {
                    indices.insert((id.clone(), port.id.clone()), index);
                    origins.insert((id.clone(), port.id), AudioOutputRef::Local(index));
                }
                nodes.push(GraphNodePlan {
                    op,
                    inputs,
                    controls,
                });
            },
        }
    }
    if nodes.len() > MAX_GRAPH_NODES {
        return Err(GraphError::NodeLimit {
            count: nodes.len(),
            limit: MAX_GRAPH_NODES,
        });
    }
    compiled.nodes = nodes;
    Ok(compiled)
}

fn ungroup_audio_instance(
    graph: &mut AudioGraph,
    instance_id: &NodeId,
    audio_groups: &BTreeMap<String, AudioGroup>,
) -> Result<Vec<NodeId>, GraphError> {
    let instance = graph
        .nodes
        .get(instance_id)
        .cloned()
        .ok_or_else(|| GraphError::UnknownNode(instance_id.clone()))?;
    let group_id = instance.group.as_deref().ok_or(GraphError::EmptyGroupId)?;
    let group = audio_groups
        .get(group_id)
        .ok_or_else(|| GraphError::GroupMissing(group_id.to_string()))?;

    let mut mapping = BTreeMap::new();
    let mut counter = 1;
    for (id, node) in &group.graph.nodes {
        if matches!(
            node.type_id.as_str(),
            "audio.group.input" | "audio.group.output"
        ) {
            continue;
        }
        let mut new_id = id.clone();
        while graph.nodes.contains_key(&new_id) || &new_id == instance_id {
            new_id = NodeId::new(format!("ungroup-{counter}"))?;
            counter += 1;
        }
        mapping.insert(id.clone(), new_id);
    }

    let mut node_ids = Vec::new();
    for (id, new_id) in &mapping {
        if let Some(mut node) = group.graph.nodes.get(id).cloned() {
            node.id = new_id.clone();
            node.frame = None;
            graph.nodes.insert(new_id.clone(), node);
            node_ids.push(new_id.clone());
        }
    }

    let mut new_connections = Vec::new();
    for connection in &group.graph.connections {
        let Some(source_node) = group.graph.nodes.get(connection.from.node()) else {
            continue;
        };
        let Some(target_node) = group.graph.nodes.get(connection.to.node()) else {
            continue;
        };
        if source_node.type_id == "audio.group.input" && target_node.type_id == "audio.group.output"
        {
            let external_in = graph.connections.iter().find(|candidate| {
                candidate.to.node() == instance_id && candidate.to.port() == connection.from.port()
            });
            let external_out = graph.connections.iter().find(|candidate| {
                candidate.from.node() == instance_id
                    && candidate.from.port() == connection.to.port()
            });
            if let (Some(external_in), Some(external_out)) = (external_in, external_out) {
                new_connections.push(AudioConnection::new(
                    external_in.from.clone(),
                    external_out.to.clone(),
                ));
            }
            continue;
        }
        if source_node.type_id == "audio.group.input" {
            let external = graph.connections.iter().find(|candidate| {
                candidate.to.node() == instance_id && candidate.to.port() == connection.from.port()
            });
            let Some(external) = external else {
                continue;
            };
            let Some(target_id) = mapping.get(connection.to.node()) else {
                continue;
            };
            new_connections.push(AudioConnection::new(
                external.from.clone(),
                PortRef::new(target_id.clone(), connection.to.port().to_string()),
            ));
            continue;
        }
        if target_node.type_id == "audio.group.output" {
            let external = graph.connections.iter().find(|candidate| {
                candidate.from.node() == instance_id
                    && candidate.from.port() == connection.to.port()
            });
            let Some(external) = external else {
                continue;
            };
            let Some(source_id) = mapping.get(connection.from.node()) else {
                continue;
            };
            new_connections.push(AudioConnection::new(
                PortRef::new(source_id.clone(), connection.from.port().to_string()),
                external.to.clone(),
            ));
            continue;
        }
        let (Some(source_id), Some(target_id)) = (
            mapping.get(connection.from.node()),
            mapping.get(connection.to.node()),
        ) else {
            continue;
        };
        new_connections.push(AudioConnection::new(
            PortRef::new(source_id.clone(), connection.from.port().to_string()),
            PortRef::new(target_id.clone(), connection.to.port().to_string()),
        ));
    }

    graph.nodes.remove(instance_id);
    graph.connections.retain(|connection| {
        connection.from.node() != instance_id && connection.to.node() != instance_id
    });
    graph.connections.extend(new_connections);
    Ok(node_ids)
}

fn output_path_nodes(graph: &AudioGraph) -> BTreeSet<NodeId> {
    let Some(output) = graph.nodes().find(|node| node.type_id == "audio.output") else {
        return BTreeSet::new();
    };
    let mut path = BTreeSet::new();
    let mut queue = vec![output.id().clone()];
    while let Some(id) = queue.pop() {
        if !path.insert(id.clone()) {
            continue;
        }
        for connection in &graph.connections {
            if connection.to.node() == &id {
                queue.push(connection.from.node().clone());
            }
        }
    }
    path
}

impl Track {
    /// 输出路径上第一个活乐器；`source` 字段只保留给旧工程读取。
    pub fn active_instrument(&self) -> Option<&AudioNode> {
        let path = output_path_nodes(&self.graph);
        self.graph
            .nodes()
            .find(|node| path.contains(node.id()) && node.is_instrument())
    }

    pub fn source(&self) -> Source {
        let Some(instrument) = self.active_instrument() else {
            return Source::Silence;
        };
        match instrument.type_id.as_str() {
            "audio.instrument.sine" => Source::Sine {
                frequency: instrument.param("frequency").unwrap_or(440.0),
            },
            "audio.group" => {
                let group = instrument.group.as_deref().unwrap_or_default();
                if group == "sine.voice" {
                    Source::Sine {
                        frequency: instrument.port_value("frequency").unwrap_or(440.0),
                    }
                } else if group == "piano.physical" {
                    Source::Physical
                } else if let Some(buffer_id) = group.strip_prefix("sampler.") {
                    Source::Sampler {
                        buffer_id: buffer_id.to_string(),
                    }
                } else {
                    Source::Piano
                }
            },
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
                kind: device_kind(&node.type_id).to_string(),
                params: node.params().clone(),
            })
            .collect()
    }

    pub fn playback_graph(
        &self,
        groups: &BTreeMap<String, NodeGroup>,
        audio_groups: &BTreeMap<String, AudioGroup>,
    ) -> Result<(Option<f64>, Option<aurtrack_graph::AdsrSettings>, GraphPlan), GraphError> {
        self.playback_graph_with_buffers(groups, audio_groups, &BTreeMap::new())
    }

    pub fn playback_graph_with_buffers(
        &self,
        groups: &BTreeMap<String, NodeGroup>,
        audio_groups: &BTreeMap<String, AudioGroup>,
        buffers: &BTreeMap<String, AudioBuffer>,
    ) -> Result<(Option<f64>, Option<aurtrack_graph::AdsrSettings>, GraphPlan), GraphError> {
        self.graph
            .validate_with_audio_groups(groups, audio_groups)?;
        let has_note_source = |node_id: &aurtrack_graph::NodeId| {
            self.graph.connections.iter().any(|connection| {
                connection.to.node() == node_id
                    && self
                        .graph
                        .nodes
                        .get(connection.from.node())
                        .is_some_and(|node| node.type_id == "audio.note")
            })
        };
        let mut source_frequency = None;
        let mut adsr = None;
        let mut plugin = None;
        let output_path = output_path_nodes(&self.graph);
        for node in self.graph.nodes() {
            if !output_path.contains(node.id()) {
                continue;
            }
            match node.type_id.as_str() {
                "audio.adsr" => {
                    adsr = Some(aurtrack_graph::AdsrSettings::new(
                        node.param("attack").unwrap_or(0.01),
                        node.param("decay").unwrap_or(0.12),
                        node.param("sustain").unwrap_or(0.75),
                        node.param("release").unwrap_or(0.18),
                    ));
                },
                "audio.plugin" => {
                    if let Some(config) = node.plugin() {
                        plugin = Some(config.clone());
                    }
                },
                _ => {},
            }
        }
        let mut pianos = Vec::new();
        let mut voice_sources = BTreeMap::new();
        for node in self.graph.nodes() {
            if !output_path.contains(node.id()) {
                continue;
            }
            if node.muted {
                continue;
            }
            if node.type_id == "audio.group" && has_note_source(node.id()) {
                let Some(group_id) = node.group.as_deref() else {
                    continue;
                };
                let group = groups
                    .get(group_id)
                    .ok_or_else(|| GraphError::GroupMissing(group_id.to_string()))?;
                group.validate_with_groups(groups)?;
                let mut piano = aurtrack_engine::PianoPlan::new(10, 1.2, 0.5, 0.2);
                piano.voice = Some(compile_voice(
                    &group.graph,
                    &group.interface,
                    groups,
                    buffers,
                )?);
                voice_sources.insert(node.id().clone(), pianos.len());
                pianos.push(piano);
                if group_id == "sine.voice" && !node.muted {
                    source_frequency = Some(node.port_value("frequency").unwrap_or(440.0));
                }
            } else if node.type_id == "audio.instrument.sine" && !node.muted {
                source_frequency = Some(node.param("frequency").unwrap_or(440.0));
            }
        }
        let compiled = compile_audio_graph(
            &self.graph,
            groups,
            audio_groups,
            None,
            None,
            &voice_sources,
            &mut Vec::new(),
        )?;
        let nodes = compiled.nodes;
        let output = compiled.output.ok_or(GraphError::AudioOutputCount(0))?;
        if nodes.len() > aurtrack_engine::MAX_GRAPH_NODES {
            return Err(GraphError::TooManyNodes(nodes.len()));
        }
        Ok((
            source_frequency,
            adsr,
            GraphPlan {
                nodes,
                output,
                plugin: plugin.map(|plugin| PluginPlan::new(plugin.path, plugin.id, plugin.state)),
                pianos,
                fallback_sine: source_frequency,
            },
        ))
    }
}

const NOTE_BINDING: usize = usize::MAX;
const NO_NOTE_BINDING: usize = usize::MAX - 1;

fn compile_voice(
    voice: &VoiceGraph,
    interface: &NodeGroupInterface,
    groups: &BTreeMap<String, NodeGroup>,
    buffers: &BTreeMap<String, AudioBuffer>,
) -> Result<VoicePlan, GraphError> {
    compile_voice_inner(
        voice,
        interface,
        groups,
        buffers,
        &mut Vec::new(),
        None,
        None,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_voice_inner(
    voice: &VoiceGraph,
    interface: &NodeGroupInterface,
    groups: &BTreeMap<String, NodeGroup>,
    buffers: &BTreeMap<String, AudioBuffer>,
    stack: &mut Vec<String>,
    input_bindings: Option<&BTreeMap<String, usize>>,
    external_inputs: Option<&BTreeMap<(NodeId, String), usize>>,
    initial_nodes: &[VoiceNodePlan],
) -> Result<VoicePlan, GraphError> {
    let order = voice.topological_order_with_zones()?;
    let zone_body_ids = voice.zone_body_ids();
    let mut indices = BTreeMap::<(NodeId, String), usize>::new();
    let mut outputs = BTreeMap::<String, usize>::new();
    let mut nodes = initial_nodes.to_vec();
    let mut output = None;
    for id in order {
        if zone_body_ids.contains(&id) {
            continue;
        }
        let node = voice.node(&id).ok_or(GraphError::VoiceUnknownNode)?;
        let input_ports = aurtrack_graph::node_inputs(node, interface, groups)?;
        let mut incoming = Vec::new();
        let mut inputs = Vec::new();
        for port in &input_ports {
            let mut matches: Vec<&VoiceConnection> = voice
                .connections
                .iter()
                .filter(|connection| {
                    !connection.muted
                        && connection.to.node() == &id
                        && connection.to.port() == port.id.as_str()
                })
                .collect();
            if !port.multi {
                matches.truncate(1);
            }
            if matches.is_empty() {
                if let Some(slot) = external_inputs
                    .and_then(|external| external.get(&(id.clone(), port.id.clone())).copied())
                {
                    inputs.push(slot);
                    continue;
                }
                let value = node.params.get(&port.id).copied().unwrap_or(port.default);
                let index = nodes.len();
                nodes.push(VoiceNodePlan::new(VoiceOp::Constant(value), Vec::new()));
                inputs.push(index);
                continue;
            }
            for connection in matches {
                incoming.push(connection);
                inputs.push(
                    indices
                        .get(&(
                            connection.from.node().clone(),
                            connection.from.port().to_string(),
                        ))
                        .copied()
                        .ok_or(GraphError::VoiceUnknownNode)?,
                );
            }
        }
        if node.type_id == "group.output" {
            for (index, port) in input_ports.iter().enumerate() {
                if let Some(value) = inputs.get(index) {
                    outputs.insert(port.id.clone(), *value);
                    if output.is_none() {
                        output = Some(*value);
                    }
                }
            }
            continue;
        }
        if node.type_id == "voice.vector_add" {
            for (port, left, right) in [("x", 0, 3), ("y", 1, 4), ("z", 2, 5)] {
                let index = nodes.len();
                nodes.push(VoiceNodePlan::new(
                    VoiceOp::Add,
                    vec![inputs[left], inputs[right]],
                ));
                indices.insert((id.clone(), port.to_string()), index);
            }
            continue;
        }
        if node.type_id == "group.input" {
            for socket in &interface.inputs {
                if socket.ty == NodeGroupSocketType::Note {
                    let binding = input_bindings
                        .and_then(|bindings| bindings.get(&socket.id))
                        .copied()
                        .unwrap_or(NOTE_BINDING);
                    indices.insert((id.clone(), socket.id.clone()), binding);
                    continue;
                }
                if let Some(index) = input_bindings.and_then(|bindings| bindings.get(&socket.id)) {
                    indices.insert((id.clone(), socket.id.clone()), *index);
                    continue;
                }
                let op = match socket.ty {
                    NodeGroupSocketType::Gate => {
                        VoiceOp::Constant(f64::from(u8::from(socket.default.unwrap_or(0.0) > 0.5)))
                    },
                    _ => VoiceOp::Constant(socket.default.unwrap_or(0.0)),
                };
                let index = nodes.len();
                nodes.push(VoiceNodePlan::new(op, Vec::new()));
                indices.insert((id.clone(), socket.id.clone()), index);
            }
            continue;
        }
        if node.type_id.starts_with("reroute.") {
            let target = *inputs.first().ok_or(GraphError::NotConnected)?;
            for port in aurtrack_graph::node_outputs(node, interface, groups)? {
                indices.insert((id.clone(), port.id), target);
            }
            continue;
        }
        if node.muted && node.type_id != "group.output" {
            let ports = aurtrack_graph::node_inputs(node, interface, groups)?;
            let audio_index = ports
                .iter()
                .position(|port| port.ty == NodeGroupSocketType::Audio);
            let target = if let Some(index) = audio_index.and_then(|index| inputs.get(index)) {
                *index
            } else {
                let index = nodes.len();
                nodes.push(VoiceNodePlan::new(VoiceOp::Constant(0.0), Vec::new()));
                index
            };
            for port in aurtrack_graph::node_outputs(node, interface, groups)? {
                indices.insert((id.clone(), port.id), target);
            }
            continue;
        }
        if node.type_id == "note.separate" {
            let note_connected = incoming.iter().any(|connection| {
                connection.to.port() == "note"
                    && indices
                        .get(&(
                            connection.from.node().clone(),
                            connection.from.port().to_string(),
                        ))
                        .is_some_and(|index| *index != NO_NOTE_BINDING)
            });
            for (port, op) in [
                ("pitch", VoiceOp::Pitch),
                ("velocity", VoiceOp::Velocity),
                ("gate", VoiceOp::Gate),
                ("index", VoiceOp::NoteIndex),
            ] {
                let op = if note_connected {
                    op
                } else {
                    VoiceOp::Constant(0.0)
                };
                let index = nodes.len();
                nodes.push(VoiceNodePlan::new(op, Vec::new()));
                indices.insert((id.clone(), port.to_string()), index);
            }
            continue;
        }
        if node.type_id == "voice.zone.repeat" {
            let input_node = voice
                .nodes
                .values()
                .find(|candidate| {
                    candidate.zone.as_deref() == Some(id.as_str())
                        && candidate.type_id == "voice.zone.input"
                })
                .ok_or(GraphError::ZoneTerminals)?;
            let output_node = voice
                .nodes
                .values()
                .find(|candidate| {
                    candidate.zone.as_deref() == Some(id.as_str())
                        && candidate.type_id == "voice.zone.output"
                })
                .ok_or(GraphError::ZoneTerminals)?;
            let (body_graph, body_interface) = voice.zone_body(node)?;
            let inputs = aurtrack_graph::zone_input_sockets(node);
            let outputs = aurtrack_graph::zone_output_sockets(node);
            if inputs.len() != outputs.len() || outputs.len() > aurtrack_engine::MAX_REPEAT_OUTPUTS
            {
                return Err(GraphError::ZoneTerminals);
            }
            let value_inputs: Vec<&NodeGroupSocket> = inputs
                .iter()
                .filter(|socket| socket.ty != NodeGroupSocketType::Note)
                .collect();
            let value_outputs: Vec<&NodeGroupSocket> = outputs
                .iter()
                .filter(|socket| socket.ty != NodeGroupSocketType::Note)
                .collect();
            let mut bindings = BTreeMap::new();
            let mut body_inputs = Vec::new();
            for (index, socket) in value_inputs.iter().enumerate() {
                bindings.insert(socket.id.clone(), index);
                body_inputs.push(VoiceNodePlan::new(VoiceOp::Input(index), Vec::new()));
            }
            let mut external_inputs = BTreeMap::new();
            let mut free_values = Vec::new();
            for connection in &voice.connections {
                if connection.muted
                    || body_graph.nodes.contains_key(connection.from.node())
                    || !body_graph.nodes.contains_key(connection.to.node())
                    || connection.to.node() == &input_node.id
                    || connection.to.node() == &output_node.id
                {
                    continue;
                }
                let target_is_note = body_graph
                    .nodes
                    .get(connection.to.node())
                    .and_then(|target| {
                        aurtrack_graph::node_inputs(target, &body_interface, groups).ok()
                    })
                    .is_some_and(|ports| {
                        ports.iter().any(|port| {
                            port.id == connection.to.port() && port.ty == NodeGroupSocketType::Note
                        })
                    });
                if target_is_note {
                    continue;
                }
                let source = indices
                    .get(&(
                        connection.from.node().clone(),
                        connection.from.port().to_string(),
                    ))
                    .copied()
                    .ok_or(GraphError::VoiceUnknownNode)?;
                let key = (
                    connection.to.node().clone(),
                    connection.to.port().to_string(),
                );
                if external_inputs.contains_key(&key) {
                    continue;
                }
                let slot = value_inputs.len() + free_values.len();
                external_inputs.insert(key, slot);
                body_inputs.push(VoiceNodePlan::new(VoiceOp::Input(slot), Vec::new()));
                free_values.push(source);
            }
            bindings.insert("iteration".to_string(), body_inputs.len());
            body_inputs.push(VoiceNodePlan::new(VoiceOp::Iteration, Vec::new()));
            let body = compile_voice_inner(
                &body_graph,
                &body_interface,
                groups,
                buffers,
                stack,
                Some(&bindings),
                Some(&external_inputs),
                &body_inputs,
            )?;
            let mut external = Vec::new();
            let count = voice
                .connections
                .iter()
                .filter(|connection| {
                    !connection.muted
                        && connection.to.node() == &input_node.id
                        && connection.to.port() == "count"
                })
                .find_map(|connection| {
                    indices
                        .get(&(
                            connection.from.node().clone(),
                            connection.from.port().to_string(),
                        ))
                        .copied()
                });
            let count_index = count.unwrap_or_else(|| {
                let index = nodes.len();
                nodes.push(VoiceNodePlan::new(
                    VoiceOp::Constant(
                        input_node
                            .params
                            .get("count")
                            .or_else(|| node.params.get("count"))
                            .copied()
                            .unwrap_or(10.0),
                    ),
                    Vec::new(),
                ));
                index
            });
            external.push(count_index);
            for socket in &value_inputs {
                let source = voice
                    .connections
                    .iter()
                    .filter(|connection| {
                        !connection.muted
                            && connection.to.node() == &input_node.id
                            && connection.to.port() == socket.id
                    })
                    .find_map(|connection| {
                        indices
                            .get(&(
                                connection.from.node().clone(),
                                connection.from.port().to_string(),
                            ))
                            .copied()
                    });
                let index = source.unwrap_or_else(|| {
                    let index = nodes.len();
                    nodes.push(VoiceNodePlan::new(
                        VoiceOp::Constant(socket.default.unwrap_or(0.0)),
                        Vec::new(),
                    ));
                    index
                });
                external.push(index);
            }
            external.extend(free_values);
            let mut slots = Vec::with_capacity(value_outputs.len());
            for socket in &value_outputs {
                slots.push(
                    *body
                        .outputs
                        .get(&socket.id)
                        .ok_or(GraphError::VoiceTerminals)?,
                );
            }
            let repeat_index = nodes.len();
            nodes.push(VoiceNodePlan::new(
                VoiceOp::Repeat {
                    zone: id.to_string(),
                    body: Box::new(body),
                    outputs: slots,
                    carried: value_inputs.len(),
                },
                external,
            ));
            for (slot, socket) in value_outputs.iter().enumerate() {
                let index = nodes.len();
                nodes.push(VoiceNodePlan::new(
                    VoiceOp::RepeatResult {
                        source: repeat_index,
                        slot,
                    },
                    Vec::new(),
                ));
                indices.insert((output_node.id.clone(), socket.id.clone()), index);
            }
            for socket in &outputs {
                if socket.ty == NodeGroupSocketType::Note {
                    indices.insert((output_node.id.clone(), socket.id.clone()), NOTE_BINDING);
                }
            }
            continue;
        }
        if node.type_id == "voice.repeat" {
            let group_id = node.group.as_deref().ok_or(GraphError::EmptyGroupId)?;
            if stack.iter().any(|item| item == group_id) {
                return Err(GraphError::GroupCycle(group_id.to_string()));
            }
            let group = groups
                .get(group_id)
                .ok_or_else(|| GraphError::GroupMissing(group_id.to_string()))?;
            if group.interface.inputs.len() != 1
                || group.interface.inputs[0].id != "in"
                || group.interface.inputs[0].ty != NodeGroupSocketType::Audio
                || group.interface.outputs.len() != 1
                || group.interface.outputs[0].id != "audio"
                || group.interface.outputs[0].ty != NodeGroupSocketType::Audio
            {
                return Err(GraphError::InvalidGroupSocket {
                    socket: "repeat".to_string(),
                    reason: "循环体必须恰好有一个 in 音频输入和一个 audio 输出".to_string(),
                });
            }
            let mut bindings = BTreeMap::new();
            bindings.insert("in".to_string(), 0);
            stack.push(group_id.to_string());
            let body = compile_voice_inner(
                &group.graph,
                &group.interface,
                groups,
                buffers,
                stack,
                Some(&bindings),
                None,
                &[VoiceNodePlan::new(VoiceOp::Input(0), Vec::new())],
            )?;
            stack.pop();
            let initial = nodes.len();
            nodes.push(VoiceNodePlan::new(VoiceOp::Constant(0.0), Vec::new()));
            let index = nodes.len();
            nodes.push(VoiceNodePlan::new(
                VoiceOp::Repeat {
                    zone: group_id.to_string(),
                    outputs: vec![body.outputs.values().next().copied().unwrap_or(body.output)],
                    body: Box::new(body),
                    carried: 1,
                },
                vec![inputs.first().copied().unwrap_or(initial), initial],
            ));
            indices.insert((id.clone(), "out".to_string()), index);
            continue;
        }
        if node.type_id == "voice.group" {
            let group_id = node.group.as_deref().ok_or(GraphError::EmptyGroupId)?;
            if stack.iter().any(|item| item == group_id) {
                return Err(GraphError::GroupCycle(group_id.to_string()));
            }
            let group = groups
                .get(group_id)
                .ok_or_else(|| GraphError::GroupMissing(group_id.to_string()))?;
            if group
                .interface
                .inputs
                .iter()
                .any(|socket| socket.ty == NodeGroupSocketType::Note)
            {
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
                for socket in &group.interface.inputs {
                    if socket.ty == NodeGroupSocketType::Note {
                        continue;
                    }
                    if bindings.contains_key(&socket.id) {
                        continue;
                    }
                    let value = node
                        .params
                        .get(&socket.id)
                        .copied()
                        .or_else(|| socket.effective_default())
                        .unwrap_or(0.0);
                    let index = nodes.len();
                    nodes.push(VoiceNodePlan::new(VoiceOp::Constant(value), Vec::new()));
                    bindings.insert(socket.id.clone(), index);
                }
                stack.push(group_id.to_string());
                let mut child = compile_voice_inner(
                    &group.graph,
                    &group.interface,
                    groups,
                    buffers,
                    stack,
                    Some(&bindings),
                    None,
                    &[],
                )?;
                stack.pop();
                let offset = nodes.len();
                for child_node in &mut child.nodes {
                    for input in &mut child_node.inputs {
                        *input += offset;
                    }
                }
                nodes.extend(child.nodes);
                for socket in &group.interface.outputs {
                    let source = child
                        .outputs
                        .get(&socket.id)
                        .copied()
                        .unwrap_or(child.output);
                    indices.insert((id.clone(), socket.id.clone()), offset + source);
                }
                continue;
            }
            let mut bindings = BTreeMap::new();
            let mut initial = Vec::with_capacity(group.interface.inputs.len());
            for (slot, socket) in group.interface.inputs.iter().enumerate() {
                bindings.insert(socket.id.clone(), slot);
                initial.push(VoiceNodePlan::new(VoiceOp::Input(slot), Vec::new()));
            }
            stack.push(group_id.to_string());
            let body = compile_voice_inner(
                &group.graph,
                &group.interface,
                groups,
                buffers,
                stack,
                Some(&bindings),
                None,
                &initial,
            )?;
            stack.pop();
            let outputs: Vec<usize> = group
                .interface
                .outputs
                .iter()
                .map(|socket| body.outputs.get(&socket.id).copied().unwrap_or(body.output))
                .collect();
            let index = nodes.len();
            nodes.push(VoiceNodePlan::new(
                VoiceOp::GroupCall {
                    body: Box::new(body),
                    outputs,
                },
                inputs.clone(),
            ));
            for (slot, socket) in group.interface.outputs.iter().enumerate() {
                let result = nodes.len();
                nodes.push(VoiceNodePlan::new(
                    VoiceOp::GroupResult {
                        source: index,
                        slot,
                    },
                    Vec::new(),
                ));
                indices.insert((id.clone(), socket.id.clone()), result);
            }
            continue;
        }
        if node.type_id == "delay.line" {
            let index = nodes.len();
            nodes.push(VoiceNodePlan::with_buffer(
                VoiceOp::Delay,
                inputs,
                VOICE_DELAY_SAMPLES,
            ));
            for port in aurtrack_graph::node_outputs(node, interface, groups)? {
                indices.insert((id.clone(), port.id), index);
            }
            continue;
        }
        let param = |name: &str, default: f64| node.params.get(name).copied().unwrap_or(default);
        if node.type_id == "string.waveguide" {
            let index = nodes.len();
            nodes.push(VoiceNodePlan::with_buffer(
                VoiceOp::Waveguide,
                inputs,
                VOICE_DELAY_SAMPLES + 4,
            ));
            for port in aurtrack_graph::node_outputs(node, interface, groups)? {
                indices.insert((id.clone(), port.id), index);
            }
            continue;
        }
        if node.type_id == "buffer.sample" {
            let buffer_id = node
                .buffer
                .as_deref()
                .ok_or_else(|| GraphError::BufferMissing("<未指定>".to_string()))?;
            let buffer = buffers
                .get(buffer_id)
                .ok_or_else(|| GraphError::BufferMissing(buffer_id.to_string()))?;
            let samples: Arc<[f32]> = Arc::from(buffer.samples.as_slice());
            let index = nodes.len();
            nodes.push(VoiceNodePlan::new(
                VoiceOp::BufferSample {
                    samples,
                    sample_rate: buffer.sample_rate,
                    root: param("root", 261.63),
                    looped: param("loop", 0.0) > 0.5,
                },
                inputs,
            ));
            for port in aurtrack_graph::node_outputs(node, interface, groups)? {
                indices.insert((id.clone(), port.id), index);
            }
            continue;
        }
        let op = match node.type_id.as_str() {
            "osc.sine" => VoiceOp::Sine,
            "hammer.exciter" => VoiceOp::Hammer,
            "damper" => VoiceOp::Damper,
            "voice.partial" => VoiceOp::Partial,
            "voice.piano_partial" => VoiceOp::PianoPartial,
            "voice.iteration" => VoiceOp::Iteration,
            "voice.pitch" => VoiceOp::Pitch,
            "voice.velocity" => VoiceOp::Velocity,
            "voice.attack" => VoiceOp::Attack,
            "voice.decay" => VoiceOp::Decay,
            "env.adsr" => VoiceOp::Adsr,
            "voice.noise" => VoiceOp::Noise,
            "filter.biquad.lowpass" => VoiceOp::Biquad { highpass: false },
            "filter.biquad.highpass" => VoiceOp::Biquad { highpass: true },
            "voice.vca" => VoiceOp::Vca,
            "voice.add" | "voice.sum" | "voice.sum_float" | "math.add" => VoiceOp::Add,
            "voice.mix" => VoiceOp::Mix,
            "voice.gate" => VoiceOp::GateNode,
            "voice.switch" => VoiceOp::Switch,
            "voice.index_switch" | "voice.index_switch_float" => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let slots = (param("count", 4.0).round() as usize).clamp(1, 16);
                VoiceOp::IndexSwitch { slots }
            },
            "voice.broadcast" => VoiceOp::Broadcast,
            "voice.sample_hold" => VoiceOp::SampleHold,
            "voice.time" => VoiceOp::Time,
            "voice.white_noise" => VoiceOp::WhiteNoise,
            "math.multiply" => VoiceOp::Multiply,
            "math.subtract" => VoiceOp::Subtract,
            "math.divide" => VoiceOp::Divide,
            "math.power" => VoiceOp::Power,
            "math.modulo" => VoiceOp::Modulo,
            "math.floor" => VoiceOp::Floor,
            "math.ceil" => VoiceOp::Ceil,
            "math.round" => VoiceOp::Round,
            "math.exp" => VoiceOp::Exp,
            "math.sin" => VoiceOp::Sin,
            "math.abs" => VoiceOp::Abs,
            "math.sqrt" => VoiceOp::Sqrt,
            "math.int_to_float" => VoiceOp::IntToFloat,
            "math.greater_than" => VoiceOp::GreaterThan,
            "math.less_than" => VoiceOp::LessThan,
            "math.map_range" => VoiceOp::MapRange,
            "math.clamp" => VoiceOp::Clamp,
            "value.float" => VoiceOp::Constant(param("value", 0.0)),
            _ => return Err(GraphError::VoiceUnknownNode),
        };
        let index = nodes.len();
        if node.type_id != "group.output" {
            for port in aurtrack_graph::node_outputs(node, interface, groups)? {
                indices.insert((id.clone(), port.id), index);
            }
        }
        nodes.push(VoiceNodePlan::new(op, inputs));
    }
    let output = output.ok_or(GraphError::VoiceTerminals)?;
    let mut preview_outputs = BTreeMap::new();
    for (id, node) in &voice.nodes {
        if let Ok(ports) = aurtrack_graph::node_outputs(node, interface, groups) {
            if let Some(port) = ports.first() {
                if let Some(index) = indices.get(&(id.clone(), port.id.clone())) {
                    preview_outputs.insert(id.to_string(), *index);
                }
            }
        }
    }
    let mut plan = VoicePlan::new(nodes, output);
    plan.set_outputs(outputs);
    plan.set_node_outputs(preview_outputs);
    Ok(plan)
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
    Physical,
    Sampler { buffer_id: String },
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    format_version: u32,
    project: Project,
}

fn remove_group_socket(sockets: &mut Vec<NodeGroupSocket>, id: &str) -> bool {
    let before = sockets.len();
    sockets.retain(|socket| socket.id != id);
    sockets.len() != before
}

fn migrate_piano_voice_graph(graph: &mut VoiceGraph) -> bool {
    let Some(header_id) = graph
        .nodes
        .values()
        .find(|node| node.type_id == "voice.zone.repeat")
        .map(|node| node.id.clone())
    else {
        return false;
    };
    let input_id = graph
        .nodes
        .values()
        .find(|node| {
            node.zone.as_deref() == Some(header_id.as_str()) && node.type_id == "voice.zone.input"
        })
        .map(|node| node.id.clone());
    let output_id = graph
        .nodes
        .values()
        .find(|node| {
            node.zone.as_deref() == Some(header_id.as_str()) && node.type_id == "voice.zone.output"
        })
        .map(|node| node.id.clone());
    let (Some(input_id), Some(output_id)) = (input_id, output_id) else {
        return false;
    };
    let partial_id = NodeId::fixed("p-partial");
    let separate_id = NodeId::fixed("note-separate");
    let mut changed = false;

    for port in ["pitch", "velocity"] {
        let paired = graph
            .nodes
            .get(&input_id)
            .is_some_and(|node| node.zone_inputs.iter().any(|socket| socket.id == port));
        let source = graph
            .connections
            .iter()
            .find(|connection| {
                connection.to.node() == &input_id
                    && connection.to.port() == port
                    && connection.from.node() != &input_id
            })
            .map(|connection| connection.from.clone());
        let target = graph
            .connections
            .iter()
            .find(|connection| {
                connection.from.node() == &input_id
                    && connection.from.port() == port
                    && connection.to.node() != &output_id
            })
            .map(|connection| connection.to.clone());
        let direct = graph.connections.iter().any(|connection| {
            connection.from.node() == &separate_id
                && connection.from.port() == port
                && connection.to.node() == &partial_id
                && connection.to.port() == port
        });
        if !direct {
            let source = if graph.nodes.contains_key(&separate_id) {
                Some(PortRef::new(separate_id.clone(), port))
            } else {
                source
            };
            let target = target.or_else(|| {
                graph
                    .nodes
                    .contains_key(&partial_id)
                    .then(|| PortRef::new(partial_id.clone(), port))
            });
            if let (Some(source), Some(target)) = (source, target) {
                graph.connections.push(VoiceConnection::new(source, target));
                changed = true;
            }
        }
        if paired {
            let before = graph.connections.len();
            graph.connections.retain(|connection| {
                let into = connection.to.node() == &input_id && connection.to.port() == port;
                let out = connection.from.node() == &input_id && connection.from.port() == port;
                !(into || out)
            });
            changed |= before != graph.connections.len();
            if let Some(node) = graph.nodes.get_mut(&input_id) {
                changed |= remove_group_socket(&mut node.zone_inputs, port);
            }
            if let Some(node) = graph.nodes.get_mut(&output_id) {
                changed |= remove_group_socket(&mut node.zone_outputs, port);
            }
            if let Some(header) = graph.nodes.get_mut(&header_id) {
                changed |= remove_group_socket(&mut header.zone_inputs, port);
                changed |= remove_group_socket(&mut header.zone_outputs, port);
            }
        }
    }

    for id in ["p-pitch", "p-velocity"] {
        let id = NodeId::fixed(id);
        if graph.nodes.remove(&id).is_some() {
            graph
                .connections
                .retain(|connection| connection.from.node() != &id && connection.to.node() != &id);
            changed = true;
        }
    }

    let open_ids: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(id, _)| id.as_str().starts_with("p-open-"))
        .map(|(id, _)| id.clone())
        .collect();
    for id in open_ids {
        let value = graph
            .nodes
            .get(&id)
            .and_then(|node| node.params.get("value").copied())
            .unwrap_or(0.0);
        let targets: Vec<(NodeId, String)> = graph
            .connections
            .iter()
            .filter(|connection| connection.from.node() == &id)
            .map(|connection| {
                (
                    connection.to.node().clone(),
                    connection.to.port().to_string(),
                )
            })
            .collect();
        for (target, port) in targets {
            if target == partial_id {
                if let Some(node) = graph.nodes.get_mut(&target) {
                    node.params.entry(port).or_insert(value);
                }
            }
        }
        graph.nodes.remove(&id);
        graph
            .connections
            .retain(|connection| connection.from.node() != &id && connection.to.node() != &id);
        changed = true;
    }

    let legacy_index = NodeId::fixed("p-index");
    if graph.nodes.remove(&legacy_index).is_some() {
        graph.connections.retain(|connection| {
            connection.from.node() != &legacy_index && connection.to.node() != &legacy_index
        });
        changed = true;
    }
    let has_iteration = graph.connections.iter().any(|connection| {
        connection.from.node() == &input_id
            && connection.from.port() == "iteration"
            && connection.to.node() == &partial_id
            && connection.to.port() == "index"
    });
    if !has_iteration
        && graph.nodes.contains_key(&input_id)
        && graph.nodes.contains_key(&partial_id)
    {
        graph.connections.push(VoiceConnection::new(
            PortRef::new(input_id, "iteration"),
            PortRef::new(partial_id, "index"),
        ));
        changed = true;
    }
    changed
}

fn insert_modal_soundboard(graph: &mut AudioGraph) -> Result<(), GraphError> {
    let instrument = NodeId::fixed("instrument");
    let output = NodeId::fixed("output");
    graph.connections.retain(|connection| {
        !(connection.from.node() == &instrument && connection.to.node() == &output)
    });
    let mut resonator_ids = Vec::new();
    for (index, (frequency, q, gain)) in [
        (110.0, 4.0, 0.12),
        (190.0, 5.0, 0.10),
        (320.0, 6.0, 0.09),
        (540.0, 7.0, 0.08),
        (860.0, 8.0, 0.07),
        (1_280.0, 7.0, 0.06),
        (1_900.0, 5.0, 0.05),
        (2_800.0, 4.0, 0.04),
    ]
    .into_iter()
    .enumerate()
    {
        let id = NodeId::new(format!("soundboard-{}", index + 1))?;
        let mut node = AudioNode::new(id.clone(), "audio.resonator");
        node.position = Some([300.0 + index as f64 * 70.0, 340.0]);
        node.params.insert("frequency".to_string(), frequency);
        node.params.insert("q".to_string(), q);
        node.params.insert("gain".to_string(), gain);
        graph.nodes.insert(id.clone(), node);
        graph.connections.push(AudioConnection::new(
            PortRef::new(instrument.clone(), "out"),
            PortRef::new(id.clone(), "in"),
        ));
        resonator_ids.push(id);
    }
    let dry = NodeId::new("soundboard-dry")?;
    let mut dry_node = AudioNode::new(dry.clone(), "audio.gain");
    dry_node.params.insert("gain".to_string(), 0.72);
    dry_node.position = Some([300.0, 460.0]);
    graph.nodes.insert(dry.clone(), dry_node);
    graph.connections.push(AudioConnection::new(
        PortRef::new(instrument, "out"),
        PortRef::new(dry.clone(), "in"),
    ));
    let mix_a = NodeId::new("soundboard-mix-a")?;
    let mut mix_a_node = AudioNode::new(mix_a.clone(), "audio.mix");
    mix_a_node.position = Some([860.0, 340.0]);
    graph.nodes.insert(mix_a.clone(), mix_a_node);
    let mix_b = NodeId::new("soundboard-mix-b")?;
    let mut mix_b_node = AudioNode::new(mix_b.clone(), "audio.mix");
    mix_b_node.position = Some([1_100.0, 340.0]);
    graph.nodes.insert(mix_b.clone(), mix_b_node);
    for (index, resonator) in resonator_ids.iter().take(5).enumerate() {
        let port = if index < 4 {
            format!("in{}", index + 1)
        } else {
            "in".to_string()
        };
        graph.connections.push(AudioConnection::new(
            PortRef::new(resonator.clone(), "out"),
            PortRef::new(mix_a.clone(), port),
        ));
    }
    for (index, resonator) in resonator_ids.iter().skip(5).enumerate() {
        graph.connections.push(AudioConnection::new(
            PortRef::new(resonator.clone(), "out"),
            PortRef::new(mix_b.clone(), format!("in{}", index + 1)),
        ));
    }
    graph.connections.push(AudioConnection::new(
        PortRef::new(mix_a, "out"),
        PortRef::new(mix_b.clone(), "in4"),
    ));
    graph.connections.push(AudioConnection::new(
        PortRef::new(dry, "out"),
        PortRef::new(mix_b.clone(), "in"),
    ));
    graph.connections.push(AudioConnection::new(
        PortRef::new(mix_b, "out"),
        PortRef::new(output, "in"),
    ));
    Ok(())
}

fn upgrade_track_soundboards(project: &mut Project) -> usize {
    let mut upgraded = 0;
    for track in &mut project.tracks {
        let old: Vec<NodeId> = track
            .graph
            .nodes
            .values()
            .filter(|node| node.type_id == "audio.soundboard")
            .map(|node| node.id.clone())
            .collect();
        if old.is_empty() {
            continue;
        }
        for id in old {
            track.graph.nodes.remove(&id);
            track
                .graph
                .connections
                .retain(|connection| connection.from.node() != &id && connection.to.node() != &id);
        }
        if insert_modal_soundboard(&mut track.graph).is_ok() {
            upgraded += 1;
        }
    }
    upgraded
}

fn upgrade_builtin_piano_groups(project: &mut Project) -> usize {
    let mut upgraded = 0;
    let old_physical = project
        .node_groups
        .get("piano.physical")
        .is_some_and(|group| {
            group.name == "Physical Piano Voice"
                && (group
                    .graph
                    .nodes
                    .values()
                    .any(|node| node.type_id == "string.waveguide")
                    || group
                        .graph
                        .nodes
                        .values()
                        .any(|node| node.group.as_deref() == Some("piano.partial"))
                    || !group
                        .graph
                        .nodes
                        .values()
                        .any(|node| node.type_id == "voice.piano_partial"))
        });
    if old_physical {
        project
            .node_groups
            .insert("piano.physical".to_string(), NodeGroup::physical_default());
        upgraded += 1;
    }
    let old_partial = project
        .node_groups
        .get("piano.partial")
        .is_some_and(|group| {
            group.name == "Piano Partial"
                && !group
                    .graph
                    .nodes
                    .values()
                    .any(|node| node.type_id == "math.sin")
        });
    if old_partial {
        project
            .node_groups
            .insert("piano.partial".to_string(), NodeGroup::partial_default());
        upgraded += 1;
    }
    upgraded
}

fn migrate_legacy_repeat_groups(project: &mut Project) -> usize {
    project
        .node_groups
        .get_mut("piano.voice")
        .map_or(0, |group| {
            usize::from(migrate_piano_voice_graph(&mut group.graph))
        })
}

/// 旧版组实例把接口值混在 `params`；迁移到显式 `port_values`，
/// 未声明接口的旧参数直接丢弃。
fn migrate_group_instance_params(project: &mut Project) -> usize {
    let mut migrated = 0;
    let groups = &project.node_groups;
    for track in &mut project.tracks {
        for node in track
            .graph
            .nodes
            .values_mut()
            .filter(|node| node.type_id == "audio.group")
        {
            let declared: BTreeSet<String> = node
                .group
                .as_ref()
                .and_then(|id| groups.get(id))
                .map(|group| {
                    group
                        .interface
                        .inputs
                        .iter()
                        .map(|socket| socket.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            let names: Vec<String> = node.params.keys().cloned().collect();
            for name in names {
                if let Some(value) = node.params.remove(&name) {
                    if declared.contains(&name) {
                        node.port_values.insert(name, value);
                    }
                    migrated += 1;
                }
            }
        }
    }
    migrated
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
            "project.migrate" => self.cmd_project_migrate(),
            "buffer.import" => self.cmd_buffer_import(&request.args),
            "project.save" => self.cmd_project_save(&request.args),
            "project.open" => self.cmd_project_open(&request.args),
            "project.master_plugin" => self.cmd_master_plugin(&request.args),
            "project.master_plugin_off" => self.cmd_master_plugin_off(),
            "track.create" => self.cmd_track_create(&request.args),
            "track.remove" => self.cmd_track_remove(&request.args),
            "node.types" => Ok(CommandResponse::ok(json!({
                "types": voice_types_json(),
                "audio_types": audio_types_json(),
            }))),
            "group.create" => self.cmd_group_create(&request.args),
            "group.duplicate" => self.cmd_group_duplicate(&request.args),
            "group.delete" => self.cmd_group_delete(&request.args),
            "group.rename" => self.cmd_group_rename(&request.args),
            "group.interface" => self.cmd_group_interface(&request.args),
            "group.voice" => self.cmd_group_voice(&request.args),
            "voice.preview" => self.cmd_voice_preview(&request.args),
            "group.from_nodes" => self.cmd_group_from_nodes(&request.args),
            "group.ungroup" => self.cmd_group_ungroup(&request.args),
            "audio_group.create" => self.cmd_audio_group_create(&request.args),
            "audio_group.from_nodes" => self.cmd_audio_group_from_nodes(&request.args),
            "audio_group.from_group_nodes" => self.cmd_audio_group_from_group_nodes(&request.args),
            "audio_group.ungroup" => self.cmd_audio_group_ungroup(&request.args),
            "audio_group.ungroup_from_group" => {
                self.cmd_audio_group_ungroup_from_group(&request.args)
            },
            "audio_group.delete" => self.cmd_audio_group_delete(&request.args),
            "audio_group.rename" => self.cmd_audio_group_rename(&request.args),
            "audio_group.interface" => self.cmd_audio_group_interface(&request.args),
            "audio_group.graph" => self.cmd_audio_group_graph(&request.args),
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
            "node.flags" => self.cmd_node_flags(&request.args),
            "node.frames" => self.cmd_node_frames(&request.args),
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
            node_groups: BTreeMap::from([
                ("piano.voice".to_string(), NodeGroup::piano_default()),
                ("piano.partial".to_string(), NodeGroup::partial_default()),
                ("piano.physical".to_string(), NodeGroup::physical_default()),
            ]),
            audio_groups: BTreeMap::new(),
            master_plugin: None,
            buffers: BTreeMap::new(),
        };
        project.loop_end_tick = project.length_ticks();
        let token = self.snapshot("project.create");
        self.project = Some(project);
        self.next_track = 0;
        self.next_device = 0;
        self.next_clip = 0;
        self.next_note = 0;
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
        let mut sampler: Option<(String, bool)> = None;
        let (instrument, frequency, group_id) = match args.source.as_deref().unwrap_or("silence") {
            "silence" => ("audio.instrument.silence", None, None),
            "piano" => ("audio.group", None, Some("piano.voice".to_string())),
            "physical" => ("audio.group", None, Some("piano.physical".to_string())),
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
                (
                    "audio.group",
                    Some(frequency),
                    Some("sine.voice".to_string()),
                )
            },
            "sampler" | "wavetable" => {
                let buffer_id = args.buffer_id.clone().ok_or_else(|| {
                    CommandError::invalid_args("sampler/wavetable 需要 buffer_id")
                })?;
                if !self
                    .project
                    .as_ref()
                    .is_some_and(|project| project.buffers.contains_key(&buffer_id))
                {
                    return Err(CommandError::not_found(format!(
                        "找不到采样 buffer：{buffer_id}"
                    )));
                }
                let looped = args
                    .looped
                    .unwrap_or(args.source.as_deref() == Some("wavetable"));
                let group_id = format!("sampler.{buffer_id}");
                sampler = Some((buffer_id, looped));
                ("audio.group", None, Some(group_id))
            },
            other => {
                return Err(CommandError::invalid_args(format!(
                    "不支持的 source：{other}"
                )));
            },
        };
        let mut graph = if let Some(group_id) = &group_id {
            aurtrack_graph::AudioGraph::empty_track(group_id)
        } else {
            aurtrack_graph::AudioGraph::track(instrument)
        };
        if group_id.as_deref() == Some("piano.physical") {
            if let Some(node) = graph
                .nodes
                .get_mut(&NodeId::new("instrument").map_err(graph_error)?)
            {
                node.port_values
                    .insert("sample_rate".to_string(), f64::from(sample_rate));
            }
            insert_modal_soundboard(&mut graph).map_err(graph_error)?;
        }
        if let Some(frequency) = frequency {
            if let Some(node) = graph
                .nodes
                .get_mut(&NodeId::new("instrument").map_err(graph_error)?)
            {
                node.port_values.insert("frequency".to_string(), frequency);
            }
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
        if group_id.as_deref() == Some("sine.voice") {
            project
                .node_groups
                .entry("sine.voice".to_string())
                .or_insert_with(NodeGroup::sine_default);
        }
        if group_id.as_deref() == Some("piano.physical") {
            project
                .node_groups
                .entry("piano.physical".to_string())
                .or_insert_with(NodeGroup::physical_default);
        }
        if let Some((buffer_id, looped)) = sampler {
            let group_id = format!("sampler.{buffer_id}");
            project
                .node_groups
                .entry(group_id)
                .or_insert_with(|| NodeGroup::sampler_default(&buffer_id, looped));
        }
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
        let type_id = match args.kind.as_str() {
            "gain" | "audio.gain" => "audio.gain",
            "amp.adsr" | "audio.adsr" => "audio.adsr",
            "filter.lowpass" | "audio.filter.lowpass" => "audio.filter.lowpass",
            "filter.highpass" | "audio.filter.highpass" => "audio.filter.highpass",
            "drive" | "audio.drive" => "audio.drive",
            "delay" | "audio.delay" => "audio.delay",
            "soundboard" | "audio.soundboard" => "audio.soundboard",
            "mix" | "audio.mix" => "audio.mix",
            "plugin" | "audio.plugin" => "audio.plugin",
            "value" | "audio.value" => "audio.value",
            "note" | "audio.note" => "audio.note",
            "audio.lfo" => "audio.lfo",
            "audio.math.add" => "audio.math.add",
            "audio.math.multiply" => "audio.math.multiply",
            "audio.math.map_range" => "audio.math.map_range",
            "audio.math.clamp" => "audio.math.clamp",
            "group" | "audio.group" => "audio.group",
            "audio.group.instance" => "audio.group.instance",
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
        if type_id == "audio.adsr"
            && track_graph
                .device_nodes()
                .map_err(graph_error)?
                .iter()
                .any(|node| node.type_id == "audio.adsr")
        {
            return Err(CommandError::invalid_args("一条轨道暂时只能有一个 ADSR"));
        }
        let token = self.snapshot(command);
        self.next_device += 1;
        let number = self.next_device;
        let device_id = format!("device-{number}");
        let path = format!("/tracks/{}/{segment}/{device_id}", args.track_id);
        let mut node = aurtrack_graph::AudioNode::new(
            NodeId::new(device_id.clone()).map_err(graph_error)?,
            type_id,
        );
        for (name, value) in args.params.unwrap_or_default() {
            node.set_param(&name, value).map_err(graph_error)?;
        }
        if let Some([x, y]) = args.position {
            node.set_position(x, y).map_err(graph_error)?;
        }
        if type_id == "audio.plugin" {
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
        if type_id == "audio.group" {
            let group = args
                .group
                .as_deref()
                .ok_or_else(|| CommandError::invalid_args("group 节点需要 group"))?;
            let exists = self
                .project
                .as_ref()
                .is_some_and(|project| project.node_groups.contains_key(group));
            if !exists {
                return Err(CommandError::not_found(format!("找不到节点组：{group}")));
            }
            node.group = Some(group.to_string());
        }
        if type_id == "audio.group.instance" {
            let group = args
                .group
                .as_deref()
                .ok_or_else(|| CommandError::invalid_args("组实例需要 group"))?;
            let exists = self
                .project
                .as_ref()
                .is_some_and(|project| project.audio_groups.contains_key(group));
            if !exists {
                return Err(CommandError::not_found(format!("找不到 Audio 组：{group}")));
            }
            node.group = Some(group.to_string());
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
        if matches!(
            node.type_id.as_str(),
            "audio.group.instance" | "audio.group"
        ) {
            let group_id = node
                .group
                .clone()
                .ok_or_else(|| CommandError::invalid_args("组节点缺少 group"))?;
            let project = self
                .project
                .as_ref()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            let socket = if node.type_id == "audio.group" {
                project.node_groups.get(&group_id).and_then(|group| {
                    group
                        .interface
                        .inputs
                        .iter()
                        .find(|socket| socket.id == name)
                })
            } else {
                project.audio_groups.get(&group_id).and_then(|group| {
                    group
                        .interface
                        .inputs
                        .iter()
                        .find(|socket| socket.id == name)
                })
            }
            .ok_or_else(|| CommandError::not_found(format!("找不到参数：{path}")))?;
            if matches!(
                socket.ty,
                NodeGroupSocketType::Audio | NodeGroupSocketType::Note
            ) {
                return Err(CommandError::invalid_args(
                    "Audio/Note 接口不能设置数值参数",
                ));
            }
            let minimum = socket.minimum.unwrap_or(f64::NEG_INFINITY);
            let maximum = socket.maximum.unwrap_or(f64::INFINITY);
            if !args.value.is_finite() || args.value < minimum || args.value > maximum {
                return Err(CommandError::invalid_args(format!(
                    "参数 {name} 必须在 {minimum} 到 {maximum} 之间"
                )));
            }
            let token = self.snapshot("param.set");
            let project = self
                .project
                .as_mut()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            let node = project
                .tracks
                .iter_mut()
                .find(|track| track.id == track_id)
                .and_then(|track| track.graph.nodes.get_mut(&node_id))
                .ok_or_else(|| CommandError::not_found(format!("找不到参数：{path}")))?;
            node.port_values.insert(name.to_string(), args.value);
            return Ok(
                CommandResponse::ok(json!({ "path": path, "value": args.value })).with_undo(token),
            );
        }
        if !node.is_device() && !matches!(node.type_id.as_str(), "audio.value" | "audio.lfo") {
            return Err(CommandError::not_found(format!("找不到参数：{path}")));
        }
        let mut probe = aurtrack_graph::AudioNode::clone(node);
        probe.set_param(name, args.value).map_err(graph_error)?;
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

    fn cmd_buffer_import(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ImportArgs {
            path: String,
            #[serde(default)]
            id: Option<String>,
            #[serde(default)]
            name: Option<String>,
        }
        let args: ImportArgs = parse_args(args)?;
        if args.path.trim().is_empty() {
            return Err(CommandError::invalid_args("path 不能为空"));
        }
        let (sample_rate, samples) = read_wav(Path::new(args.path.trim()))?;
        let token = self.snapshot("buffer.import");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let id = args
            .id
            .unwrap_or_else(|| format!("buffer-{}", project.buffers.len() + 1));
        NodeId::new(id.clone()).map_err(graph_error)?;
        let name = args.name.unwrap_or_else(|| id.clone());
        project.buffers.insert(
            id.clone(),
            AudioBuffer {
                id: id.clone(),
                name: name.clone(),
                sample_rate,
                samples,
            },
        );
        let frames = project.buffers[&id].samples.len();
        Ok(CommandResponse::ok(json!({
            "buffer_id": id,
            "name": name,
            "sample_rate": sample_rate,
            "frames": frames,
        }))
        .with_undo(token))
    }

    fn cmd_project_migrate(&mut self) -> Result<CommandResponse, CommandError> {
        let token = self.snapshot("project.migrate");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let changed_groups = migrate_legacy_repeat_groups(project);
        Ok(CommandResponse::ok(json!({ "changed_groups": changed_groups })).with_undo(token))
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
        let mut project = file.project;
        let upgraded_builtin = upgrade_builtin_piano_groups(&mut project);
        let upgraded_soundboards = upgrade_track_soundboards(&mut project);
        project
            .node_groups
            .entry("piano.partial".to_string())
            .or_insert_with(NodeGroup::partial_default);
        project
            .node_groups
            .entry("piano.physical".to_string())
            .or_insert_with(NodeGroup::physical_default);
        let migrated_groups = migrate_legacy_repeat_groups(&mut project);
        let migrated_params = migrate_group_instance_params(&mut project);
        let (next_track, next_device, next_clip, next_note) = validate_project(&project)?;
        let token = self.snapshot("project.open");
        self.project = Some(project);
        self.next_track = next_track;
        self.next_device = next_device;
        self.next_clip = next_clip;
        self.next_note = next_note;
        Ok(CommandResponse::ok(json!({
            "path": path,
            "migrated_groups": migrated_groups,
            "migrated_params": migrated_params,
            "upgraded_builtin": upgraded_builtin,
            "upgraded_soundboards": upgraded_soundboards,
            "project": self.project_summary(),
        }))
        .with_undo(token))
    }

    fn cmd_group_duplicate(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct DuplicateArgs {
            source_id: String,
            group_id: String,
            #[serde(default)]
            name: Option<String>,
        }
        let args: DuplicateArgs = parse_args(args)?;
        let source_id = args.source_id.trim();
        let new_id = args.group_id.trim();
        if source_id.is_empty() || new_id.is_empty() {
            return Err(CommandError::invalid_args("source_id 和 group_id 不能为空"));
        }
        NodeId::new(new_id).map_err(graph_error)?;
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let source = project
            .node_groups
            .get(source_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点组：{source_id}")))?;
        if project.node_groups.contains_key(new_id) {
            return Err(CommandError::invalid_args(format!(
                "节点组已存在：{new_id}"
            )));
        }
        let name = args
            .name
            .unwrap_or_else(|| format!("{} 副本", source.name))
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }
        let mut group = source.clone();
        group.id = new_id.to_string();
        group.name = name;
        group
            .validate_with_groups(&project.node_groups)
            .map_err(graph_error)?;
        let token = self.snapshot("group.duplicate");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        project.node_groups.insert(new_id.to_string(), group);
        Ok(CommandResponse::ok(json!({ "group_id": new_id })).with_undo(token))
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
                node.type_id == "audio.group" && node.group() == Some(args.group_id.as_str())
            })
        }) || project.node_groups.values().any(|group| {
            group.id != args.group_id
                && group.graph.nodes.values().any(|node| {
                    node.type_id == "voice.group"
                        && node.group.as_deref() == Some(args.group_id.as_str())
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

    fn cmd_voice_preview(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PreviewArgs {
            group_id: String,
            node_id: String,
            #[serde(default)]
            points: Option<u32>,
            #[serde(default)]
            pitch: Option<f64>,
            #[serde(default)]
            velocity: Option<f64>,
            #[serde(default)]
            time: Option<f64>,
        }
        let args: PreviewArgs = parse_args(args)?;
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let group = project
            .node_groups
            .get(&args.group_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点组：{}", args.group_id)))?;
        let plan = compile_voice(
            &group.graph,
            &group.interface,
            &project.node_groups,
            &project.buffers,
        )
        .map_err(graph_error)?;
        let points = args.points.unwrap_or(32).clamp(2, 256) as usize;
        let pitch = args.pitch.unwrap_or(261.63);
        let velocity = args.velocity.unwrap_or(0.8);
        let base_time = args.time.unwrap_or(0.0).max(0.0);
        let mut values = Vec::with_capacity(points);
        for index in 0..points {
            #[allow(clippy::cast_precision_loss)]
            let time = base_time + 0.5 * index as f64 / (points - 1) as f64;
            let context = aurtrack_engine::VoiceContext {
                pitch,
                velocity,
                local_time: time,
                gate: true,
                seed: 0,
                note_index: 0,
                iteration: 0,
            };
            values.push(plan.sample_node(&args.node_id, context).unwrap_or(0.0));
        }
        let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
        let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let repeat_traces = plan.trace_repeats(aurtrack_engine::VoiceContext {
            pitch,
            velocity,
            local_time: base_time,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        });
        Ok(CommandResponse::ok(json!({
            "node_id": args.node_id,
            "values": values,
            "minimum": minimum,
            "maximum": maximum,
            "repeat_traces": repeat_traces
                .into_iter()
                .map(|trace| json!({
                    "zone": trace.zone,
                    "iterations": trace.iterations,
                }))
                .collect::<Vec<_>>(),
        })))
    }

    fn cmd_group_from_nodes(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FromNodesArgs {
            source_group_id: String,
            node_ids: Vec<String>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            group_id: Option<String>,
        }

        struct InputBoundary {
            socket: NodeGroupSocket,
            target: PortRef,
            external: PortRef,
        }
        struct OutputBoundary {
            socket: NodeGroupSocket,
            source: PortRef,
            external: PortRef,
        }

        let args: FromNodesArgs = parse_args(args)?;
        if args.node_ids.is_empty() {
            return Err(CommandError::invalid_args("node_ids 不能为空"));
        }
        let token = self.snapshot("group.from_nodes");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut groups = project.node_groups.clone();
        let source = groups.get(&args.source_group_id).cloned().ok_or_else(|| {
            CommandError::not_found(format!("找不到节点组：{}", args.source_group_id))
        })?;

        let mut selected = BTreeSet::new();
        for raw in &args.node_ids {
            let id = NodeId::new(raw.clone()).map_err(graph_error)?;
            let node = source
                .graph
                .nodes
                .get(&id)
                .ok_or_else(|| CommandError::not_found(format!("找不到节点：{id}")))?;
            if matches!(node.type_id.as_str(), "group.input" | "group.output") {
                return Err(CommandError::invalid_args("不能把组输入/输出打成组"));
            }
            if node.type_id.starts_with("voice.zone.") || node.zone.is_some() {
                return Err(CommandError::invalid_args("不能把 Repeat Zone 节点打成组"));
            }
            if node.group.as_deref() == Some(args.source_group_id.as_str()) {
                return Err(CommandError::invalid_args("不能把组自身放进子组"));
            }
            selected.insert(id);
        }

        let group_id = if let Some(id) = args.group_id {
            NodeId::new(id.clone()).map_err(graph_error)?;
            if groups.contains_key(&id) {
                return Err(CommandError::invalid_args(format!("节点组已存在：{id}")));
            }
            id
        } else {
            let mut number = groups.len() + 1;
            loop {
                let id = format!("group-{number}");
                if !groups.contains_key(&id) {
                    break id;
                }
                number += 1;
            }
        };
        let name = args
            .name
            .unwrap_or_else(|| format!("Group {group_id}"))
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for connection in &source.graph.connections {
            let from_selected = selected.contains(connection.from().node());
            let to_selected = selected.contains(connection.to().node());
            if from_selected == to_selected {
                continue;
            }
            if to_selected {
                let target_node = source
                    .graph
                    .nodes
                    .get(connection.to().node())
                    .ok_or_else(|| CommandError::not_found("连线目标节点不存在"))?;
                let ports =
                    node_inputs(target_node, &source.interface, &groups).map_err(graph_error)?;
                let port = ports
                    .iter()
                    .find(|port| port.id == connection.to().port())
                    .ok_or_else(|| CommandError::invalid_args("连线目标端口不存在"))?;
                let socket =
                    NodeGroupSocket::new(&format!("in-{}", inputs.len() + 1), &port.name, port.ty);
                inputs.push(InputBoundary {
                    socket,
                    target: connection.to().clone(),
                    external: connection.from().clone(),
                });
            } else {
                let source_node = source
                    .graph
                    .nodes
                    .get(connection.from().node())
                    .ok_or_else(|| CommandError::not_found("连线来源节点不存在"))?;
                let ports =
                    node_outputs(source_node, &source.interface, &groups).map_err(graph_error)?;
                let port = ports
                    .iter()
                    .find(|port| port.id == connection.from().port())
                    .ok_or_else(|| CommandError::invalid_args("连线来源端口不存在"))?;
                let socket = NodeGroupSocket::new(
                    &format!("out-{}", outputs.len() + 1),
                    &port.name,
                    port.ty,
                );
                outputs.push(OutputBoundary {
                    socket,
                    source: connection.from().clone(),
                    external: connection.to().clone(),
                });
            }
        }

        let mut interface = NodeGroupInterface::empty();
        interface.inputs = inputs
            .iter()
            .map(|boundary| boundary.socket.clone())
            .collect();
        interface.outputs = outputs
            .iter()
            .map(|boundary| boundary.socket.clone())
            .collect();
        let mut child = VoiceGraph::shell(&interface);
        for id in &selected {
            if let Some(node) = source.graph.nodes.get(id) {
                let mut node = node.clone();
                node.frame = None;
                child.nodes.insert(id.clone(), node);
            }
        }
        for connection in &source.graph.connections {
            if selected.contains(connection.from().node())
                && selected.contains(connection.to().node())
            {
                child.connections.push(connection.clone());
            }
        }
        let input_node_id = NodeId::new("voice-in").map_err(graph_error)?;
        let output_node_id = NodeId::new("out").map_err(graph_error)?;
        for boundary in &inputs {
            child.connections.push(VoiceConnection::new(
                PortRef::new(input_node_id.clone(), boundary.socket.id.clone()),
                boundary.target.clone(),
            ));
        }
        for boundary in &outputs {
            child.connections.push(VoiceConnection::new(
                boundary.source.clone(),
                PortRef::new(output_node_id.clone(), boundary.socket.id.clone()),
            ));
        }
        let child_group = NodeGroup::new(group_id.clone(), name.clone(), interface, child);
        child_group
            .validate_with_groups(&groups)
            .map_err(graph_error)?;

        groups.insert(group_id.clone(), child_group);
        let parent = groups
            .get_mut(&args.source_group_id)
            .ok_or_else(|| CommandError::not_found("源节点组不存在"))?;
        for id in &selected {
            parent.graph.nodes.remove(id);
        }
        parent.graph.connections.retain(|connection| {
            !selected.contains(connection.from().node())
                && !selected.contains(connection.to().node())
        });

        let mut number = 1;
        let group_node_id = loop {
            let candidate = NodeId::new(format!("group-{number}")).map_err(graph_error)?;
            if !parent.graph.nodes.contains_key(&candidate) {
                break candidate;
            }
            number += 1;
        };
        let positions: Vec<[f64; 2]> = selected
            .iter()
            .filter_map(|id| source.graph.nodes.get(id)?.position)
            .collect();
        let position = if positions.is_empty() {
            [80.0, 80.0]
        } else {
            let count = positions.len() as f64;
            [
                positions.iter().map(|position| position[0]).sum::<f64>() / count,
                positions.iter().map(|position| position[1]).sum::<f64>() / count,
            ]
        };
        let mut group_node = VoiceNode::new(group_node_id.clone(), "voice.group");
        group_node.position = Some(position);
        group_node.group = Some(group_id.clone());
        parent.graph.nodes.insert(group_node_id.clone(), group_node);
        for boundary in &inputs {
            parent.graph.connections.push(VoiceConnection::new(
                boundary.external.clone(),
                PortRef::new(group_node_id.clone(), boundary.socket.id.clone()),
            ));
        }
        for boundary in &outputs {
            parent.graph.connections.push(VoiceConnection::new(
                PortRef::new(group_node_id.clone(), boundary.socket.id.clone()),
                boundary.external.clone(),
            ));
        }
        let parent_interface = parent.interface.clone();
        let parent_graph = parent.graph.clone();
        parent_graph
            .validate_with_groups(&parent_interface, &groups)
            .map_err(graph_error)?;

        project.node_groups = groups;
        Ok(CommandResponse::ok(json!({
            "group_id": group_id,
            "node_id": group_node_id,
            "name": name,
        }))
        .with_undo(token))
    }

    fn cmd_group_ungroup(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct UngroupArgs {
            group_id: String,
            node_id: String,
        }
        let args: UngroupArgs = parse_args(args)?;
        let token = self.snapshot("group.ungroup");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut groups = project.node_groups.clone();
        let source = groups
            .get(&args.group_id)
            .cloned()
            .ok_or_else(|| CommandError::not_found(format!("找不到节点组：{}", args.group_id)))?;
        let group_node_id = NodeId::new(args.node_id.clone()).map_err(graph_error)?;
        let group_node = source
            .graph
            .nodes
            .get(&group_node_id)
            .cloned()
            .ok_or_else(|| CommandError::not_found(format!("找不到节点：{}", args.node_id)))?;
        if group_node.type_id != "voice.group" {
            return Err(CommandError::invalid_args("只有组节点可以解组"));
        }
        let child_id = group_node
            .group
            .clone()
            .ok_or_else(|| CommandError::invalid_args("组节点没有引用节点组"))?;
        let child = groups
            .get(&child_id)
            .cloned()
            .ok_or_else(|| CommandError::not_found(format!("找不到节点组：{child_id}")))?;
        let origin = group_node.position.unwrap_or([0.0, 0.0]);

        let parent = groups
            .get_mut(&args.group_id)
            .ok_or_else(|| CommandError::not_found("源节点组不存在"))?;
        let incoming: Vec<(PortRef, String)> = parent
            .graph
            .connections
            .iter()
            .filter(|connection| connection.to().node() == &group_node_id)
            .map(|connection| {
                (
                    connection.from().clone(),
                    connection.to().port().to_string(),
                )
            })
            .collect();
        let outgoing: Vec<(String, PortRef)> = parent
            .graph
            .connections
            .iter()
            .filter(|connection| connection.from().node() == &group_node_id)
            .map(|connection| {
                (
                    connection.from().port().to_string(),
                    connection.to().clone(),
                )
            })
            .collect();

        let mut id_map: BTreeMap<NodeId, NodeId> = BTreeMap::new();
        for (old_id, child_node) in &child.graph.nodes {
            if matches!(child_node.type_id.as_str(), "group.input" | "group.output") {
                continue;
            }
            let mut suffix = 0;
            let new_id = loop {
                let candidate = if suffix == 0 {
                    old_id.to_string()
                } else {
                    format!("{old_id}-{suffix}")
                };
                let candidate = NodeId::new(candidate).map_err(graph_error)?;
                if !parent.graph.nodes.contains_key(&candidate)
                    && !id_map.values().any(|id| id == &candidate)
                {
                    break candidate;
                }
                suffix += 1;
            };
            id_map.insert(old_id.clone(), new_id);
        }
        for (old_id, new_id) in &id_map {
            let Some(node) = child.graph.nodes.get(old_id) else {
                continue;
            };
            let mut node = node.clone();
            node.id = new_id.clone();
            node.frame = None;
            if let Some(position) = node.position.as_mut() {
                position[0] += origin[0];
                position[1] += origin[1];
            }
            parent.graph.nodes.insert(new_id.clone(), node);
        }
        for connection in &child.graph.connections {
            let (Some(from), Some(to)) = (
                id_map.get(connection.from().node()),
                id_map.get(connection.to().node()),
            ) else {
                continue;
            };
            parent.graph.connections.push(VoiceConnection::new(
                PortRef::new(from.clone(), connection.from().port().to_string()),
                PortRef::new(to.clone(), connection.to().port().to_string()),
            ));
        }
        for socket in &child.interface.inputs {
            let targets: Vec<PortRef> = child
                .graph
                .connections
                .iter()
                .filter(|connection| {
                    connection.from().node().to_string() == "voice-in"
                        && connection.from().port() == socket.id
                })
                .map(|connection| connection.to().clone())
                .collect();
            for (external, port) in &incoming {
                if port != &socket.id {
                    continue;
                }
                for target in &targets {
                    let Some(new_target) = id_map.get(target.node()) else {
                        continue;
                    };
                    parent.graph.connections.push(VoiceConnection::new(
                        external.clone(),
                        PortRef::new(new_target.clone(), target.port().to_string()),
                    ));
                }
            }
        }
        for socket in &child.interface.outputs {
            let sources: Vec<PortRef> = child
                .graph
                .connections
                .iter()
                .filter(|connection| {
                    connection.to().node().to_string() == "out"
                        && connection.to().port() == socket.id
                })
                .map(|connection| connection.from().clone())
                .collect();
            for (port, external) in &outgoing {
                if port != &socket.id {
                    continue;
                }
                for source in &sources {
                    let Some(new_source) = id_map.get(source.node()) else {
                        continue;
                    };
                    parent.graph.connections.push(VoiceConnection::new(
                        PortRef::new(new_source.clone(), source.port().to_string()),
                        external.clone(),
                    ));
                }
            }
        }
        parent.graph.nodes.remove(&group_node_id);
        parent.graph.connections.retain(|connection| {
            connection.from().node() != &group_node_id && connection.to().node() != &group_node_id
        });
        let parent_interface = parent.interface.clone();
        let parent_graph = parent.graph.clone();
        parent_graph
            .validate_with_groups(&parent_interface, &groups)
            .map_err(graph_error)?;
        project.node_groups = groups;
        Ok(CommandResponse::ok(json!({
            "group_id": args.group_id,
            "node_ids": id_map.values().map(ToString::to_string).collect::<Vec<_>>(),
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

    fn cmd_audio_group_create(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct CreateArgs {
            name: String,
            #[serde(default)]
            group_id: Option<String>,
        }
        let args: CreateArgs = parse_args(args)?;
        let name = args.name.trim();
        if name.is_empty() {
            return Err(CommandError::invalid_args("组名不能为空"));
        }
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut number = project.audio_groups.len() + 1;
        let group_id = loop {
            let candidate = args
                .group_id
                .clone()
                .unwrap_or_else(|| format!("audio-group-{number}"));
            if !project.node_groups.contains_key(&candidate)
                && !project.audio_groups.contains_key(&candidate)
            {
                break candidate;
            }
            number += 1;
        };
        let group = AudioGroup::empty(group_id.clone(), name);
        let mut groups = project.audio_groups.clone();
        groups.insert(group_id.clone(), group.clone());
        group
            .graph
            .validate_with_audio_interface(&project.node_groups, &groups, &group.interface)
            .map_err(graph_error)?;
        let token = self.snapshot("audio_group.create");
        self.project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?
            .audio_groups
            .insert(group_id.clone(), group);
        Ok(CommandResponse::ok(json!({ "group_id": group_id })).with_undo(token))
    }

    fn cmd_audio_group_from_nodes(
        &mut self,
        args: &Value,
    ) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FromNodesArgs {
            source_track_id: String,
            node_ids: Vec<String>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            group_id: Option<String>,
        }

        struct InputBoundary {
            socket: NodeGroupSocket,
            target: PortRef,
            external: PortRef,
        }
        struct OutputBoundary {
            socket: NodeGroupSocket,
            source: PortRef,
            external: PortRef,
        }

        let args: FromNodesArgs = parse_args(args)?;
        if args.node_ids.is_empty() {
            return Err(CommandError::invalid_args("node_ids 不能为空"));
        }
        let token = self.snapshot("audio_group.from_nodes");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut audio_groups = project.audio_groups.clone();
        let track = project
            .tracks
            .iter()
            .find(|track| track.id == args.source_track_id)
            .ok_or_else(|| {
                CommandError::not_found(format!("找不到轨道：{}", args.source_track_id))
            })?;
        let source = track.graph.clone();

        let mut selected = BTreeSet::new();
        for raw in &args.node_ids {
            let id = NodeId::new(raw.clone()).map_err(graph_error)?;
            let node = source
                .nodes
                .get(&id)
                .ok_or_else(|| CommandError::not_found(format!("找不到节点：{id}")))?;
            let node_type = audio_node_type(&node.type_id)
                .ok_or_else(|| GraphError::UnknownAudioNodeType(node.type_id.clone()))
                .map_err(graph_error)?;
            if matches!(
                node_type.category,
                "Instrument" | "Output" | "Group" | "Interface"
            ) {
                return Err(CommandError::invalid_args(
                    "只能把 Audio 效果或控制节点打包成 Audio 组",
                ));
            }
            selected.insert(id);
        }

        let group_id = if let Some(id) = args.group_id {
            NodeId::new(id.clone()).map_err(graph_error)?;
            if audio_groups.contains_key(&id) || project.node_groups.contains_key(&id) {
                return Err(CommandError::invalid_args(format!("节点组已存在：{id}")));
            }
            id
        } else {
            let mut number = audio_groups.len() + 1;
            loop {
                let id = format!("audio-group-{number}");
                if !audio_groups.contains_key(&id) && !project.node_groups.contains_key(&id) {
                    break id;
                }
                number += 1;
            }
        };
        let name = args
            .name
            .unwrap_or_else(|| format!("Audio Group {group_id}"))
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for connection in &source.connections {
            let from_selected = selected.contains(connection.from.node());
            let to_selected = selected.contains(connection.to.node());
            if from_selected == to_selected {
                continue;
            }
            if to_selected {
                let target_node = source
                    .nodes
                    .get(connection.to.node())
                    .ok_or_else(|| CommandError::not_found("连线目标节点不存在"))?;
                let ports = AudioGraph::inputs_with_groups(target_node, &audio_groups, None)
                    .map_err(graph_error)?;
                let port = ports
                    .iter()
                    .find(|port| port.id == connection.to.port())
                    .ok_or_else(|| CommandError::invalid_args("连线目标端口不存在"))?;
                let mut socket =
                    NodeGroupSocket::new(&format!("in-{}", inputs.len() + 1), &port.name, port.ty);
                if port.ty != NodeGroupSocketType::Audio {
                    socket.default = Some(port.default);
                }
                inputs.push(InputBoundary {
                    socket,
                    target: connection.to.clone(),
                    external: connection.from.clone(),
                });
            } else {
                let source_node = source
                    .nodes
                    .get(connection.from.node())
                    .ok_or_else(|| CommandError::not_found("连线来源节点不存在"))?;
                let ports = AudioGraph::outputs_with_groups(source_node, &audio_groups, None)
                    .map_err(graph_error)?;
                let port = ports
                    .iter()
                    .find(|port| port.id == connection.from.port())
                    .ok_or_else(|| CommandError::invalid_args("连线来源端口不存在"))?;
                outputs.push(OutputBoundary {
                    socket: NodeGroupSocket::new(
                        &format!("out-{}", outputs.len() + 1),
                        &port.name,
                        port.ty,
                    ),
                    source: connection.from.clone(),
                    external: connection.to.clone(),
                });
            }
        }
        if outputs.is_empty() {
            return Err(CommandError::invalid_args(
                "选中的节点没有连到组外，无法确定 Audio 组输出",
            ));
        }

        let mut interface = NodeGroupInterface::empty();
        interface.inputs = inputs
            .iter()
            .map(|boundary| boundary.socket.clone())
            .collect();
        interface.outputs = outputs
            .iter()
            .map(|boundary| boundary.socket.clone())
            .collect();
        let mut child_nodes = BTreeMap::new();
        for id in &selected {
            if let Some(node) = source.nodes.get(id) {
                let mut node = node.clone();
                node.frame = None;
                child_nodes.insert(id.clone(), node);
            }
        }
        let input_id = NodeId::new("group-input").map_err(graph_error)?;
        let output_id = NodeId::new("group-output").map_err(graph_error)?;
        child_nodes.insert(
            input_id.clone(),
            AudioNode::new(input_id.clone(), "audio.group.input"),
        );
        child_nodes.insert(
            output_id.clone(),
            AudioNode::new(output_id.clone(), "audio.group.output"),
        );
        let mut child_connections = Vec::new();
        for connection in &source.connections {
            if selected.contains(connection.from.node()) && selected.contains(connection.to.node())
            {
                child_connections.push(connection.clone());
            }
        }
        for boundary in &inputs {
            child_connections.push(AudioConnection::new(
                PortRef::new(input_id.clone(), boundary.socket.id.clone()),
                boundary.target.clone(),
            ));
        }
        for boundary in &outputs {
            child_connections.push(AudioConnection::new(
                boundary.source.clone(),
                PortRef::new(output_id.clone(), boundary.socket.id.clone()),
            ));
        }
        let child = AudioGroup::new(
            group_id.clone(),
            name.clone(),
            interface,
            AudioGraph::new(child_nodes, child_connections, BTreeMap::new()),
        );
        child
            .graph
            .validate_with_audio_interface(&project.node_groups, &audio_groups, &child.interface)
            .map_err(graph_error)?;
        audio_groups.insert(group_id.clone(), child);

        let parent_index = project
            .tracks
            .iter()
            .position(|track| track.id == args.source_track_id)
            .ok_or_else(|| CommandError::not_found("源轨道不存在"))?;
        let parent = &mut project.tracks[parent_index];
        for id in &selected {
            parent.graph.nodes.remove(id);
        }
        parent.graph.connections.retain(|connection| {
            !selected.contains(connection.from.node()) && !selected.contains(connection.to.node())
        });
        let mut number = 1;
        let group_node_id = loop {
            let candidate = NodeId::new(format!("audio-group-{number}")).map_err(graph_error)?;
            if !parent.graph.nodes.contains_key(&candidate) {
                break candidate;
            }
            number += 1;
        };
        let positions: Vec<[f64; 2]> = selected
            .iter()
            .filter_map(|id| source.nodes.get(id)?.position)
            .collect();
        let position = if positions.is_empty() {
            [80.0, 80.0]
        } else {
            let count = positions.len() as f64;
            [
                positions.iter().map(|position| position[0]).sum::<f64>() / count,
                positions.iter().map(|position| position[1]).sum::<f64>() / count,
            ]
        };
        let mut group_node = AudioNode::new(group_node_id.clone(), "audio.group.instance");
        group_node.position = Some(position);
        group_node.group = Some(group_id.clone());
        parent.graph.nodes.insert(group_node_id.clone(), group_node);
        for boundary in &inputs {
            parent.graph.connections.push(AudioConnection::new(
                boundary.external.clone(),
                PortRef::new(group_node_id.clone(), boundary.socket.id.clone()),
            ));
        }
        for boundary in &outputs {
            parent.graph.connections.push(AudioConnection::new(
                PortRef::new(group_node_id.clone(), boundary.socket.id.clone()),
                boundary.external.clone(),
            ));
        }
        let parent_graph = parent.graph.clone();
        parent_graph
            .validate_with_audio_groups(&project.node_groups, &audio_groups)
            .map_err(graph_error)?;
        project.audio_groups = audio_groups;

        Ok(CommandResponse::ok(json!({
            "group_id": group_id,
            "node_id": group_node_id,
            "name": name,
        }))
        .with_undo(token))
    }

    fn cmd_audio_group_from_group_nodes(
        &mut self,
        args: &Value,
    ) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FromNodesArgs {
            source_group_id: String,
            node_ids: Vec<String>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            group_id: Option<String>,
        }
        struct InputBoundary {
            socket: NodeGroupSocket,
            target: PortRef,
            external: PortRef,
        }
        struct OutputBoundary {
            socket: NodeGroupSocket,
            source: PortRef,
            external: PortRef,
        }

        let args: FromNodesArgs = parse_args(args)?;
        if args.node_ids.is_empty() {
            return Err(CommandError::invalid_args("node_ids 不能为空"));
        }
        let token = self.snapshot("audio_group.from_group_nodes");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let mut audio_groups = project.audio_groups.clone();
        let source_group = audio_groups
            .get(&args.source_group_id)
            .cloned()
            .ok_or_else(|| {
                CommandError::not_found(format!("找不到 Audio 组：{}", args.source_group_id))
            })?;
        let source = source_group.graph.clone();
        let source_interface = source_group.interface.clone();

        let mut selected = BTreeSet::new();
        for raw in &args.node_ids {
            let id = NodeId::new(raw.clone()).map_err(graph_error)?;
            let node = source
                .nodes
                .get(&id)
                .ok_or_else(|| CommandError::not_found(format!("找不到节点：{id}")))?;
            let node_type = audio_node_type(&node.type_id)
                .ok_or_else(|| GraphError::UnknownAudioNodeType(node.type_id.clone()))
                .map_err(graph_error)?;
            if matches!(
                node_type.category,
                "Instrument" | "Output" | "Group" | "Interface"
            ) {
                return Err(CommandError::invalid_args(
                    "只能把 Audio 效果或控制节点打包成 Audio 组",
                ));
            }
            selected.insert(id);
        }

        let group_id = if let Some(id) = args.group_id {
            NodeId::new(id.clone()).map_err(graph_error)?;
            if audio_groups.contains_key(&id) || project.node_groups.contains_key(&id) {
                return Err(CommandError::invalid_args(format!("节点组已存在：{id}")));
            }
            id
        } else {
            let mut number = audio_groups.len() + 1;
            loop {
                let id = format!("audio-group-{number}");
                if !audio_groups.contains_key(&id) && !project.node_groups.contains_key(&id) {
                    break id;
                }
                number += 1;
            }
        };
        let name = args
            .name
            .unwrap_or_else(|| format!("Audio Group {group_id}"))
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for connection in &source.connections {
            let from_selected = selected.contains(connection.from.node());
            let to_selected = selected.contains(connection.to.node());
            if from_selected == to_selected {
                continue;
            }
            if to_selected {
                let target_node = source
                    .nodes
                    .get(connection.to.node())
                    .ok_or_else(|| CommandError::not_found("连线目标节点不存在"))?;
                let ports = AudioGraph::inputs_with_groups(
                    target_node,
                    &audio_groups,
                    Some(&source_interface),
                )
                .map_err(graph_error)?;
                let port = ports
                    .iter()
                    .find(|port| port.id == connection.to.port())
                    .ok_or_else(|| CommandError::invalid_args("连线目标端口不存在"))?;
                let mut socket =
                    NodeGroupSocket::new(&format!("in-{}", inputs.len() + 1), &port.name, port.ty);
                if port.ty != NodeGroupSocketType::Audio {
                    socket.default = Some(port.default);
                }
                inputs.push(InputBoundary {
                    socket,
                    target: connection.to.clone(),
                    external: connection.from.clone(),
                });
            } else {
                let source_node = source
                    .nodes
                    .get(connection.from.node())
                    .ok_or_else(|| CommandError::not_found("连线来源节点不存在"))?;
                let ports = AudioGraph::outputs_with_groups(
                    source_node,
                    &audio_groups,
                    Some(&source_interface),
                )
                .map_err(graph_error)?;
                let port = ports
                    .iter()
                    .find(|port| port.id == connection.from.port())
                    .ok_or_else(|| CommandError::invalid_args("连线来源端口不存在"))?;
                outputs.push(OutputBoundary {
                    socket: NodeGroupSocket::new(
                        &format!("out-{}", outputs.len() + 1),
                        &port.name,
                        port.ty,
                    ),
                    source: connection.from.clone(),
                    external: connection.to.clone(),
                });
            }
        }
        if outputs.is_empty() {
            return Err(CommandError::invalid_args(
                "选中的节点没有连到组外，无法确定 Audio 组输出",
            ));
        }

        let mut interface = NodeGroupInterface::empty();
        interface.inputs = inputs
            .iter()
            .map(|boundary| boundary.socket.clone())
            .collect();
        interface.outputs = outputs
            .iter()
            .map(|boundary| boundary.socket.clone())
            .collect();
        let mut child_nodes = BTreeMap::new();
        for id in &selected {
            if let Some(node) = source.nodes.get(id) {
                let mut node = node.clone();
                node.frame = None;
                child_nodes.insert(id.clone(), node);
            }
        }
        let input_id = NodeId::new("group-input").map_err(graph_error)?;
        let output_id = NodeId::new("group-output").map_err(graph_error)?;
        child_nodes.insert(
            input_id.clone(),
            AudioNode::new(input_id.clone(), "audio.group.input"),
        );
        child_nodes.insert(
            output_id.clone(),
            AudioNode::new(output_id.clone(), "audio.group.output"),
        );
        let mut child_connections = Vec::new();
        for connection in &source.connections {
            if selected.contains(connection.from.node()) && selected.contains(connection.to.node())
            {
                child_connections.push(connection.clone());
            }
        }
        for boundary in &inputs {
            child_connections.push(AudioConnection::new(
                PortRef::new(input_id.clone(), boundary.socket.id.clone()),
                boundary.target.clone(),
            ));
        }
        for boundary in &outputs {
            child_connections.push(AudioConnection::new(
                boundary.source.clone(),
                PortRef::new(output_id.clone(), boundary.socket.id.clone()),
            ));
        }
        let child = AudioGroup::new(
            group_id.clone(),
            name.clone(),
            interface.clone(),
            AudioGraph::new(child_nodes, child_connections, BTreeMap::new()),
        );
        child
            .graph
            .validate_with_audio_interface(&project.node_groups, &audio_groups, &interface)
            .map_err(graph_error)?;
        audio_groups.insert(group_id.clone(), child);

        let parent = audio_groups
            .get_mut(&args.source_group_id)
            .ok_or_else(|| CommandError::not_found("源 Audio 组不存在"))?;
        for id in &selected {
            parent.graph.nodes.remove(id);
        }
        parent.graph.connections.retain(|connection| {
            !selected.contains(connection.from.node()) && !selected.contains(connection.to.node())
        });
        let mut number = 1;
        let group_node_id = loop {
            let candidate = NodeId::new(format!("audio-group-{number}")).map_err(graph_error)?;
            if !parent.graph.nodes.contains_key(&candidate) {
                break candidate;
            }
            number += 1;
        };
        let positions: Vec<[f64; 2]> = selected
            .iter()
            .filter_map(|id| source.nodes.get(id)?.position)
            .collect();
        let position = if positions.is_empty() {
            [80.0, 80.0]
        } else {
            let count = positions.len() as f64;
            [
                positions.iter().map(|position| position[0]).sum::<f64>() / count,
                positions.iter().map(|position| position[1]).sum::<f64>() / count,
            ]
        };
        let mut group_node = AudioNode::new(group_node_id.clone(), "audio.group.instance");
        group_node.position = Some(position);
        group_node.group = Some(group_id.clone());
        parent.graph.nodes.insert(group_node_id.clone(), group_node);
        for boundary in &inputs {
            parent.graph.connections.push(AudioConnection::new(
                boundary.external.clone(),
                PortRef::new(group_node_id.clone(), boundary.socket.id.clone()),
            ));
        }
        for boundary in &outputs {
            parent.graph.connections.push(AudioConnection::new(
                PortRef::new(group_node_id.clone(), boundary.socket.id.clone()),
                boundary.external.clone(),
            ));
        }
        let parent_graph = parent.graph.clone();
        let parent_interface = parent.interface.clone();
        parent_graph
            .validate_with_audio_interface(&project.node_groups, &audio_groups, &parent_interface)
            .map_err(graph_error)?;
        project.audio_groups = audio_groups;

        Ok(CommandResponse::ok(json!({
            "group_id": group_id,
            "node_id": group_node_id,
            "name": name,
        }))
        .with_undo(token))
    }

    fn cmd_audio_group_ungroup(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct UngroupArgs {
            track_id: String,
            node_id: String,
        }
        let args: UngroupArgs = parse_args(args)?;
        let token = self.snapshot("audio_group.ungroup");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track_index = project
            .tracks
            .iter()
            .position(|track| track.id == args.track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{}", args.track_id)))?;
        let mut graph = project.tracks[track_index].graph.clone();
        let instance_id = NodeId::new(args.node_id.clone()).map_err(graph_error)?;
        let instance = graph
            .nodes
            .get(&instance_id)
            .cloned()
            .ok_or_else(|| CommandError::not_found(format!("找不到节点：{}", args.node_id)))?;
        if instance.type_id != "audio.group.instance" {
            return Err(CommandError::invalid_args("只有 Audio 组实例可以解组"));
        }
        let group_id = instance
            .group
            .clone()
            .ok_or_else(|| CommandError::invalid_args("组实例缺少 group"))?;
        let group = project
            .audio_groups
            .get(&group_id)
            .cloned()
            .ok_or_else(|| CommandError::not_found(format!("找不到 Audio 组：{group_id}")))?;

        let mut mapping = BTreeMap::new();
        let mut counter = 1;
        for (id, node) in &group.graph.nodes {
            if matches!(
                node.type_id.as_str(),
                "audio.group.input" | "audio.group.output"
            ) {
                continue;
            }
            let mut new_id = id.clone();
            while graph.nodes.contains_key(&new_id) || new_id == instance_id {
                new_id = NodeId::new(format!("ungroup-{counter}")).map_err(graph_error)?;
                counter += 1;
            }
            mapping.insert(id.clone(), new_id.clone());
        }

        let mut node_ids = Vec::new();
        for (id, new_id) in &mapping {
            let Some(mut node) = group.graph.nodes.get(id).cloned() else {
                continue;
            };
            node.id = new_id.clone();
            node.frame = None;
            graph.nodes.insert(new_id.clone(), node);
            node_ids.push(new_id.clone());
        }

        let mut new_connections = Vec::new();
        for connection in &group.graph.connections {
            let Some(source_node) = group.graph.nodes.get(connection.from.node()) else {
                continue;
            };
            let Some(target_node) = group.graph.nodes.get(connection.to.node()) else {
                continue;
            };
            if source_node.type_id == "audio.group.input"
                && target_node.type_id == "audio.group.output"
            {
                let external_in = graph.connections.iter().find(|candidate| {
                    candidate.to.node() == &instance_id
                        && candidate.to.port() == connection.from.port()
                });
                let external_out = graph.connections.iter().find(|candidate| {
                    candidate.from.node() == &instance_id
                        && candidate.from.port() == connection.to.port()
                });
                if let (Some(external_in), Some(external_out)) = (external_in, external_out) {
                    new_connections.push(AudioConnection::new(
                        external_in.from.clone(),
                        external_out.to.clone(),
                    ));
                }
                continue;
            }
            if source_node.type_id == "audio.group.input" {
                let external = graph.connections.iter().find(|candidate| {
                    candidate.to.node() == &instance_id
                        && candidate.to.port() == connection.from.port()
                });
                let Some(external) = external else {
                    continue;
                };
                let Some(target_id) = mapping.get(connection.to.node()) else {
                    continue;
                };
                new_connections.push(AudioConnection::new(
                    external.from.clone(),
                    PortRef::new(target_id.clone(), connection.to.port().to_string()),
                ));
                continue;
            }
            if target_node.type_id == "audio.group.output" {
                let external = graph.connections.iter().find(|candidate| {
                    candidate.from.node() == &instance_id
                        && candidate.from.port() == connection.to.port()
                });
                let Some(external) = external else {
                    continue;
                };
                let Some(source_id) = mapping.get(connection.from.node()) else {
                    continue;
                };
                new_connections.push(AudioConnection::new(
                    PortRef::new(source_id.clone(), connection.from.port().to_string()),
                    external.to.clone(),
                ));
                continue;
            }
            let (Some(source_id), Some(target_id)) = (
                mapping.get(connection.from.node()),
                mapping.get(connection.to.node()),
            ) else {
                continue;
            };
            new_connections.push(AudioConnection::new(
                PortRef::new(source_id.clone(), connection.from.port().to_string()),
                PortRef::new(target_id.clone(), connection.to.port().to_string()),
            ));
        }

        graph.nodes.remove(&instance_id);
        graph.connections.retain(|connection| {
            connection.from.node() != &instance_id && connection.to.node() != &instance_id
        });
        graph.connections.extend(new_connections);
        graph
            .validate_with_audio_groups(&project.node_groups, &project.audio_groups)
            .map_err(graph_error)?;
        project.tracks[track_index].graph = graph;

        Ok(CommandResponse::ok(json!({
            "node_ids": node_ids,
        }))
        .with_undo(token))
    }

    fn cmd_audio_group_ungroup_from_group(
        &mut self,
        args: &Value,
    ) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct UngroupArgs {
            source_group_id: String,
            node_id: String,
        }
        let args: UngroupArgs = parse_args(args)?;
        let token = self.snapshot("audio_group.ungroup_from_group");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let source = project
            .audio_groups
            .get(&args.source_group_id)
            .cloned()
            .ok_or_else(|| {
                CommandError::not_found(format!("找不到 Audio 组：{}", args.source_group_id))
            })?;
        let instance_id = NodeId::new(args.node_id.clone()).map_err(graph_error)?;
        let mut graph = source.graph.clone();
        let instance = graph
            .nodes
            .get(&instance_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点：{}", args.node_id)))?;
        if instance.type_id != "audio.group.instance" {
            return Err(CommandError::invalid_args("只有 Audio 组实例可以解组"));
        }
        let node_ids = ungroup_audio_instance(&mut graph, &instance_id, &project.audio_groups)
            .map_err(graph_error)?;
        graph
            .validate_with_audio_interface(
                &project.node_groups,
                &project.audio_groups,
                &source.interface,
            )
            .map_err(graph_error)?;
        project
            .audio_groups
            .get_mut(&args.source_group_id)
            .ok_or_else(|| CommandError::not_found("源 Audio 组不存在"))?
            .graph = graph;
        Ok(CommandResponse::ok(json!({ "node_ids": node_ids })).with_undo(token))
    }

    fn cmd_audio_group_delete(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct DeleteArgs {
            group_id: String,
        }
        let args: DeleteArgs = parse_args(args)?;
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        if !project.audio_groups.contains_key(&args.group_id) {
            return Err(CommandError::not_found(format!(
                "找不到 Audio 组：{}",
                args.group_id
            )));
        }
        if project.audio_groups.values().any(|group| {
            group.graph.nodes.values().any(|node| {
                node.type_id == "audio.group.instance"
                    && node.group.as_deref() == Some(args.group_id.as_str())
            })
        }) || project.tracks.iter().any(|track| {
            track.graph.nodes.values().any(|node| {
                node.type_id == "audio.group.instance"
                    && node.group.as_deref() == Some(args.group_id.as_str())
            })
        }) {
            return Err(CommandError::invalid_args("Audio 组仍被其他图引用"));
        }
        let token = self.snapshot("audio_group.delete");
        self.project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?
            .audio_groups
            .remove(&args.group_id);
        Ok(CommandResponse::ok(json!({ "group_id": args.group_id })).with_undo(token))
    }

    fn cmd_audio_group_rename(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RenameArgs {
            group_id: String,
            name: String,
        }
        let args: RenameArgs = parse_args(args)?;
        let name = args.name.trim();
        if name.is_empty() {
            return Err(CommandError::invalid_args("组名不能为空"));
        }
        let token = self.snapshot("audio_group.rename");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let group = project
            .audio_groups
            .get_mut(&args.group_id)
            .ok_or_else(|| {
                CommandError::not_found(format!("找不到 Audio 组：{}", args.group_id))
            })?;
        group.name = name.to_string();
        Ok(
            CommandResponse::ok(json!({ "group_id": args.group_id, "name": name }))
                .with_undo(token),
        )
    }

    fn cmd_audio_group_interface(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct InterfaceArgs {
            group_id: String,
            interface: NodeGroupInterface,
        }
        let args: InterfaceArgs = parse_args(args)?;
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let groups = project.audio_groups.clone();
        let mut probe = project
            .audio_groups
            .get(&args.group_id)
            .cloned()
            .ok_or_else(|| {
                CommandError::not_found(format!("找不到 Audio 组：{}", args.group_id))
            })?;
        probe.interface = args.interface.clone();
        probe
            .graph
            .validate_with_audio_interface(&project.node_groups, &groups, &probe.interface)
            .map_err(graph_error)?;
        let token = self.snapshot("audio_group.interface");
        self.project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?
            .audio_groups
            .get_mut(&args.group_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到 Audio 组：{}", args.group_id)))?
            .interface = args.interface;
        Ok(CommandResponse::ok(json!({ "group_id": args.group_id })).with_undo(token))
    }

    fn cmd_audio_group_graph(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct GraphArgs {
            group_id: String,
            graph: aurtrack_graph::AudioGraph,
        }
        let args: GraphArgs = parse_args(args)?;
        {
            let project = self
                .project
                .as_ref()
                .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
            let interface = project
                .audio_groups
                .get(&args.group_id)
                .map(|group| group.interface.clone())
                .ok_or_else(|| {
                    CommandError::not_found(format!("找不到 Audio 组：{}", args.group_id))
                })?;
            args.graph
                .validate_with_audio_interface(
                    &project.node_groups,
                    &project.audio_groups,
                    &interface,
                )
                .map_err(graph_error)?;
        }
        let token = self.snapshot("audio_group.graph");
        self.project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?
            .audio_groups
            .get_mut(&args.group_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到 Audio 组：{}", args.group_id)))?
            .graph = args.graph;
        Ok(CommandResponse::ok(json!({ "group_id": args.group_id })).with_undo(token))
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
        let node_type = self
            .project
            .as_ref()
            .and_then(|project| project.tracks.iter().find(|track| track.id == track_id))
            .and_then(|track| track.graph.node(&node_id))
            .filter(|node| node.is_device())
            .map(|node| node.type_id.clone());
        let Some(node_type) = node_type else {
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
        if node_type == "audio.mix" {
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
        if !node.is_device() {
            return Err(CommandError::invalid_args("只有设备节点可以移动"));
        }
        let target = track
            .graph
            .node(&before_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到节点：{}", args.before)))?;
        if !aurtrack_graph::AudioGraph::inputs(target)
            .map_err(graph_error)?
            .iter()
            .any(|port| port.id == "in")
        {
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
        let multi_input = track
            .graph
            .nodes
            .get(to.node())
            .is_some_and(|node| aurtrack_graph::audio_multi_input(&node.type_id));
        if !multi_input {
            let previous: Vec<_> = track
                .graph
                .connections
                .iter()
                .filter(|connection| connection.to == to)
                .map(|connection| connection.from.clone())
                .collect();
            for from in previous {
                let _ = track.graph.disconnect(&from, &to);
            }
        }
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

    fn cmd_node_flags(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FlagsArgs {
            track_id: String,
            node_id: String,
            #[serde(default)]
            muted: Option<bool>,
            #[serde(default)]
            collapsed: Option<bool>,
        }
        let args: FlagsArgs = parse_args(args)?;
        if args.muted.is_none() && args.collapsed.is_none() {
            return Err(CommandError::invalid_args(
                "muted 和 collapsed 至少提供一个",
            ));
        }
        let node_id = NodeId::new(args.node_id).map_err(graph_error)?;
        let token = self.snapshot("node.flags");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == args.track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{}", args.track_id)))?;
        track
            .graph
            .set_flags(&node_id, args.muted, args.collapsed)
            .map_err(graph_error)?;
        Ok(CommandResponse::ok(json!({ "node_id": node_id.to_string() })).with_undo(token))
    }

    fn cmd_node_frames(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FramesArgs {
            track_id: String,
            frames: BTreeMap<String, VoiceFrame>,
            #[serde(default)]
            assignments: BTreeMap<String, Option<String>>,
        }
        let args: FramesArgs = parse_args(args)?;
        let token = self.snapshot("node.frames");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == args.track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{}", args.track_id)))?;
        track
            .graph
            .set_frames(args.frames, args.assignments)
            .map_err(graph_error)?;
        Ok(CommandResponse::ok(json!({ "track_id": args.track_id })).with_undo(token))
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
            if !node.is_device()
                || node.is_instrument()
                || node.type_id == "audio.adsr"
                || node.type_id == "audio.output"
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
            let track = project
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{track_id}")))?;
            if track.graph.node(&node_id).is_none() {
                return Err(CommandError::not_found(format!("找不到节点：{path}")));
            }
            parsed.push((track_id.to_string(), node_id));
        }
        let token = self.snapshot("node.remove_many");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        for (track_id, node_id) in parsed {
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
                "source_node": track.active_instrument().map(AudioNode::id),
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
        "audio_groups": &project.audio_groups,
        "tracks": project.tracks.iter().map(|track| json!({
            "id": &track.id,
            "name": &track.name,
            "source": track.source(),
            "source_node": track.active_instrument().map(AudioNode::id),
            "devices": track.devices(),
            "graph": graph_view(&track.graph),
            "clips": track.clips.iter().map(|clip| clip_view(project, clip)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn voice_types_json() -> Vec<Value> {
    aurtrack_graph::VOICE_NODE_TYPES
        .iter()
        .map(|node_type| {
            json!({
                "id": node_type.id,
                "name": node_type.name,
                "category": node_type.category,
                "inputs": node_type.inputs.iter().map(|socket| json!({
                    "id": socket.id,
                    "name": socket.name,
                    "type": socket.ty,
                    "default": socket.default,
                    "minimum": socket.minimum,
                    "maximum": socket.maximum,
                    "step": socket.step,
                    "multi": socket.multi,
                })).collect::<Vec<_>>(),
                "outputs": node_type.outputs.iter().map(|socket| json!({
                    "id": socket.id,
                    "name": socket.name,
                    "type": socket.ty,
                })).collect::<Vec<_>>(),
                "params": node_type.params.iter().map(|param| json!({
                    "name": param.name,
                    "default": param.default,
                    "minimum": param.minimum,
                    "maximum": param.maximum,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn audio_types_json() -> Vec<Value> {
    aurtrack_graph::AUDIO_NODE_TYPES
        .iter()
        .map(|node_type| {
            json!({
                "id": node_type.id,
                "name": node_type.name,
                "category": node_type.category,
                "multi_input": aurtrack_graph::audio_multi_input(node_type.id),
                "inputs": node_type.inputs.iter().map(|socket| json!({
                    "id": socket.id,
                    "name": socket.name,
                    "type": socket.ty,
                    "default": socket.default,
                })).collect::<Vec<_>>(),
                "outputs": node_type.outputs.iter().map(|socket| json!({
                    "id": socket.id,
                    "name": socket.name,
                    "type": socket.ty,
                    "default": socket.default,
                })).collect::<Vec<_>>(),
                "params": node_type.params.iter().map(|param| json!({
                    "name": param.name,
                    "default": param.default,
                    "minimum": param.minimum,
                    "maximum": param.maximum,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn device_kind(type_id: &str) -> &'static str {
    match type_id {
        "audio.instrument.sine" => "instrument.sine",
        "audio.instrument.silence" => "instrument.silence",
        "audio.group" => "voice.group",
        "audio.adsr" => "amp.adsr",
        "audio.gain" | "audio.drive" | "audio.delay" => "gain",
        "audio.mix" => "mix",
        "audio.filter.lowpass" | "audio.filter.highpass" => "filter.lowpass",
        "audio.plugin" => "plugin",
        "audio.value"
        | "audio.note"
        | "audio.lfo"
        | "audio.math.add"
        | "audio.math.multiply"
        | "audio.math.map_range"
        | "audio.math.clamp" => "control",
        _ => "output",
    }
}

fn graph_view(graph: &aurtrack_graph::AudioGraph) -> Value {
    let nodes: serde_json::Map<String, Value> = graph
        .nodes
        .iter()
        .map(|(id, node)| {
            let mut view = json!({
                "id": id,
                "type_id": &node.type_id,
                "kind": device_kind(&node.type_id),
                "params": &node.params,
                "port_values": &node.port_values,
            });
            if let Some(position) = node.position {
                view["position"] = json!(position);
            }
            if let Some(group) = &node.group {
                view["group"] = json!(group);
            }
            if let Some(frame) = &node.frame {
                view["frame"] = json!(frame);
            }
            if let Some(plugin) = &node.plugin {
                view["plugin"] = json!(plugin);
            }
            if node.muted {
                view["muted"] = json!(true);
            }
            if node.collapsed {
                view["collapsed"] = json!(true);
            }
            (id.to_string(), view)
        })
        .collect();
    json!({
        "nodes": nodes,
        "connections": graph.connections,
        "frames": graph.frames,
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

    for group in project.audio_groups.values() {
        group
            .graph
            .validate_with_audio_interface(
                &project.node_groups,
                &project.audio_groups,
                &group.interface,
            )
            .map_err(graph_error)?;
    }

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
        track
            .graph
            .validate_with_audio_groups(&project.node_groups, &project.audio_groups)
            .map_err(graph_error)?;
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
        let (_, _adsr_settings, graph) = track
            .playback_graph_with_buffers(
                &project.node_groups,
                &project.audio_groups,
                &project.buffers,
            )
            .map_err(graph_error)?;
        let pianos = &graph.pianos;
        let piano_notes: Vec<NotePlan> = if pianos.is_empty() {
            Vec::new()
        } else {
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
        };
        for (index, slot) in mix.iter_mut().enumerate() {
            let time = start_seconds + index as f64 / sample_rate_f64;
            let mut voices = [0.0_f64; aurtrack_engine::MAX_VOICES];
            for (voice_index, piano) in pianos.iter().enumerate().take(aurtrack_engine::MAX_VOICES)
            {
                voices[voice_index] = piano.sample(&piano_notes, time);
            }
            if let Some(frequency) = graph.fallback_sine {
                voices[0] += (std::f64::consts::TAU * frequency * time).sin() * 0.25;
            }
            let sample = graph.sample(&voices, project.sample_rate, time);
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

fn read_wav(path: &Path) -> Result<(f64, Vec<f32>), CommandError> {
    let bytes = std::fs::read(path)
        .map_err(|error| CommandError::new("io_error", format!("读采样文件失败：{error}")))?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(CommandError::invalid_args("不是合法的 WAV 文件"));
    }
    let mut position = 12;
    let mut format = 1_u16;
    let mut channels = 1_usize;
    let mut sample_rate = 48_000_u32;
    let mut bits = 16_u16;
    while position + 8 <= bytes.len() {
        let id = &bytes[position..position + 4];
        let size = u32::from_le_bytes([
            bytes[position + 4],
            bytes[position + 5],
            bytes[position + 6],
            bytes[position + 7],
        ]) as usize;
        position += 8;
        if position + size > bytes.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            format = u16::from_le_bytes([bytes[position], bytes[position + 1]]);
            channels = usize::from(u16::from_le_bytes([
                bytes[position + 2],
                bytes[position + 3],
            ]))
            .max(1);
            sample_rate = u32::from_le_bytes([
                bytes[position + 4],
                bytes[position + 5],
                bytes[position + 6],
                bytes[position + 7],
            ]);
            bits = u16::from_le_bytes([bytes[position + 14], bytes[position + 15]]);
        } else if id == b"data" {
            if channels == 0 || !matches!(bits, 16 | 24 | 32) {
                return Err(CommandError::invalid_args("只支持 16/24/32 位 PCM WAV"));
            }
            let bytes_per_sample = usize::from(bits / 8);
            let frame_bytes = bytes_per_sample * channels;
            if frame_bytes == 0 {
                return Err(CommandError::invalid_args("WAV 帧大小非法"));
            }
            let frames = size / frame_bytes;
            let mut samples = Vec::with_capacity(frames);
            for frame in 0..frames {
                let mut sum = 0.0_f32;
                for channel in 0..channels {
                    let start = position + frame * frame_bytes + channel * bytes_per_sample;
                    let value = match (format, bits) {
                        (3, 32) => f32::from_le_bytes([
                            bytes[start],
                            bytes[start + 1],
                            bytes[start + 2],
                            bytes[start + 3],
                        ]),
                        (_, 16) => {
                            let value = i16::from_le_bytes([bytes[start], bytes[start + 1]]);
                            f32::from(value) / 32_768.0
                        },
                        (_, 24) => {
                            let value = i32::from_le_bytes([
                                bytes[start],
                                bytes[start + 1],
                                bytes[start + 2],
                                if bytes[start + 2] & 0x80 == 0 {
                                    0
                                } else {
                                    0xff
                                },
                            ]);
                            value as f32 / 8_388_608.0
                        },
                        (_, 32) => {
                            let value = i32::from_le_bytes([
                                bytes[start],
                                bytes[start + 1],
                                bytes[start + 2],
                                bytes[start + 3],
                            ]);
                            value as f32 / 2_147_483_648.0
                        },
                        _ => 0.0,
                    };
                    sum += value;
                }
                samples.push(sum / channels as f32);
            }
            return Ok((f64::from(sample_rate), samples));
        }
        position += size + (size & 1);
    }
    Err(CommandError::invalid_args("WAV 缺少 data 块"))
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
    #[serde(default)]
    buffer_id: Option<String>,
    #[serde(default)]
    looped: Option<bool>,
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
    #[serde(default)]
    group: Option<String>,
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod repeat_zone_tests {
    use super::*;

    #[test]
    fn sine_group_renders_a_note() {
        let group = NodeGroup::sine_default();
        let plan = compile_voice(
            &group.graph,
            &group.interface,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let context = aurtrack_engine::VoiceContext {
            pitch: 440.0,
            velocity: 0.8,
            local_time: 0.1013,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!(plan.sample(context).abs() > 1e-9);
    }

    #[test]
    fn multi_output_group_maps_each_socket() {
        let input = NodeGroupSocket::new("x", "X", NodeGroupSocketType::Float).with_default(2.0);
        let mut child_interface = NodeGroupInterface::empty();
        child_interface.inputs = vec![input.clone()];
        child_interface.outputs = vec![
            NodeGroupSocket::new("a", "A", NodeGroupSocketType::Float),
            NodeGroupSocket::new("b", "B", NodeGroupSocketType::Float),
        ];
        let mut child_graph = VoiceGraph::shell(&child_interface);
        let one = NodeId::new("one").unwrap();
        let mut one_node = VoiceNode::new(one.clone(), "value.float");
        one_node.params.insert("value".to_string(), 1.0);
        child_graph.nodes.insert(one.clone(), one_node);
        child_graph.connections = vec![
            VoiceConnection::new(
                PortRef::new(NodeId::fixed("voice-in"), "x"),
                PortRef::new(NodeId::fixed("out"), "a"),
            ),
            VoiceConnection::new(
                PortRef::new(one, "out"),
                PortRef::new(NodeId::fixed("out"), "b"),
            ),
        ];
        let child = NodeGroup::new("multi", "Multi", child_interface, child_graph);
        let mut groups = BTreeMap::new();
        groups.insert("multi".to_string(), child);

        let mut parent_interface = NodeGroupInterface::empty();
        parent_interface.inputs = vec![input];
        parent_interface.outputs = vec![NodeGroupSocket::new(
            "audio",
            "Audio",
            NodeGroupSocketType::Float,
        )];
        let mut parent = VoiceGraph::shell(&parent_interface);
        let instance = NodeId::new("instance").unwrap();
        let add = NodeId::new("add").unwrap();
        let mut instance_node = VoiceNode::new(instance.clone(), "voice.group");
        instance_node.group = Some("multi".to_string());
        parent.nodes.insert(instance.clone(), instance_node);
        parent
            .nodes
            .insert(add.clone(), VoiceNode::new(add.clone(), "voice.add"));
        parent.connections = vec![
            VoiceConnection::new(
                PortRef::new(NodeId::fixed("voice-in"), "x"),
                PortRef::new(instance.clone(), "x"),
            ),
            VoiceConnection::new(
                PortRef::new(instance.clone(), "a"),
                PortRef::new(add.clone(), "in1"),
            ),
            VoiceConnection::new(
                PortRef::new(instance, "b"),
                PortRef::new(add.clone(), "in2"),
            ),
            VoiceConnection::new(
                PortRef::new(add, "out"),
                PortRef::new(NodeId::fixed("out"), "audio"),
            ),
        ];
        let plan = compile_voice(&parent, &parent_interface, &groups, &BTreeMap::new()).unwrap();
        let context = aurtrack_engine::VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn repeat_zone_compiles_note_pair() {
        let mut graph = VoiceGraph::piano_default();
        let note = NodeGroupSocket::new("note-carry", "Note", NodeGroupSocketType::Note);
        for id in ["partials", "p-in"] {
            if let Some(node) = graph.nodes.get_mut(&NodeId::new(id).unwrap()) {
                node.zone_inputs.push(note.clone());
            }
        }
        for id in ["partials", "p-out"] {
            if let Some(node) = graph.nodes.get_mut(&NodeId::new(id).unwrap()) {
                node.zone_outputs.push(note.clone());
            }
        }
        let plan = compile_voice(
            &graph,
            &NodeGroupInterface::piano(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(
            plan.nodes
                .iter()
                .any(|node| matches!(node.op, aurtrack_engine::VoiceOp::Repeat { .. }))
        );
    }

    #[test]
    fn repeat_zone_compiles_multiple_pairs() {
        let mut graph = VoiceGraph::piano_default();
        let aux = NodeGroupSocket::new("aux", "Aux", NodeGroupSocketType::Float);
        for id in ["partials", "p-in"] {
            if let Some(node) = graph.nodes.get_mut(&NodeId::new(id).unwrap()) {
                let mut sockets = aurtrack_graph::zone_input_sockets(node);
                sockets.push(aux.clone());
                node.zone_inputs = sockets;
            }
        }
        for id in ["partials", "p-out"] {
            if let Some(node) = graph.nodes.get_mut(&NodeId::new(id).unwrap()) {
                let mut sockets = aurtrack_graph::zone_output_sockets(node);
                sockets.push(aux.clone());
                node.zone_outputs = sockets;
            }
        }
        let mut value = VoiceNode::new(NodeId::new("aux-value").unwrap(), "value.float");
        value.params.insert("value".to_string(), 5.0);
        value.zone = Some("partials".to_string());
        graph.nodes.insert(value.id.clone(), value);
        graph.connections.push(VoiceConnection::new(
            PortRef::new(NodeId::new("aux-value").unwrap(), "out"),
            PortRef::new(NodeId::new("p-out").unwrap(), "aux"),
        ));

        let plan = compile_voice(
            &graph,
            &NodeGroupInterface::piano(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(
            plan.nodes
                .iter()
                .any(|node| matches!(node.op, aurtrack_engine::VoiceOp::Repeat { .. }))
        );
        assert!(
            plan.nodes
                .iter()
                .any(|node| matches!(node.op, aurtrack_engine::VoiceOp::RepeatResult { .. }))
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod buffer_tests {
    use super::*;

    #[test]
    fn physical_piano_track_renders() {
        let wav = std::env::temp_dir()
            .join(format!("aurtrack-physical-{}.wav", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let mut session = Session::default();
        session
            .execute(&CommandRequest {
                cmd: "project.create".to_string(),
                args: json!({ "name": "physical" }),
            })
            .unwrap();
        let project = session.project().unwrap();
        let physical = &project.node_groups["piano.physical"];
        assert!(
            !physical
                .graph
                .nodes
                .values()
                .any(|node| node.type_id == "string.waveguide")
        );
        assert!(
            physical
                .graph
                .nodes
                .values()
                .any(|node| node.type_id == "voice.piano_partial")
        );
        assert!(
            project.node_groups["piano.partial"]
                .graph
                .nodes
                .values()
                .any(|node| node.type_id == "math.sin")
        );
        session
            .execute(&CommandRequest {
                cmd: "track.create".to_string(),
                args: json!({ "name": "piano", "source": "physical" }),
            })
            .unwrap();
        session
            .execute(&CommandRequest {
                cmd: "clip.create".to_string(),
                args: json!({ "track_id": "track-1", "start": 0.0, "duration": 1.0 }),
            })
            .unwrap();
        session
            .execute(&CommandRequest {
                cmd: "note.add".to_string(),
                args: json!({
                    "path": "/tracks/track-1/clips/clip-1",
                    "pitch": 69,
                    "start": 0.0,
                    "duration": 0.3,
                    "velocity": 0.9
                }),
            })
            .unwrap();
        let rendered = session
            .execute(&CommandRequest {
                cmd: "render.segment".to_string(),
                args: json!({ "duration_seconds": 0.5, "path": wav }),
            })
            .unwrap();
        assert!(rendered.data["peak"].as_f64().unwrap() > 0.0);
        let source = session.project.as_ref().unwrap().tracks[0].source();
        assert!(matches!(source, Source::Physical));
        let project = session.project.as_ref().unwrap();
        assert_eq!(
            project.tracks[0]
                .graph
                .nodes
                .values()
                .filter(|node| node.type_id == "audio.resonator")
                .count(),
            8
        );
        let (_, _, plan) = project.tracks[0]
            .playback_graph(&project.node_groups, &project.audio_groups)
            .unwrap();
        assert!(
            plan.pianos[0]
                .voice
                .as_ref()
                .is_some_and(|voice| voice.nodes.len() < 200),
            "物理钢琴的展开节点数应保持在线性成本范围"
        );
        assert!(
            project.tracks[0]
                .graph
                .nodes
                .values()
                .all(|node| node.type_id != "audio.soundboard")
        );
        session
            .execute(&CommandRequest {
                cmd: "param.set".to_string(),
                args: json!({
                    "path": "/tracks/track-1/nodes/instrument/params/pedal",
                    "value": 0.5
                }),
            })
            .unwrap();
        let node =
            &session.project.as_ref().unwrap().tracks[0].graph.nodes[&NodeId::fixed("instrument")];
        assert_eq!(node.port_value("pedal"), Some(0.5));
        let _ = std::fs::remove_file(wav);
    }

    #[test]
    fn imports_wav_and_renders_sampler_track() {
        let wav = std::env::temp_dir()
            .join(format!("aurtrack-buffer-{}.wav", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let samples: Vec<i16> = (0..24_000)
            .map(|index| {
                let phase = std::f64::consts::TAU * 220.0 * index as f64 / 48_000.0;
                (phase.sin() * 16_000.0) as i16
            })
            .collect();
        write_wav(Path::new(&wav), 48_000, &samples).unwrap();
        let mut session = Session::default();
        session
            .execute(&CommandRequest {
                cmd: "project.create".to_string(),
                args: json!({ "name": "sampler" }),
            })
            .unwrap();
        let imported = session
            .execute(&CommandRequest {
                cmd: "buffer.import".to_string(),
                args: json!({ "path": wav }),
            })
            .unwrap();
        let buffer_id = imported.data["buffer_id"].as_str().unwrap().to_string();
        session
            .execute(&CommandRequest {
                cmd: "track.create".to_string(),
                args: json!({
                    "name": "sampler",
                    "source": "sampler",
                    "buffer_id": buffer_id
                }),
            })
            .unwrap();
        session
            .execute(&CommandRequest {
                cmd: "clip.create".to_string(),
                args: json!({ "track_id": "track-1", "start": 0.0, "duration": 1.0 }),
            })
            .unwrap();
        session
            .execute(&CommandRequest {
                cmd: "note.add".to_string(),
                args: json!({
                    "path": "/tracks/track-1/clips/clip-1",
                    "pitch": 69,
                    "start": 0.0,
                    "duration": 0.5,
                    "velocity": 0.8
                }),
            })
            .unwrap();
        let rendered = session
            .execute(&CommandRequest {
                cmd: "render.segment".to_string(),
                args: json!({ "duration_seconds": 0.5, "path": format!("{wav}.out.wav") }),
            })
            .unwrap();
        assert!(rendered.data["peak"].as_f64().unwrap() > 0.0);
        let output = format!("{wav}.out.wav");
        let _ = std::fs::remove_file(&wav);
        let _ = std::fs::remove_file(output);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod migration_tests {
    use super::*;

    fn add_legacy_pair(
        graph: &mut VoiceGraph,
        port: &str,
        input: &NodeId,
        output: &NodeId,
        header: &NodeId,
    ) {
        let socket = NodeGroupSocket::new(port, port, NodeGroupSocketType::Float);
        for id in [input, header] {
            if let Some(node) = graph.nodes.get_mut(id) {
                node.zone_inputs.push(socket.clone());
            }
        }
        for id in [output, header] {
            if let Some(node) = graph.nodes.get_mut(id) {
                node.zone_outputs.push(socket.clone());
            }
        }
        graph.connections.push(VoiceConnection::new(
            PortRef::new(NodeId::fixed("note-separate"), port),
            PortRef::new(input.clone(), port),
        ));
        graph.connections.push(VoiceConnection::new(
            PortRef::new(input.clone(), port),
            PortRef::new(NodeId::fixed("p-partial"), port),
        ));
        graph.connections.push(VoiceConnection::new(
            PortRef::new(input.clone(), port),
            PortRef::new(output.clone(), port),
        ));
    }

    #[test]
    fn migrates_legacy_piano_pairs_and_index() {
        let mut graph = VoiceGraph::piano_default();
        let input = NodeId::fixed("p-in");
        let output = NodeId::fixed("p-out");
        let header = NodeId::fixed("partials");
        graph.connections.retain(|connection| {
            !(connection.from.node() == &NodeId::fixed("note-separate")
                && matches!(connection.from.port(), "pitch" | "velocity")
                && connection.to.node() == &NodeId::fixed("p-partial"))
        });
        add_legacy_pair(&mut graph, "pitch", &input, &output, &header);
        add_legacy_pair(&mut graph, "velocity", &input, &output, &header);
        let index = VoiceNode::new(NodeId::fixed("p-index"), "voice.iteration");
        graph.nodes.insert(index.id.clone(), index);
        graph.connections.retain(|connection| {
            !(connection.from.node() == &input
                && connection.from.port() == "iteration"
                && connection.to.node() == &NodeId::fixed("p-partial"))
        });
        graph.connections.push(VoiceConnection::new(
            PortRef::new(NodeId::fixed("p-index"), "out"),
            PortRef::new(NodeId::fixed("p-partial"), "index"),
        ));
        let open = VoiceNode::new(NodeId::fixed("p-open-decay"), "value.float");
        let mut open = open;
        open.params.insert("value".to_string(), 2.5);
        graph.nodes.insert(open.id.clone(), open);
        graph.connections.push(VoiceConnection::new(
            PortRef::new(NodeId::fixed("p-open-decay"), "out"),
            PortRef::new(NodeId::fixed("p-partial"), "decay"),
        ));

        assert!(migrate_piano_voice_graph(&mut graph));
        assert_eq!(graph.nodes[&input].zone_inputs.len(), 1);
        for port in ["pitch", "velocity"] {
            assert!(graph.connections.iter().any(|connection| {
                connection.from.node() == &NodeId::fixed("note-separate")
                    && connection.from.port() == port
                    && connection.to.node() == &NodeId::fixed("p-partial")
                    && connection.to.port() == port
            }));
        }
        assert!(!graph.nodes.contains_key(&NodeId::fixed("p-index")));
        assert!(!graph.nodes.contains_key(&NodeId::fixed("p-open-decay")));
        assert!((graph.nodes[&NodeId::fixed("p-partial")].params["decay"] - 2.5).abs() < 1e-9);
        assert!(graph.connections.iter().any(|connection| {
            connection.from.node() == &input
                && connection.from.port() == "iteration"
                && connection.to.node() == &NodeId::fixed("p-partial")
                && connection.to.port() == "index"
        }));
        assert!(!migrate_piano_voice_graph(&mut graph));
    }

    #[test]
    fn group_param_set_writes_explicit_port_value() {
        let mut session = Session::default();
        session
            .execute(&CommandRequest {
                cmd: "project.create".to_string(),
                args: json!({ "name": "port-values" }),
            })
            .unwrap();
        session
            .execute(&CommandRequest {
                cmd: "track.create".to_string(),
                args: json!({ "name": "sine", "source": "sine" }),
            })
            .unwrap();
        session
            .execute(&CommandRequest {
                cmd: "param.set".to_string(),
                args: json!({
                    "path": "/tracks/track-1/nodes/instrument/params/frequency",
                    "value": 220.0
                }),
            })
            .unwrap();
        let node =
            &session.project.as_ref().unwrap().tracks[0].graph.nodes[&NodeId::fixed("instrument")];
        assert!(!node.params.contains_key("frequency"));
        assert_eq!(node.port_value("frequency"), Some(220.0));
    }

    #[test]
    fn project_open_drops_undeclared_group_frequency() {
        let path = std::env::temp_dir()
            .join(format!(
                "aurtrack-group-param-{}.aurtrack.json",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned();
        let mut session = Session::default();
        session
            .execute(&CommandRequest {
                cmd: "project.create".to_string(),
                args: json!({ "name": "params" }),
            })
            .unwrap();
        session
            .execute(&CommandRequest {
                cmd: "track.create".to_string(),
                args: json!({ "name": "piano", "source": "piano" }),
            })
            .unwrap();
        if let Some(track) = session
            .project
            .as_mut()
            .and_then(|project| project.tracks.first_mut())
        {
            if let Some(instrument) = track.graph.nodes.get_mut(&NodeId::fixed("instrument")) {
                instrument.params.insert("frequency".to_string(), 440.0);
            }
        }
        session
            .execute(&CommandRequest {
                cmd: "project.save".to_string(),
                args: json!({ "path": path }),
            })
            .unwrap();
        let mut opened = Session::default();
        let response = opened
            .execute(&CommandRequest {
                cmd: "project.open".to_string(),
                args: json!({ "path": path }),
            })
            .unwrap();
        assert_eq!(response.data["migrated_params"], 1);
        let project = opened.project.as_ref().unwrap();
        let instrument = project.tracks[0]
            .graph
            .nodes
            .get(&NodeId::fixed("instrument"))
            .unwrap();
        assert!(!instrument.params.contains_key("frequency"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn project_open_migrates_legacy_piano_graph() {
        let path = std::env::temp_dir()
            .join(format!(
                "aurtrack-migrate-open-{}.aurtrack.json",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned();
        let mut session = Session::default();
        session
            .execute(&CommandRequest {
                cmd: "project.create".to_string(),
                args: json!({ "name": "legacy" }),
            })
            .unwrap();
        {
            let project = session.project.as_mut().unwrap();
            let graph = &mut project.node_groups.get_mut("piano.voice").unwrap().graph;
            let input = NodeId::fixed("p-in");
            let output = NodeId::fixed("p-out");
            let header = NodeId::fixed("partials");
            graph.connections.retain(|connection| {
                !(connection.from.node() == &NodeId::fixed("note-separate")
                    && matches!(connection.from.port(), "pitch" | "velocity")
                    && connection.to.node() == &NodeId::fixed("p-partial"))
            });
            add_legacy_pair(graph, "pitch", &input, &output, &header);
            add_legacy_pair(graph, "velocity", &input, &output, &header);
            let index = VoiceNode::new(NodeId::fixed("p-index"), "voice.iteration");
            graph.nodes.insert(index.id.clone(), index);
            graph.connections.retain(|connection| {
                !(connection.from.node() == &input
                    && connection.from.port() == "iteration"
                    && connection.to.node() == &NodeId::fixed("p-partial"))
            });
            graph.connections.push(VoiceConnection::new(
                PortRef::new(NodeId::fixed("p-index"), "out"),
                PortRef::new(NodeId::fixed("p-partial"), "index"),
            ));
        }
        session
            .execute(&CommandRequest {
                cmd: "project.save".to_string(),
                args: json!({ "path": path }),
            })
            .unwrap();

        let mut opened = Session::default();
        let response = opened
            .execute(&CommandRequest {
                cmd: "project.open".to_string(),
                args: json!({ "path": path }),
            })
            .unwrap();
        assert_eq!(response.data["migrated_groups"], 1);
        let graph = &opened
            .project
            .as_ref()
            .unwrap()
            .node_groups
            .get("piano.voice")
            .unwrap()
            .graph;
        assert_eq!(graph.nodes[&NodeId::fixed("p-in")].zone_inputs.len(), 1);
        assert!(!graph.nodes.contains_key(&NodeId::fixed("p-index")));
        assert!(graph.connections.iter().any(|connection| {
            connection.from.node() == &NodeId::fixed("note-separate")
                && connection.from.port() == "pitch"
                && connection.to.node() == &NodeId::fixed("p-partial")
                && connection.to.port() == "pitch"
        }));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn repairs_missing_free_inputs() {
        let mut graph = VoiceGraph::piano_default();
        graph.connections.retain(|connection| {
            !(connection.from.node() == &NodeId::fixed("note-separate")
                && connection.to.node() == &NodeId::fixed("p-partial")
                && matches!(connection.to.port(), "pitch" | "velocity"))
        });
        assert!(migrate_piano_voice_graph(&mut graph));
        assert!(graph.connections.iter().any(|connection| {
            connection.from.node() == &NodeId::fixed("note-separate")
                && connection.from.port() == "pitch"
                && connection.to.node() == &NodeId::fixed("p-partial")
                && connection.to.port() == "pitch"
        }));
    }
}
