//! 职责：
//! - 维护实时播放计划与走带状态
//! - 在音频回调里按块渲染，不分配、不加锁
//!
//! 禁止：
//! - 依赖会话、IO 或 UI
//! - 在 `render` 里分配、加锁、日志

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

use arc_swap::ArcSwap;

/// 音符首尾的去咔嗒微包络长度。
pub const NOTE_FADE_SECONDS: f64 = 0.005;

const DECLICK_FADE_SECONDS: f64 = 0.003;
const NO_SEEK: u64 = u64::MAX;
const FADE_IDLE: u8 = 0;
const FADE_OUT: u8 = 1;
const FADE_IN: u8 = 2;

/// 一条轨道的播放参数。
#[derive(Debug, Clone)]
pub struct TrackPlan {
    /// `None` 表示静音轨道。
    pub frequency: Option<f64>,
    pub gain: f64,
    pub adsr: Option<AdsrPlan>,
    pub filters: Vec<FilterPlan>,
    pub graph: Option<GraphPlan>,
    pub notes: Vec<NotePlan>,
    pub plugin: Option<PluginPlan>,
    pub piano: Option<PianoPlan>,
}

/// 单个音符的 Voice 上下文。
#[derive(Debug, Clone, Copy)]
pub struct VoiceContext {
    pub pitch: f64,
    pub velocity: f64,
    pub local_time: f64,
    pub gate: bool,
    pub seed: u64,
    pub note_index: u64,
    pub iteration: u64,
}

/// 一次 Repeat 诊断采样的迭代携带值。
#[derive(Debug, Clone, Default)]
pub struct RepeatTrace {
    pub zone: String,
    pub iterations: Vec<Vec<f64>>,
}

#[derive(Debug, Default)]
struct TraceSink {
    repeats: Vec<RepeatTrace>,
}

/// Voice 子图编译结果。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct VoicePlan {
    pub nodes: Vec<VoiceNodePlan>,
    pub output: usize,
    pub outputs: BTreeMap<String, usize>,
    pub node_outputs: BTreeMap<String, usize>,
    last_remap: Vec<usize>,
    repeat_cache: Arc<[AtomicU64]>,
    repeat_stride: usize,
    scratch: Arc<[AtomicU64]>,
}

impl VoicePlan {
    pub fn new(nodes: Vec<VoiceNodePlan>, output: usize) -> Self {
        let repeat_stride = nodes
            .iter()
            .map(|node| match &node.op {
                VoiceOp::Repeat { outputs, .. } | VoiceOp::GroupCall { outputs, .. } => {
                    outputs.len()
                },
                _ => 0,
            })
            .max()
            .unwrap_or(1)
            .max(1);
        let repeat_slots = nodes.len().max(1).saturating_mul(repeat_stride);
        let scratch_slots = nodes.len().max(1).saturating_mul(VOICE_STATE_SLOTS);
        let last_remap = (0..nodes.len()).collect();
        Self {
            nodes,
            output,
            outputs: BTreeMap::new(),
            node_outputs: BTreeMap::new(),
            last_remap,
            repeat_cache: (0..repeat_slots)
                .map(|_| AtomicU64::new(0))
                .collect::<Vec<_>>()
                .into(),
            repeat_stride,
            scratch: (0..scratch_slots)
                .map(|_| AtomicU64::new(0))
                .collect::<Vec<_>>()
                .into(),
        }
    }

    fn scratch_value(&self, note_index: u64, index: usize) -> f64 {
        let base = (note_index as usize % VOICE_STATE_SLOTS) * self.nodes.len().max(1);
        self.scratch
            .get(base + index)
            .map_or(0.0, |value| f64::from_bits(value.load(Ordering::Relaxed)))
    }

    pub fn set_outputs(&mut self, outputs: BTreeMap<String, usize>) {
        self.outputs = outputs;
    }

    pub fn set_node_outputs(&mut self, node_outputs: BTreeMap<String, usize>) {
        self.node_outputs = node_outputs;
    }

    pub fn sample_output(&self, socket: &str, context: VoiceContext) -> Option<f64> {
        let target = *self.outputs.get(socket)?;
        self.sample_internal(context, &[], Some(target))
    }

    pub fn sample_node(&self, node_id: &str, context: VoiceContext) -> Option<f64> {
        let target = *self.node_outputs.get(node_id)?;
        self.sample_internal(context, &[], Some(target))
    }

    /// 编译期通用优化：嵌套子计划、常量折叠、代数化简、CSE 和死代码消除。
    #[must_use]
    pub fn optimize(mut self) -> Self {
        self.optimize_bodies();
        for _ in 0..2 {
            self.simplify_and_fold();
            self.common_subexpressions();
            self.remove_dead_nodes();
        }
        self
    }

    fn optimize_bodies(&mut self) {
        for node in &mut self.nodes {
            if let VoiceOp::Repeat { body, outputs, .. }
            | VoiceOp::GroupCall { body, outputs, .. } = &mut node.op
            {
                let old_outputs = outputs.clone();
                let placeholder = VoicePlan::new(Vec::new(), 0);
                let old = std::mem::replace(body.as_mut(), placeholder);
                *body = Box::new(old.optimize());
                for (slot, old_index) in old_outputs.into_iter().enumerate() {
                    if let Some(mapped) = body.last_remap.get(old_index).copied() {
                        outputs[slot] = mapped;
                    }
                }
            }
        }
    }

    pub fn fuse_group_calls(&mut self) {
        let mut replacements = Vec::new();
        for (index, node) in self.nodes.iter().enumerate() {
            let VoiceOp::GroupCall { body, outputs } = &node.op else {
                continue;
            };
            let mut fused = Vec::new();
            for (result_index, result) in self.nodes.iter().enumerate() {
                let VoiceOp::GroupResult { source, slot } = result.op else {
                    continue;
                };
                if source != index {
                    continue;
                }
                let Some(output) = outputs.get(slot).copied() else {
                    continue;
                };
                let Some(expr) = build_expression(body, output, 0) else {
                    fused.clear();
                    break;
                };
                let Some(compiled) = CompiledVoice::compile(&expr) else {
                    fused.clear();
                    break;
                };
                fused.push((result_index, compiled, node.inputs.clone()));
            }
            if !fused.is_empty() {
                replacements.extend(fused);
            }
        }
        for (index, expr, inputs) in replacements {
            self.nodes[index].op = VoiceOp::Expression(Arc::new(expr));
            self.nodes[index].inputs = inputs;
        }
    }

    fn simplify_and_fold(&mut self) {
        let mut values: Vec<Option<f64>> = vec![None; self.nodes.len()];
        for index in 0..self.nodes.len() {
            let inputs: Vec<Option<f64>> = self.nodes[index]
                .inputs
                .iter()
                .map(|input| values.get(*input).copied().flatten())
                .collect();
            if let VoiceOp::Add = self.nodes[index].op {
                let kept: Vec<usize> = self.nodes[index]
                    .inputs
                    .iter()
                    .zip(&inputs)
                    .filter_map(|(input, value)| (*value != Some(0.0)).then_some(*input))
                    .collect();
                if kept.len() != self.nodes[index].inputs.len() {
                    self.nodes[index].inputs = kept;
                }
            }
            if let VoiceOp::Multiply = self.nodes[index].op {
                if inputs.contains(&Some(0.0)) {
                    self.nodes[index].op = VoiceOp::Constant(0.0);
                    self.nodes[index].inputs.clear();
                    values[index] = Some(0.0);
                    continue;
                }
                let kept: Vec<usize> = self.nodes[index]
                    .inputs
                    .iter()
                    .zip(&inputs)
                    .filter_map(|(input, value)| (*value != Some(1.0)).then_some(*input))
                    .collect();
                if kept.len() != self.nodes[index].inputs.len() {
                    self.nodes[index].inputs = kept;
                }
            }
            if let VoiceOp::Subtract = self.nodes[index].op
                && inputs.get(1) == Some(&Some(0.0))
            {
                self.nodes[index].op = VoiceOp::Add;
                self.nodes[index].inputs.truncate(1);
            }
            if let VoiceOp::Divide = self.nodes[index].op
                && inputs.get(1) == Some(&Some(1.0))
            {
                self.nodes[index].op = VoiceOp::Add;
                self.nodes[index].inputs.truncate(1);
            }
            if let VoiceOp::Power = self.nodes[index].op
                && inputs.get(1) == Some(&Some(1.0))
            {
                self.nodes[index].op = VoiceOp::Add;
                self.nodes[index].inputs.truncate(1);
            }
            let all_constant =
                !self.nodes[index].inputs.is_empty() && inputs.iter().all(Option::is_some);
            let folded = if all_constant {
                let constants: Vec<f64> = inputs.iter().filter_map(|value| *value).collect();
                eval_constant(&self.nodes[index].op, &constants)
            } else {
                None
            };
            if let Some(value) = folded {
                self.nodes[index].op = VoiceOp::Constant(value);
                self.nodes[index].inputs.clear();
                values[index] = Some(value);
            } else if let VoiceOp::Constant(value) = self.nodes[index].op {
                values[index] = Some(value);
            }
        }
    }

    fn common_subexpressions(&mut self) {
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut replace: BTreeMap<usize, usize> = BTreeMap::new();
        let roots: Vec<usize> = self
            .outputs
            .values()
            .chain(self.node_outputs.values())
            .copied()
            .chain(std::iter::once(self.output))
            .collect();
        for index in 0..self.nodes.len() {
            if roots.contains(&index) {
                continue;
            }
            let Some(key) = pure_key(&self.nodes[index]) else {
                continue;
            };
            if let Some(canonical) = seen.get(&key) {
                replace.insert(index, *canonical);
            } else {
                seen.insert(key, index);
            }
        }
        if replace.is_empty() {
            return;
        }
        for node in &mut self.nodes {
            for input in &mut node.inputs {
                if let Some(canonical) = replace.get(input) {
                    *input = *canonical;
                }
            }
        }
    }

    fn remove_dead_nodes(&mut self) {
        let previous_remap = self.last_remap.clone();
        let mut live = vec![false; self.nodes.len()];
        let mut stack: Vec<usize> = self
            .outputs
            .values()
            .chain(self.node_outputs.values())
            .copied()
            .chain(std::iter::once(self.output))
            .collect();
        while let Some(index) = stack.pop() {
            let Some(node) = self.nodes.get(index) else {
                continue;
            };
            if live.get(index) == Some(&true) {
                continue;
            }
            if let Some(slot) = live.get_mut(index) {
                *slot = true;
            }
            stack.extend(node.inputs.iter().copied());
            if let VoiceOp::RepeatResult { source, .. } | VoiceOp::GroupResult { source, .. } =
                &node.op
            {
                stack.push(*source);
            }
        }
        let mut mapping = vec![usize::MAX; self.nodes.len()];
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for (index, node) in std::mem::take(&mut self.nodes).into_iter().enumerate() {
            if !live[index] {
                continue;
            }
            mapping[index] = nodes.len();
            nodes.push(node);
        }
        for node in &mut nodes {
            for input in &mut node.inputs {
                if let Some(mapped) = mapping.get(*input).copied()
                    && mapped != usize::MAX
                {
                    *input = mapped;
                }
            }
            if let VoiceOp::RepeatResult { source, .. } | VoiceOp::GroupResult { source, .. } =
                &mut node.op
            {
                let mapped = mapping.get(*source).copied().unwrap_or(usize::MAX);
                if mapped != usize::MAX {
                    *source = mapped;
                }
            }
        }
        let mut outputs = std::mem::take(&mut self.outputs);
        let mut node_outputs = std::mem::take(&mut self.node_outputs);
        let mut output = self.output;
        for index in outputs
            .values_mut()
            .chain(node_outputs.values_mut())
            .chain(std::iter::once(&mut output))
        {
            let mapped = mapping.get(*index).copied().unwrap_or(usize::MAX);
            if mapped != usize::MAX {
                *index = mapped;
            }
        }
        let mut composed = previous_remap;
        for value in &mut composed {
            if *value != usize::MAX {
                *value = mapping.get(*value).copied().unwrap_or(usize::MAX);
            }
        }
        *self = VoicePlan::new(nodes, output);
        self.outputs = outputs;
        self.node_outputs = node_outputs;
        self.last_remap = composed;
    }

    /// 离线诊断：记录每个 Repeat 每次迭代结束时的携带值。
    pub fn trace_repeats(&self, context: VoiceContext) -> Vec<RepeatTrace> {
        let mut sink = TraceSink::default();
        let _ = self.sample_values_traced(context, &[], Some(&mut sink));
        sink.repeats
    }
}

/// Voice 子图中的一个节点。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct VoiceNodePlan {
    pub op: VoiceOp,
    pub inputs: Vec<usize>,
    state: Arc<[AtomicU64; VOICE_STATE_SLOTS]>,
    buffer: Option<Arc<[AtomicU64]>>,
}

impl VoiceNodePlan {
    pub fn new(op: VoiceOp, inputs: Vec<usize>) -> Self {
        Self {
            op,
            inputs,
            state: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            buffer: None,
        }
    }

    /// 历史节点在编译期分配缓冲；音频线程只读写原子槽。
    pub fn with_buffer(op: VoiceOp, inputs: Vec<usize>, samples_per_note: usize) -> Self {
        let samples_per_note = samples_per_note.max(1);
        let buffer: Arc<[AtomicU64]> = (0..samples_per_note * VOICE_STATE_SLOTS)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>()
            .into();
        Self {
            op,
            inputs,
            state: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            buffer: Some(buffer),
        }
    }
}

/// Voice 子图节点操作。
/// 编译期融合的无状态表达式；音频线程只读，不求值分配。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum VoiceExpr {
    External(usize),
    Constant(f64),
    ContextPitch,
    ContextVelocity,
    ContextTime,
    ContextGate,
    Add(Box<VoiceExpr>, Box<VoiceExpr>),
    Subtract(Box<VoiceExpr>, Box<VoiceExpr>),
    Multiply(Box<VoiceExpr>, Box<VoiceExpr>),
    Divide(Box<VoiceExpr>, Box<VoiceExpr>),
    Power(Box<VoiceExpr>, Box<VoiceExpr>),
    Modulo(Box<VoiceExpr>, Box<VoiceExpr>),
    Floor(Box<VoiceExpr>),
    Ceil(Box<VoiceExpr>),
    Round(Box<VoiceExpr>),
    Exp(Box<VoiceExpr>),
    Sqrt(Box<VoiceExpr>),
    Sin(Box<VoiceExpr>),
    Abs(Box<VoiceExpr>),
    GreaterThan(Box<VoiceExpr>, Box<VoiceExpr>),
    LessThan(Box<VoiceExpr>, Box<VoiceExpr>),
    MapRange(
        Box<VoiceExpr>,
        Box<VoiceExpr>,
        Box<VoiceExpr>,
        Box<VoiceExpr>,
        Box<VoiceExpr>,
    ),
    Clamp(Box<VoiceExpr>, Box<VoiceExpr>, Box<VoiceExpr>),
    Sine {
        frequency: Box<VoiceExpr>,
        phase: Box<VoiceExpr>,
        time: Box<VoiceExpr>,
    },
}

#[allow(dead_code)]
impl VoiceExpr {
    fn eval(&self, context: &VoiceContext, source: &dyn Fn(usize) -> f64) -> f64 {
        match self {
            Self::External(slot) => source(*slot),
            Self::Constant(value) => *value,
            Self::ContextPitch => context.pitch,
            Self::ContextVelocity => context.velocity,
            Self::ContextTime => context.local_time,
            Self::ContextGate => f64::from(u8::from(context.gate)),
            Self::Add(a, b) => a.eval(context, source) + b.eval(context, source),
            Self::Subtract(a, b) => a.eval(context, source) - b.eval(context, source),
            Self::Multiply(a, b) => a.eval(context, source) * b.eval(context, source),
            Self::Divide(a, b) => {
                let divisor = b.eval(context, source);
                if divisor == 0.0 {
                    0.0
                } else {
                    a.eval(context, source) / divisor
                }
            },
            Self::Power(a, b) => a.eval(context, source).powf(b.eval(context, source)),
            Self::Modulo(a, b) => {
                let divisor = b.eval(context, source);
                if divisor == 0.0 {
                    0.0
                } else {
                    a.eval(context, source) % divisor
                }
            },
            Self::Floor(a) => a.eval(context, source).floor(),
            Self::Ceil(a) => a.eval(context, source).ceil(),
            Self::Round(a) => a.eval(context, source).round(),
            Self::Exp(a) => a.eval(context, source).exp(),
            Self::Sqrt(a) => a.eval(context, source).max(0.0).sqrt(),
            Self::Sin(a) => a.eval(context, source).sin(),
            Self::Abs(a) => a.eval(context, source).abs(),
            Self::GreaterThan(a, b) => {
                f64::from(u8::from(a.eval(context, source) > b.eval(context, source)))
            },
            Self::LessThan(a, b) => {
                f64::from(u8::from(a.eval(context, source) < b.eval(context, source)))
            },
            Self::MapRange(value_expr, in_min, in_max, out_min, out_max) => {
                let value = value_expr.eval(context, source);
                let in_min = in_min.eval(context, source);
                let in_max = in_max.eval(context, source);
                let out_min = out_min.eval(context, source);
                let out_max = out_max.eval(context, source);
                let span = in_max - in_min;
                if span == 0.0 {
                    out_min
                } else {
                    out_min + (value - in_min) / span * (out_max - out_min)
                }
            },
            Self::Clamp(value_expr, min, max) => value_expr
                .eval(context, source)
                .clamp(min.eval(context, source), max.eval(context, source)),
            Self::Sine {
                frequency,
                phase,
                time,
            } => (std::f64::consts::TAU
                * frequency.eval(context, source)
                * time.eval(context, source)
                + phase.eval(context, source))
            .sin(),
        }
    }
}

#[derive(Debug, Clone)]
enum CompiledOp {
    Constant(f64),
    External(usize),
    ContextPitch,
    ContextVelocity,
    ContextTime,
    ContextGate,
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
    Modulo,
    Floor,
    Ceil,
    Round,
    Exp,
    Sqrt,
    Sin,
    Abs,
    GreaterThan,
    LessThan,
    MapRange,
    Clamp,
    Sine,
}

#[derive(Debug, Clone)]
struct CompiledInstruction {
    op: CompiledOp,
    destination: usize,
    operands: [usize; 5],
}

/// 寄存器式表达式程序；音频线程在固定数组上执行。
#[derive(Clone)]
pub struct CompiledVoice {
    instructions: Vec<CompiledInstruction>,
    root: usize,
    jit: Option<std::sync::Arc<JitVoice>>,
}

struct JitVoice {
    _module: usize,
    entry: JitEntry,
}

type JitEntry = extern "C" fn(f64, f64, f64, f64, f64, f64, f64, *const f64) -> f64;

impl std::fmt::Debug for JitVoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JitVoice")
    }
}

impl std::fmt::Debug for CompiledVoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompiledVoice")
            .field("instructions", &self.instructions.len())
            .field("root", &self.root)
            .field("jit", &self.jit.is_some())
            .finish()
    }
}

impl CompiledVoice {
    fn compile(expr: &VoiceExpr) -> Option<Self> {
        let mut instructions = Vec::new();
        let mut registers = 0;
        let root = compile_expr(expr, &mut instructions, &mut registers)?;
        if registers > 256 {
            return None;
        }
        let jit = build_jit_voice(&instructions, root)
            .ok()
            .map(std::sync::Arc::new);
        Some(Self {
            instructions,
            root,
            jit,
        })
    }

    fn eval(&self, context: &VoiceContext, inputs: &[f64]) -> f64 {
        if let Some(jit) = &self.jit {
            return (jit.entry)(
                context.pitch,
                context.velocity,
                context.local_time,
                f64::from(u8::from(context.gate)),
                context.seed as f64,
                context.note_index as f64,
                context.iteration as f64,
                inputs.as_ptr(),
            );
        }
        let mut values = [0.0_f64; 256];
        for instruction in &self.instructions {
            let operands = instruction.operands;
            values[instruction.destination] = match instruction.op {
                CompiledOp::Constant(value) => value,
                CompiledOp::External(slot) => inputs.get(slot).copied().unwrap_or(0.0),
                CompiledOp::ContextPitch => context.pitch,
                CompiledOp::ContextVelocity => context.velocity,
                CompiledOp::ContextTime => context.local_time,
                CompiledOp::ContextGate => f64::from(u8::from(context.gate)),
                CompiledOp::Add => values[operands[0]] + values[operands[1]],
                CompiledOp::Subtract => values[operands[0]] - values[operands[1]],
                CompiledOp::Multiply => values[operands[0]] * values[operands[1]],
                CompiledOp::Divide => {
                    if values[operands[1]] == 0.0 {
                        0.0
                    } else {
                        values[operands[0]] / values[operands[1]]
                    }
                },
                CompiledOp::Power => values[operands[0]].powf(values[operands[1]]),
                CompiledOp::Modulo => {
                    if values[operands[1]] == 0.0 {
                        0.0
                    } else {
                        values[operands[0]] % values[operands[1]]
                    }
                },
                CompiledOp::Floor => values[operands[0]].floor(),
                CompiledOp::Ceil => values[operands[0]].ceil(),
                CompiledOp::Round => values[operands[0]].round(),
                CompiledOp::Exp => values[operands[0]].exp(),
                CompiledOp::Sqrt => values[operands[0]].max(0.0).sqrt(),
                CompiledOp::Sin => values[operands[0]].sin(),
                CompiledOp::Abs => values[operands[0]].abs(),
                CompiledOp::GreaterThan => {
                    f64::from(u8::from(values[operands[0]] > values[operands[1]]))
                },
                CompiledOp::LessThan => {
                    f64::from(u8::from(values[operands[0]] < values[operands[1]]))
                },
                CompiledOp::MapRange => {
                    let value = values[operands[0]];
                    let in_min = values[operands[1]];
                    let in_max = values[operands[2]];
                    let out_min = values[operands[3]];
                    let out_max = values[operands[4]];
                    let span = in_max - in_min;
                    if span.abs() < 1e-12 {
                        out_min
                    } else {
                        out_min + (value - in_min) / span * (out_max - out_min)
                    }
                },
                CompiledOp::Clamp => {
                    values[operands[0]].clamp(values[operands[1]], values[operands[2]])
                },
                CompiledOp::Sine => {
                    (std::f64::consts::TAU * values[operands[0]] * values[operands[1]]
                        + values[operands[2]])
                        .sin()
                },
            };
        }
        values[self.root]
    }
}

fn compile_expr(
    expr: &VoiceExpr,
    instructions: &mut Vec<CompiledInstruction>,
    registers: &mut usize,
) -> Option<usize> {
    macro_rules! emit {
        ($op:expr, $operands:expr) => {{
            let destination = *registers;
            *registers += 1;
            instructions.push(CompiledInstruction {
                op: $op,
                destination,
                operands: $operands,
            });
            destination
        }};
    }
    match expr {
        VoiceExpr::External(slot) => {
            let destination = *registers;
            *registers += 1;
            instructions.push(CompiledInstruction {
                op: CompiledOp::External(*slot),
                destination,
                operands: [0; 5],
            });
            Some(destination)
        },
        VoiceExpr::Constant(value) => {
            let destination = *registers;
            *registers += 1;
            instructions.push(CompiledInstruction {
                op: CompiledOp::Constant(*value),
                destination,
                operands: [0; 5],
            });
            Some(destination)
        },
        VoiceExpr::ContextPitch
        | VoiceExpr::ContextVelocity
        | VoiceExpr::ContextTime
        | VoiceExpr::ContextGate => {
            let op = match expr {
                VoiceExpr::ContextPitch => CompiledOp::ContextPitch,
                VoiceExpr::ContextVelocity => CompiledOp::ContextVelocity,
                VoiceExpr::ContextTime => CompiledOp::ContextTime,
                _ => CompiledOp::ContextGate,
            };
            Some(emit!(op, [0; 5]))
        },
        VoiceExpr::Add(a, b) => compile_binary(a, b, instructions, registers, CompiledOp::Add),
        VoiceExpr::Subtract(a, b) => {
            compile_binary(a, b, instructions, registers, CompiledOp::Subtract)
        },
        VoiceExpr::Multiply(a, b) => {
            compile_binary(a, b, instructions, registers, CompiledOp::Multiply)
        },
        VoiceExpr::Divide(a, b) => {
            compile_binary(a, b, instructions, registers, CompiledOp::Divide)
        },
        VoiceExpr::Power(a, b) => compile_binary(a, b, instructions, registers, CompiledOp::Power),
        VoiceExpr::Modulo(a, b) => {
            compile_binary(a, b, instructions, registers, CompiledOp::Modulo)
        },
        VoiceExpr::GreaterThan(a, b) => {
            compile_binary(a, b, instructions, registers, CompiledOp::GreaterThan)
        },
        VoiceExpr::LessThan(a, b) => {
            compile_binary(a, b, instructions, registers, CompiledOp::LessThan)
        },
        VoiceExpr::Floor(a)
        | VoiceExpr::Ceil(a)
        | VoiceExpr::Round(a)
        | VoiceExpr::Exp(a)
        | VoiceExpr::Sqrt(a)
        | VoiceExpr::Sin(a)
        | VoiceExpr::Abs(a) => {
            let source = compile_expr(a, instructions, registers)?;
            let op = match expr {
                VoiceExpr::Floor(_) => CompiledOp::Floor,
                VoiceExpr::Ceil(_) => CompiledOp::Ceil,
                VoiceExpr::Round(_) => CompiledOp::Round,
                VoiceExpr::Exp(_) => CompiledOp::Exp,
                VoiceExpr::Sqrt(_) => CompiledOp::Sqrt,
                VoiceExpr::Sin(_) => CompiledOp::Sin,
                _ => CompiledOp::Abs,
            };
            Some(emit!(op, [source, 0, 0, 0, 0]))
        },
        VoiceExpr::Clamp(value, min, max) => {
            let value = compile_expr(value, instructions, registers)?;
            let min = compile_expr(min, instructions, registers)?;
            let max = compile_expr(max, instructions, registers)?;
            Some(emit!(CompiledOp::Clamp, [value, min, max, 0, 0]))
        },
        VoiceExpr::MapRange(value, in_min, in_max, out_min, out_max) => {
            let value = compile_expr(value, instructions, registers)?;
            let in_min = compile_expr(in_min, instructions, registers)?;
            let in_max = compile_expr(in_max, instructions, registers)?;
            let out_min = compile_expr(out_min, instructions, registers)?;
            let out_max = compile_expr(out_max, instructions, registers)?;
            Some(emit!(
                CompiledOp::MapRange,
                [value, in_min, in_max, out_min, out_max]
            ))
        },
        VoiceExpr::Sine {
            frequency,
            phase,
            time,
        } => {
            let frequency = compile_expr(frequency, instructions, registers)?;
            let time = compile_expr(time, instructions, registers)?;
            let phase = compile_expr(phase, instructions, registers)?;
            Some(emit!(CompiledOp::Sine, [frequency, time, phase, 0, 0]))
        },
    }
}

#[unsafe(no_mangle)]
extern "C" fn aurtrack_jit_sin(value: f64) -> f64 {
    value.sin()
}

#[unsafe(no_mangle)]
extern "C" fn aurtrack_jit_exp(value: f64) -> f64 {
    value.exp()
}

#[unsafe(no_mangle)]
extern "C" fn aurtrack_jit_pow(base: f64, exponent: f64) -> f64 {
    base.powf(exponent)
}

#[unsafe(no_mangle)]
extern "C" fn aurtrack_jit_gt(a: f64, b: f64) -> f64 {
    f64::from(u8::from(a > b))
}

#[unsafe(no_mangle)]
extern "C" fn aurtrack_jit_lt(a: f64, b: f64) -> f64 {
    f64::from(u8::from(a < b))
}

#[unsafe(no_mangle)]
extern "C" fn aurtrack_jit_mod(a: f64, b: f64) -> f64 {
    if b == 0.0 { 0.0 } else { a % b }
}

#[unsafe(no_mangle)]
extern "C" fn aurtrack_jit_map(
    value: f64,
    in_min: f64,
    in_max: f64,
    out_min: f64,
    out_max: f64,
) -> f64 {
    let span = in_max - in_min;
    if span.abs() < 1e-12 {
        out_min
    } else {
        out_min + (value - in_min) / span * (out_max - out_min)
    }
}

#[allow(
    clippy::too_many_lines,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
fn build_jit_voice(instructions: &[CompiledInstruction], root: usize) -> Result<JitVoice, ()> {
    use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlags, UserFuncName, types};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_module::{Linkage, Module};

    let mut builder = cranelift_jit::JITBuilder::new(cranelift_module::default_libcall_names())
        .map_err(|_| ())?;
    builder.symbol("aurtrack_jit_sin", aurtrack_jit_sin as *const u8);
    builder.symbol("aurtrack_jit_exp", aurtrack_jit_exp as *const u8);
    builder.symbol("aurtrack_jit_pow", aurtrack_jit_pow as *const u8);
    builder.symbol("aurtrack_jit_mod", aurtrack_jit_mod as *const u8);
    builder.symbol("aurtrack_jit_gt", aurtrack_jit_gt as *const u8);
    builder.symbol("aurtrack_jit_lt", aurtrack_jit_lt as *const u8);
    builder.symbol("aurtrack_jit_map", aurtrack_jit_map as *const u8);
    let mut module = cranelift_jit::JITModule::new(builder);
    let pointer_type = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    for _ in 0..7 {
        signature.params.push(AbiParam::new(types::F64));
    }
    signature.params.push(AbiParam::new(pointer_type));
    signature.returns.push(AbiParam::new(types::F64));
    let mut context = module.make_context();
    context.func.signature = signature.clone();
    context.func.name = UserFuncName::user(0, 0);
    let mut function_context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut context.func, &mut function_context);
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    builder.seal_block(entry);
    let params = builder.block_params(entry).to_vec();
    let input_pointer = params[7];
    let mut variables = Vec::with_capacity(instructions.len());
    for _ in 0..instructions.len() {
        variables.push(builder.declare_var(types::F64));
    }
    let mut call_helper =
        |builder: &mut FunctionBuilder<'_>, name: &str, args: &[cranelift_codegen::ir::Value]| {
            let mut helper = module.make_signature();
            for _ in args {
                helper.params.push(AbiParam::new(types::F64));
            }
            helper.returns.push(AbiParam::new(types::F64));
            let id = module
                .declare_function(name, Linkage::Import, &helper)
                .expect("JIT helper");
            let reference = module.declare_func_in_func(id, builder.func);
            let call = builder.ins().call(reference, args);
            builder.inst_results(call)[0]
        };
    let mut canonical: Vec<usize> = (0..instructions.len()).collect();
    let mut common_values = std::collections::HashMap::new();
    for (index, instruction) in instructions.iter().enumerate() {
        let operands = instruction.operands;
        let canonical_operands: [usize; 5] = std::array::from_fn(|slot| canonical[operands[slot]]);
        let key = format!("{:?}:{canonical_operands:?}", instruction.op);
        if let Some((canonical_index, existing)) = common_values.get(&key).copied() {
            canonical[index] = canonical_index;
            builder.def_var(variables[index], existing);
            continue;
        }
        let mut value = |slot: usize| builder.use_var(variables[operands[slot]]);
        let value = match instruction.op {
            CompiledOp::Constant(value) => builder.ins().f64const(value),
            CompiledOp::External(slot) => builder.ins().load(
                types::F64,
                MemFlags::trusted(),
                input_pointer,
                (slot * 8) as i32,
            ),
            CompiledOp::ContextPitch => params[0],
            CompiledOp::ContextVelocity => params[1],
            CompiledOp::ContextTime => params[2],
            CompiledOp::ContextGate => params[3],
            CompiledOp::Add => {
                let a = value(0);
                let b = value(1);
                builder.ins().fadd(a, b)
            },
            CompiledOp::Subtract => {
                let a = value(0);
                let b = value(1);
                builder.ins().fsub(a, b)
            },
            CompiledOp::Multiply => {
                let a = value(0);
                let b = value(1);
                builder.ins().fmul(a, b)
            },
            CompiledOp::Divide => {
                let a = value(0);
                let b = value(1);
                let zero = builder.ins().f64const(0.0);
                let is_zero =
                    builder
                        .ins()
                        .fcmp(cranelift_codegen::ir::condcodes::FloatCC::Equal, b, zero);
                let zero_value = builder.ins().f64const(0.0);
                let quotient = builder.ins().fdiv(a, b);
                builder.ins().select(is_zero, zero_value, quotient)
            },
            CompiledOp::Power => {
                let a = value(0);
                let b = value(1);
                call_helper(&mut builder, "aurtrack_jit_pow", &[a, b])
            },
            CompiledOp::Modulo => {
                let a = value(0);
                let b = value(1);
                call_helper(&mut builder, "aurtrack_jit_mod", &[a, b])
            },
            CompiledOp::Floor => {
                let a = value(0);
                builder.ins().floor(a)
            },
            CompiledOp::Ceil => {
                let a = value(0);
                builder.ins().ceil(a)
            },
            CompiledOp::Round => {
                let a = value(0);
                builder.ins().nearest(a)
            },
            CompiledOp::Exp => {
                let a = value(0);
                call_helper(&mut builder, "aurtrack_jit_exp", &[a])
            },
            CompiledOp::Sqrt => {
                let a = value(0);
                builder.ins().sqrt(a)
            },
            CompiledOp::Sin => {
                let a = value(0);
                call_helper(&mut builder, "aurtrack_jit_sin", &[a])
            },
            CompiledOp::Abs => {
                let a = value(0);
                builder.ins().fabs(a)
            },
            CompiledOp::GreaterThan => {
                let a = value(0);
                let b = value(1);
                call_helper(&mut builder, "aurtrack_jit_gt", &[a, b])
            },
            CompiledOp::LessThan => {
                let a = value(0);
                let b = value(1);
                call_helper(&mut builder, "aurtrack_jit_lt", &[a, b])
            },
            CompiledOp::MapRange => {
                let values = [value(0), value(1), value(2), value(3), value(4)];
                call_helper(&mut builder, "aurtrack_jit_map", &values)
            },
            CompiledOp::Clamp => {
                let clamped_value = value(0);
                let min = value(1);
                let max = value(2);
                let clamped = builder.ins().fmax(clamped_value, min);
                builder.ins().fmin(clamped, max)
            },
            CompiledOp::Sine => {
                let frequency = value(0);
                let time = value(1);
                let phase = value(2);
                let tau = builder.ins().f64const(std::f64::consts::TAU);
                let angle = builder.ins().fmul(frequency, tau);
                let angle = builder.ins().fmul(angle, time);
                let angle = builder.ins().fadd(angle, phase);
                call_helper(&mut builder, "aurtrack_jit_sin", &[angle])
            },
        };
        canonical[index] = index;
        common_values.insert(key, (index, value));
        builder.def_var(variables[index], value);
    }
    let result = builder.use_var(variables[root]);
    builder.ins().return_(&[result]);
    builder.finalize();
    let id = module
        .declare_function("aurtrack_voice_expr", Linkage::Export, &signature)
        .map_err(|_| ())?;
    module.define_function(id, &mut context).map_err(|_| ())?;
    module.clear_context(&mut context);
    module.finalize_definitions().map_err(|_| ())?;
    let pointer = module.get_finalized_function(id);
    // SAFETY: 函数签名由上方 signature 定义，参数与 eval 调用一致。
    let entry: JitEntry = unsafe { std::mem::transmute(pointer) };
    // NOTE: JIT 代码页必须活到进程结束；模块只在此处泄漏一次。
    let module_address = Box::into_raw(Box::new(module)) as usize;
    Ok(JitVoice {
        _module: module_address,
        entry,
    })
}

fn compile_binary(
    a: &VoiceExpr,
    b: &VoiceExpr,
    instructions: &mut Vec<CompiledInstruction>,
    registers: &mut usize,
    op: CompiledOp,
) -> Option<usize> {
    let a = compile_expr(a, instructions, registers)?;
    let b = compile_expr(b, instructions, registers)?;
    let destination = *registers;
    *registers += 1;
    instructions.push(CompiledInstruction {
        op,
        destination,
        operands: [a, b, 0, 0, 0],
    });
    Some(destination)
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum VoiceOp {
    Pitch,
    Velocity,
    Gate,
    Time,
    NoteIndex,
    Iteration,
    Input(usize),
    WhiteNoise,
    Constant(f64),
    Sine,
    Sin,
    Abs,
    Hammer,
    Waveguide,
    Damper,
    BufferSample {
        samples: Arc<[f32]>,
        sample_rate: f64,
        root: f64,
        looped: bool,
    },
    Partial,
    PianoPartial,
    Attack,
    Decay,
    Adsr,
    Biquad {
        highpass: bool,
    },
    Delay,
    Repeat {
        zone: String,
        body: Box<VoicePlan>,
        outputs: Vec<usize>,
        carried: usize,
    },
    RepeatResult {
        source: usize,
        slot: usize,
    },
    GroupCall {
        body: Box<VoicePlan>,
        outputs: Vec<usize>,
    },
    GroupResult {
        source: usize,
        slot: usize,
    },
    Expression(Arc<CompiledVoice>),
    OscPartials {
        partials: usize,
        inharmonicity: f64,
        brightness: f64,
        decay: f64,
    },
    Envelope {
        attack: f64,
        decay: f64,
    },
    Noise,
    Vca,
    Mix,
    GateNode,
    Switch,
    IndexSwitch {
        slots: usize,
    },
    Broadcast,
    SampleHold,
    Add,
    Multiply,
    Subtract,
    Divide,
    Power,
    Modulo,
    Floor,
    Ceil,
    Round,
    Exp,
    Sqrt,
    IntToFloat,
    GreaterThan,
    LessThan,
    MapRange,
    Clamp,
    Output,
}

impl VoicePlan {
    pub fn sample(&self, context: VoiceContext) -> f64 {
        self.sample_with_inputs(context, &[])
    }

    pub fn sample_with_inputs(&self, context: VoiceContext, inputs: &[f64]) -> f64 {
        self.sample_internal(context, inputs, None).unwrap_or(0.0)
    }

    fn sample_values(&self, context: VoiceContext, inputs: &[f64]) -> Option<()> {
        self.sample_values_traced(context, inputs, None)
    }

    fn sample_values_traced(
        &self,
        context: VoiceContext,
        inputs: &[f64],
        mut trace: Option<&mut TraceSink>,
    ) -> Option<()> {
        let node_count = self.nodes.len();
        if node_count == 0 || self.output >= node_count {
            return None;
        }
        let base = (context.note_index as usize % VOICE_STATE_SLOTS) * node_count;
        let scratch_value = |index: usize| {
            self.scratch
                .get(base + index)
                .map_or(0.0, |value| f64::from_bits(value.load(Ordering::Relaxed)))
        };
        for (index, node) in self.nodes.iter().enumerate() {
            let value = match node.op {
                VoiceOp::Pitch => context.pitch,
                VoiceOp::Velocity => context.velocity,
                VoiceOp::Gate => f64::from(u8::from(context.gate)),
                VoiceOp::Time => context.local_time,
                VoiceOp::NoteIndex => context.note_index as f64,
                VoiceOp::Iteration => context.iteration as f64,
                VoiceOp::Input(index) => inputs.get(index).copied().unwrap_or(0.0),
                VoiceOp::WhiteNoise => {
                    let value = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    white_noise(value)
                },
                VoiceOp::Constant(value) => value,
                VoiceOp::Sine => {
                    let frequency = node
                        .inputs
                        .first()
                        .map_or(440.0, |input| scratch_value(*input))
                        .clamp(0.0, 20_000.0);
                    let phase = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    let time = node
                        .inputs
                        .get(2)
                        .map_or(0.0, |input| scratch_value(*input));
                    if !frequency.is_finite() || !phase.is_finite() || !time.is_finite() {
                        0.0
                    } else {
                        (std::f64::consts::TAU * frequency * time + phase).sin()
                    }
                },
                VoiceOp::OscPartials {
                    partials,
                    inharmonicity,
                    brightness,
                    decay,
                } => {
                    let pitch = node
                        .inputs
                        .first()
                        .map_or(context.pitch, |input| scratch_value(*input));
                    let velocity = node
                        .inputs
                        .get(1)
                        .map_or(context.velocity, |input| scratch_value(*input));
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let partials = node.inputs.get(2).map_or(partials, |input| {
                        (scratch_value(*input).round() as usize).clamp(1, 16)
                    });
                    let inharmonicity = node
                        .inputs
                        .get(3)
                        .map_or(inharmonicity, |input| scratch_value(*input));
                    let brightness = node
                        .inputs
                        .get(4)
                        .map_or(brightness, |input| scratch_value(*input));
                    let decay = node
                        .inputs
                        .get(5)
                        .map_or(decay, |input| scratch_value(*input));
                    osc_partials(
                        pitch,
                        velocity,
                        context.local_time,
                        partials,
                        inharmonicity,
                        brightness,
                        decay,
                    )
                },
                VoiceOp::Partial => {
                    let pitch = node
                        .inputs
                        .first()
                        .map_or(context.pitch, |input| scratch_value(*input));
                    let velocity = node
                        .inputs
                        .get(1)
                        .map_or(context.velocity, |input| scratch_value(*input));
                    let time = node
                        .inputs
                        .get(2)
                        .map_or(context.local_time, |input| scratch_value(*input));
                    let index = node
                        .inputs
                        .get(3)
                        .map_or(0.0, |input| scratch_value(*input));
                    let inharmonicity = node
                        .inputs
                        .get(4)
                        .map_or(0.0004, |input| scratch_value(*input));
                    let brightness = node
                        .inputs
                        .get(5)
                        .map_or(0.5, |input| scratch_value(*input));
                    let decay = node
                        .inputs
                        .get(6)
                        .map_or(1.2, |input| scratch_value(*input));
                    partial_sample(
                        pitch,
                        velocity,
                        time,
                        index,
                        inharmonicity,
                        brightness,
                        decay,
                    )
                },
                VoiceOp::PianoPartial => {
                    let pitch = node
                        .inputs
                        .first()
                        .map_or(context.pitch, |input| scratch_value(*input));
                    let velocity = node
                        .inputs
                        .get(1)
                        .map_or(context.velocity, |input| scratch_value(*input));
                    let time = node
                        .inputs
                        .get(2)
                        .map_or(context.local_time, |input| scratch_value(*input));
                    let index = node
                        .inputs
                        .get(3)
                        .map_or(0.0, |input| scratch_value(*input));
                    let inharmonicity = node
                        .inputs
                        .get(4)
                        .map_or(0.0004, |input| scratch_value(*input));
                    let brightness = node
                        .inputs
                        .get(5)
                        .map_or(0.5, |input| scratch_value(*input));
                    let decay = node
                        .inputs
                        .get(6)
                        .map_or(1.2, |input| scratch_value(*input));
                    let position = node
                        .inputs
                        .get(7)
                        .map_or(0.12, |input| scratch_value(*input));
                    piano_partial_sample(
                        pitch,
                        velocity,
                        time,
                        index,
                        inharmonicity,
                        brightness,
                        decay,
                        position,
                    )
                },
                VoiceOp::Hammer => {
                    let velocity = node
                        .inputs
                        .first()
                        .map_or(context.velocity, |input| scratch_value(*input))
                        .clamp(0.0, 1.0);
                    let hardness = node
                        .inputs
                        .get(1)
                        .map_or(0.5, |input| scratch_value(*input))
                        .clamp(0.0, 1.0);
                    let time = context.local_time.max(0.0);
                    let contact = 0.0012 + (1.0 - velocity) * 0.0025;
                    let envelope = (-time / contact).exp();
                    let noise = (hash64(context.seed ^ time.to_bits().rotate_left(17) ^ 0x51ed_2701)
                        as f64
                        / u64::MAX as f64)
                        * 2.0
                        - 1.0;
                    let thump = (std::f64::consts::TAU * 92.0 * time).sin();
                    (noise * (0.65 + hardness * 0.35) + thump * 0.2) * envelope * velocity * 0.55
                },
                VoiceOp::Waveguide => {
                    let Some(buffer) = &node.buffer else {
                        self.scratch[base + index].store(0.0_f64.to_bits(), Ordering::Relaxed);
                        continue;
                    };
                    let capacity = VOICE_DELAY_SAMPLES;
                    let stride = capacity + 4;
                    if capacity < 8 || buffer.len() < stride * node.state.len() {
                        self.scratch[base + index].store(0.0_f64.to_bits(), Ordering::Relaxed);
                        continue;
                    }
                    let slot = context.note_index as usize % node.state.len();
                    let excitation = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let frequency = node
                        .inputs
                        .get(1)
                        .map_or(context.pitch, |input| scratch_value(*input))
                        .clamp(8.0, 20_000.0);
                    let sample_rate = node
                        .inputs
                        .get(2)
                        .map_or(48_000.0, |input| scratch_value(*input))
                        .clamp(8_000.0, 192_000.0);
                    let damping = node
                        .inputs
                        .get(3)
                        .map_or(0.995, |input| scratch_value(*input))
                        .clamp(0.0, 0.9999);
                    let brightness = node
                        .inputs
                        .get(4)
                        .map_or(0.5, |input| scratch_value(*input))
                        .clamp(0.0, 1.0);
                    let stiffness = node
                        .inputs
                        .get(5)
                        .map_or(0.5, |input| scratch_value(*input))
                        .clamp(0.0, 1.0);
                    let delay = (sample_rate / frequency).clamp(2.0, (capacity - 2) as f64);
                    let write = ((context.local_time.max(0.0) * sample_rate) as usize) % capacity;
                    let read = (write as f64 - delay).rem_euclid(capacity as f64);
                    let read_index = read.floor() as usize;
                    let fraction = read - read_index as f64;
                    let base = slot * stride;
                    let a = f64::from_bits(buffer[base + read_index].load(Ordering::Relaxed));
                    let b = f64::from_bits(
                        buffer[base + (read_index + 1) % capacity].load(Ordering::Relaxed),
                    );
                    let x = a + (b - a) * fraction;
                    let state = node.state[slot].load(Ordering::Relaxed);
                    let stored = f64::from_bits(state);
                    let coefficient = 0.15 + brightness * 0.8;
                    let y = if stored.is_nan() {
                        x
                    } else {
                        stored + coefficient * (x - stored)
                    };
                    node.state[slot].store(y.to_bits(), Ordering::Relaxed);
                    let allpass = stiffness * 0.55;
                    let state_base = base + capacity;
                    let x_previous = f64::from_bits(buffer[state_base + 1].load(Ordering::Relaxed));
                    let y_previous = f64::from_bits(buffer[state_base + 2].load(Ordering::Relaxed));
                    let dispersed = -allpass * y + x_previous + allpass * y_previous;
                    buffer[state_base + 1].store(y.to_bits(), Ordering::Relaxed);
                    buffer[state_base + 2].store(dispersed.to_bits(), Ordering::Relaxed);
                    let feedback = (dispersed * damping).clamp(-1.0, 1.0);
                    buffer[base + write]
                        .store((excitation + feedback).to_bits(), Ordering::Relaxed);
                    x * 0.25 + dispersed * 0.75
                },
                VoiceOp::Damper => {
                    let input = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let gate = node
                        .inputs
                        .get(1)
                        .map_or(1.0, |input| scratch_value(*input))
                        > 0.5;
                    let pedal = node
                        .inputs
                        .get(2)
                        .map_or(0.0, |input| scratch_value(*input))
                        .clamp(0.0, 1.0);
                    let damping = node
                        .inputs
                        .get(3)
                        .map_or(0.01, |input| scratch_value(*input))
                        .clamp(0.0001, 0.2);
                    let slot = context.note_index as usize % node.state.len();
                    let stored = f64::from_bits(node.state[slot].load(Ordering::Relaxed));
                    let target = if gate { 1.0 } else { pedal };
                    let level = if stored.is_nan() {
                        target
                    } else if target > 0.001 {
                        stored + (target - stored) * 0.01
                    } else {
                        (stored - damping).max(0.0)
                    };
                    node.state[slot].store(level.to_bits(), Ordering::Relaxed);
                    input * level
                },
                VoiceOp::BufferSample {
                    ref samples,
                    sample_rate,
                    root,
                    looped,
                } => {
                    let gate = node
                        .inputs
                        .get(2)
                        .map_or(0.0, |input| scratch_value(*input));
                    let time = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let frequency = node
                        .inputs
                        .get(1)
                        .map_or(root, |input| scratch_value(*input));
                    if gate <= 0.5 || samples.is_empty() || !time.is_finite() {
                        0.0
                    } else {
                        let rate = frequency.max(0.0) / root.max(1e-9);
                        let mut position = time.max(0.0) * sample_rate * rate;
                        if looped {
                            position %= samples.len() as f64;
                        }
                        let index = position.floor();
                        if !index.is_finite() || index < 0.0 {
                            0.0
                        } else {
                            let index = index as usize;
                            if index >= samples.len() {
                                0.0
                            } else if index + 1 < samples.len() {
                                let fraction = position - index as f64;
                                let a = f64::from(samples[index]);
                                let b = f64::from(samples[index + 1]);
                                a + (b - a) * fraction
                            } else {
                                f64::from(samples[index])
                            }
                        }
                    }
                },
                VoiceOp::Adsr => {
                    let gate = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input))
                        > 0.5;
                    let time = node
                        .inputs
                        .get(1)
                        .map_or(context.local_time, |input| scratch_value(*input));
                    let attack = node
                        .inputs
                        .get(2)
                        .map_or(0.01, |input| scratch_value(*input));
                    let decay = node
                        .inputs
                        .get(3)
                        .map_or(0.1, |input| scratch_value(*input));
                    let sustain = node
                        .inputs
                        .get(4)
                        .map_or(0.7, |input| scratch_value(*input));
                    let release = node
                        .inputs
                        .get(5)
                        .map_or(0.2, |input| scratch_value(*input));
                    let slot = context.note_index as usize % node.state.len();
                    if gate {
                        node.state[slot].store(f64::NAN.to_bits(), Ordering::Relaxed);
                        adsr_held(time, attack, decay, sustain)
                    } else {
                        let stored = f64::from_bits(node.state[slot].load(Ordering::Relaxed));
                        let off = if stored.is_nan() {
                            node.state[slot].store(time.to_bits(), Ordering::Relaxed);
                            time
                        } else {
                            stored
                        };
                        let elapsed = (time - off).max(0.0);
                        let release = release.max(0.0001);
                        (adsr_held(off, attack, decay, sustain) * (1.0 - elapsed / release))
                            .max(0.0)
                    }
                },
                VoiceOp::Delay => {
                    let Some(buffer) = &node.buffer else {
                        self.scratch[base + index].store(0.0_f64.to_bits(), Ordering::Relaxed);
                        continue;
                    };
                    let per_note = buffer.len() / node.state.len();
                    if per_note == 0 {
                        self.scratch[base + index].store(0.0_f64.to_bits(), Ordering::Relaxed);
                        continue;
                    }
                    let input = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let time = node
                        .inputs
                        .get(1)
                        .map_or(0.01, |input| scratch_value(*input));
                    let feedback = node
                        .inputs
                        .get(2)
                        .map_or(0.0, |input| scratch_value(*input));
                    let rate = node
                        .inputs
                        .get(3)
                        .map_or(48_000.0, |input| scratch_value(*input));
                    let slot = context.note_index as usize % node.state.len();
                    let write = (node.state[slot].load(Ordering::Relaxed) as usize) % per_note;
                    let delay_samples =
                        (time * rate).round().clamp(1.0, (per_note - 1) as f64) as usize;
                    let read = (write + per_note - delay_samples) % per_note;
                    let base = slot * per_note;
                    let delayed = f64::from_bits(buffer[base + read].load(Ordering::Relaxed));
                    let next = input + delayed * feedback.clamp(0.0, 0.95);
                    buffer[base + write].store(next.to_bits(), Ordering::Relaxed);
                    node.state[slot].store(((write + 1) % per_note) as u64, Ordering::Relaxed);
                    delayed
                },
                VoiceOp::Biquad { highpass } => {
                    let input = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let cutoff = node
                        .inputs
                        .get(1)
                        .map_or(1200.0, |input| scratch_value(*input));
                    let q = node
                        .inputs
                        .get(2)
                        .map_or(0.707, |input| scratch_value(*input));
                    let rate = node
                        .inputs
                        .get(3)
                        .map_or(48_000.0, |input| scratch_value(*input));
                    if !input.is_finite()
                        || !cutoff.is_finite()
                        || !q.is_finite()
                        || !rate.is_finite()
                    {
                        0.0
                    } else {
                        let rate = rate.clamp(1_000.0, 192_000.0);
                        let cutoff = cutoff.clamp(20.0, rate * 0.45);
                        let q = q.clamp(0.1, 10.0);
                        let omega = std::f64::consts::TAU * cutoff / rate;
                        let alpha = omega.sin() / (2.0 * q);
                        let cosine = omega.cos();
                        let (b0, b1, b2) = if highpass {
                            let b0 = f64::midpoint(1.0, cosine);
                            (b0, -2.0 * b0, b0)
                        } else {
                            ((1.0 - cosine) / 2.0, 1.0 - cosine, (1.0 - cosine) / 2.0)
                        };
                        let a0 = 1.0 + alpha;
                        let a1 = -2.0 * cosine;
                        let a2 = 1.0 - alpha;
                        let b0 = b0 / a0;
                        let b1 = b1 / a0;
                        let b2 = b2 / a0;
                        let a1 = a1 / a0;
                        let a2 = a2 / a0;
                        let slot = (context.note_index as usize * 2) % (node.state.len() - 1);
                        let z1 = f64::from_bits(node.state[slot].load(Ordering::Relaxed));
                        let z2 = f64::from_bits(node.state[slot + 1].load(Ordering::Relaxed));
                        let output = b0 * input + z1;
                        let next_z1 = b1 * input - a1 * output + z2;
                        let next_z2 = b2 * input - a2 * output;
                        node.state[slot].store(next_z1.to_bits(), Ordering::Relaxed);
                        node.state[slot + 1].store(next_z2.to_bits(), Ordering::Relaxed);
                        if output.is_finite() { output } else { 0.0 }
                    }
                },
                VoiceOp::Attack => {
                    let time = node
                        .inputs
                        .first()
                        .map_or(context.local_time, |input| scratch_value(*input));
                    let attack = node
                        .inputs
                        .get(1)
                        .map_or(0.004, |input| scratch_value(*input));
                    (time / attack.max(0.0001)).min(1.0)
                },
                VoiceOp::Decay => {
                    let time = node
                        .inputs
                        .first()
                        .map_or(context.local_time, |input| scratch_value(*input));
                    let decay = node
                        .inputs
                        .get(1)
                        .map_or(1.2, |input| scratch_value(*input));
                    (-time / decay.max(0.01)).exp()
                },
                VoiceOp::Envelope { attack, decay } => {
                    let time = node
                        .inputs
                        .first()
                        .map_or(context.local_time, |input| scratch_value(*input));
                    let attack = node
                        .inputs
                        .get(1)
                        .map_or(attack, |input| scratch_value(*input));
                    let decay = node
                        .inputs
                        .get(2)
                        .map_or(decay, |input| scratch_value(*input));
                    let attack_gain = (time / attack.max(0.0001)).min(1.0);
                    attack_gain * (-time / decay.max(0.01)).exp()
                },
                VoiceOp::Noise => {
                    let time = node
                        .inputs
                        .first()
                        .map_or(context.local_time, |input| scratch_value(*input));
                    let seed = node
                        .inputs
                        .get(1)
                        .map_or(context.seed, |input| scratch_value(*input).to_bits());
                    noise_sample(seed, time)
                },
                VoiceOp::Vca => {
                    let input = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let gain = node
                        .inputs
                        .get(1)
                        .map_or(1.0, |input| scratch_value(*input));
                    input * gain
                },
                VoiceOp::Mix => {
                    let factor = node
                        .inputs
                        .first()
                        .map_or(0.5, |input| scratch_value(*input));
                    let a = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    let b = node
                        .inputs
                        .get(2)
                        .map_or(0.0, |input| scratch_value(*input));
                    a * (1.0 - factor) + b * factor
                },
                VoiceOp::Switch => {
                    let switch = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let index = if switch > 0.5 { 2 } else { 1 };
                    node.inputs
                        .get(index)
                        .map_or(0.0, |input| scratch_value(*input))
                },
                VoiceOp::IndexSwitch { slots } => {
                    let index = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let slot = (index.round().max(0.0) as usize).min(slots.saturating_sub(1)) + 1;
                    node.inputs
                        .get(slot)
                        .map_or(0.0, |input| scratch_value(*input))
                },
                VoiceOp::Broadcast => node
                    .inputs
                    .first()
                    .map_or(0.0, |input| scratch_value(*input)),
                VoiceOp::SampleHold => {
                    let next = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let voice = context.note_index as usize % node.state.len();
                    let previous = node.state[voice].swap(next.to_bits(), Ordering::Relaxed);
                    f64::from_bits(previous)
                },
                VoiceOp::GateNode => {
                    let signal = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let gate = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    signal * gate
                },
                VoiceOp::Add => node.inputs.iter().map(|input| scratch_value(*input)).sum(),
                VoiceOp::Multiply => node
                    .inputs
                    .iter()
                    .map(|input| scratch_value(*input))
                    .product::<f64>(),
                VoiceOp::Subtract => {
                    let a = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let b = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    a - b
                },
                VoiceOp::Divide => {
                    let a = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let b = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    if b.abs() < 1e-12 { 0.0 } else { a / b }
                },
                VoiceOp::Power => {
                    let a = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let b = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    let value = a.powf(b);
                    if value.is_finite() { value } else { 0.0 }
                },
                VoiceOp::Modulo => {
                    let a = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let b = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    if b.abs() < 1e-12 { 0.0 } else { a % b }
                },
                VoiceOp::Floor => node
                    .inputs
                    .first()
                    .map_or(0.0, |input| scratch_value(*input))
                    .floor(),
                VoiceOp::Ceil => node
                    .inputs
                    .first()
                    .map_or(0.0, |input| scratch_value(*input))
                    .ceil(),
                VoiceOp::Round => node
                    .inputs
                    .first()
                    .map_or(0.0, |input| scratch_value(*input))
                    .round(),
                VoiceOp::Exp => {
                    let value = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input))
                        .exp();
                    if value.is_finite() { value } else { 0.0 }
                },
                VoiceOp::Sin => {
                    let value = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    value.sin()
                },
                VoiceOp::Abs => {
                    let value = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    value.abs()
                },
                VoiceOp::Sqrt => {
                    let value = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    if value < 0.0 { 0.0 } else { value.sqrt() }
                },
                VoiceOp::IntToFloat => node
                    .inputs
                    .first()
                    .map_or(0.0, |input| scratch_value(*input)),
                VoiceOp::GreaterThan => {
                    let a = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let b = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    f64::from(u8::from(a > b))
                },
                VoiceOp::LessThan => {
                    let a = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let b = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    f64::from(u8::from(a < b))
                },
                VoiceOp::MapRange => {
                    let value = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let from_min = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    let from_max = node
                        .inputs
                        .get(2)
                        .map_or(1.0, |input| scratch_value(*input));
                    let to_min = node
                        .inputs
                        .get(3)
                        .map_or(0.0, |input| scratch_value(*input));
                    let to_max = node
                        .inputs
                        .get(4)
                        .map_or(1.0, |input| scratch_value(*input));
                    let span = from_max - from_min;
                    if span.abs() < 1e-12 {
                        to_min
                    } else {
                        to_min + (value - from_min) / span * (to_max - to_min)
                    }
                },
                VoiceOp::Clamp => {
                    let value = node
                        .inputs
                        .first()
                        .map_or(0.0, |input| scratch_value(*input));
                    let min = node
                        .inputs
                        .get(1)
                        .map_or(0.0, |input| scratch_value(*input));
                    let max = node
                        .inputs
                        .get(2)
                        .map_or(1.0, |input| scratch_value(*input));
                    value.clamp(min, max)
                },
                VoiceOp::GroupCall {
                    ref body,
                    ref outputs,
                } => {
                    if node.inputs.len() > 64 {
                        0.0
                    } else {
                        let mut args = [0.0_f64; 64];
                        for (slot, input) in args.iter_mut().zip(&node.inputs) {
                            *slot = scratch_value(*input);
                        }
                        if body
                            .sample_values_traced(
                                context,
                                &args[..node.inputs.len()],
                                trace.as_deref_mut(),
                            )
                            .is_none()
                        {
                            0.0
                        } else {
                            let base = index * self.repeat_stride;
                            for (slot, output) in outputs.iter().enumerate() {
                                let value = body.scratch_value(context.note_index, *output);
                                if let Some(target) = self.repeat_cache.get(base + slot) {
                                    target.store(value.to_bits(), Ordering::Relaxed);
                                }
                            }
                            outputs.first().map_or(0.0, |output| {
                                body.scratch_value(context.note_index, *output)
                            })
                        }
                    }
                },
                VoiceOp::GroupResult { source, slot } => {
                    let base = source * self.repeat_stride + slot;
                    self.repeat_cache
                        .get(base)
                        .map_or(0.0, |value| f64::from_bits(value.load(Ordering::Relaxed)))
                },
                VoiceOp::Expression(ref expr) => {
                    let mut local_inputs = [0.0_f64; 16];
                    let count = node.inputs.len().min(local_inputs.len());
                    for (slot, input) in node.inputs.iter().take(count).enumerate() {
                        local_inputs[slot] = scratch_value(*input);
                    }
                    expr.eval(&context, &local_inputs[..count])
                },
                VoiceOp::Repeat {
                    ref zone,
                    ref body,
                    ref outputs,
                    carried: carried_count,
                } => {
                    let carried_count = carried_count.min(outputs.len());
                    if outputs.len() > MAX_REPEAT_OUTPUTS {
                        0.0
                    } else {
                        let count = node
                            .inputs
                            .first()
                            .map_or(1.0, |input| scratch_value(*input))
                            .round()
                            .clamp(0.0, 64.0) as usize;
                        let mut carried = [0.0_f64; MAX_REPEAT_OUTPUTS];
                        for (slot, input) in carried[..carried_count]
                            .iter_mut()
                            .zip(node.inputs.iter().skip(1))
                        {
                            *slot = scratch_value(*input);
                        }
                        let mut free = [0.0_f64; 64];
                        for (slot, input) in free
                            .iter_mut()
                            .zip(node.inputs.iter().skip(1 + carried_count))
                        {
                            *slot = scratch_value(*input);
                        }
                        let mut body_inputs = [0.0_f64; 64];
                        body_inputs[..carried_count].copy_from_slice(&carried[..carried_count]);
                        let free_count =
                            node.inputs.len().saturating_sub(1 + carried_count).min(64);
                        body_inputs[carried_count..carried_count + free_count]
                            .copy_from_slice(&free[..free_count]);
                        let mut sampled = carried;
                        let mut iteration_values = Vec::new();
                        for iteration in 0..count {
                            let mut loop_context = context;
                            loop_context.iteration = iteration as u64;
                            body.sample_values_traced(
                                loop_context,
                                &body_inputs[..carried_count + free_count],
                                trace.as_deref_mut(),
                            )?;
                            for (slot, output) in outputs.iter().enumerate() {
                                sampled[slot] =
                                    body.scratch_value(loop_context.note_index, *output);
                            }
                            if carried_count > 0 {
                                body_inputs[..carried_count]
                                    .copy_from_slice(&sampled[..carried_count]);
                                carried[..carried_count].copy_from_slice(&sampled[..carried_count]);
                            }
                            if trace.is_some() {
                                iteration_values.push(carried[..outputs.len()].to_vec());
                            }
                        }
                        if let Some(sink) = trace.as_deref_mut() {
                            sink.repeats.push(RepeatTrace {
                                zone: zone.clone(),
                                iterations: iteration_values,
                            });
                        }
                        let base = index * self.repeat_stride;
                        for (slot, value) in carried[..outputs.len()].iter().enumerate() {
                            self.repeat_cache[base + slot]
                                .store(value.to_bits(), Ordering::Relaxed);
                        }
                        carried[0]
                    }
                },
                VoiceOp::RepeatResult { source, slot } => {
                    let base = source * self.repeat_stride + slot;
                    self.repeat_cache
                        .get(base)
                        .map_or(0.0, |value| f64::from_bits(value.load(Ordering::Relaxed)))
                },
                VoiceOp::Output => node
                    .inputs
                    .first()
                    .map_or(0.0, |input| scratch_value(*input)),
            };
            self.scratch[base + index].store(value.to_bits(), Ordering::Relaxed);
        }
        Some(())
    }

    fn sample_internal(
        &self,
        context: VoiceContext,
        inputs: &[f64],
        target: Option<usize>,
    ) -> Option<f64> {
        self.sample_values(context, inputs)?;
        let index = target.unwrap_or(self.output);
        Some(self.scratch_value(context.note_index, index))
    }

    pub fn sample_sockets(
        &self,
        sockets: &[usize],
        context: VoiceContext,
        inputs: &[f64],
    ) -> Option<Vec<f64>> {
        let mut output = vec![0.0; sockets.len()];
        self.sample_sockets_into(sockets, context, inputs, &mut output)?;
        Some(output)
    }

    /// 音频线程路径：把 socket 结果写进调用方缓冲区，避免分配。
    pub fn sample_sockets_into(
        &self,
        sockets: &[usize],
        context: VoiceContext,
        inputs: &[f64],
        output: &mut [f64],
    ) -> Option<()> {
        self.sample_values(context, inputs)?;
        for (slot, index) in output.iter_mut().zip(sockets) {
            *slot = self.scratch_value(context.note_index, *index);
        }
        Some(())
    }
}

fn hash64(mut state: u64) -> u64 {
    state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    state = (state ^ (state >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    state = (state ^ (state >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    state ^ (state >> 31)
}

fn white_noise(value: f64) -> f64 {
    (hash64(value.to_bits()) >> 11) as f64 / (1_u64 << 53) as f64
}

fn noise_sample(seed: u64, time: f64) -> f64 {
    let state = hash64(seed ^ time.to_bits().wrapping_mul(0x9E37_79B9_7F4A_7C15));
    (state >> 11) as f64 / (1_u64 << 53) as f64 * 2.0 - 1.0
}

fn adsr_held(time: f64, attack: f64, decay: f64, sustain: f64) -> f64 {
    if !time.is_finite() || time < 0.0 {
        return 0.0;
    }
    let sustain = sustain.clamp(0.0, 1.0);
    let attack = attack.max(0.0001);
    if time < attack {
        return time / attack;
    }
    let decay = decay.max(0.0);
    if time < attack + decay && decay > 0.0 {
        return 1.0 - (1.0 - sustain) * ((time - attack) / decay);
    }
    sustain
}

fn partial_sample(
    pitch: f64,
    velocity: f64,
    time: f64,
    index: f64,
    inharmonicity: f64,
    brightness: f64,
    decay: f64,
) -> f64 {
    let partial = index.max(0.0).round() as usize + 1;
    let frequency =
        pitch * partial as f64 * (1.0 + inharmonicity * (partial * partial) as f64).sqrt();
    if frequency >= 20_000.0 {
        return 0.0;
    }
    let envelope = (-time * (0.8 + partial as f64 * 0.35) / decay.max(0.05)).exp();
    let velocity = velocity.clamp(0.0, 1.0).powf(brightness + 0.5);
    (std::f64::consts::TAU * frequency * time).sin() * envelope * velocity * 0.18 / partial as f64
}

#[allow(clippy::too_many_arguments)]
fn piano_partial_sample(
    pitch: f64,
    velocity: f64,
    time: f64,
    index: f64,
    inharmonicity: f64,
    brightness: f64,
    decay: f64,
    position: f64,
) -> f64 {
    let partial = index.max(0.0).round() as usize + 1;
    let harmonic = partial as f64;
    let frequency =
        pitch * harmonic * (1.0 + inharmonicity.clamp(0.0, 0.01) * harmonic * harmonic).sqrt();
    if frequency >= 20_000.0 || time < 0.0 {
        return 0.0;
    }
    let brightness = brightness.clamp(0.0, 1.0);
    let velocity = velocity.clamp(0.0, 1.0).powf(0.55 + brightness * 0.7);
    let decay = decay.clamp(0.05, 8.0) / (1.0 + harmonic * 0.06);
    let prompt = (-time / 0.08).exp();
    let sustain = (-time / decay).exp();
    let envelope = (0.35 * prompt + 0.65 * sustain) * (1.0 - (-time / 0.0025).exp());
    let position_amplitude = (std::f64::consts::PI * harmonic * position.clamp(0.0, 1.0))
        .sin()
        .abs()
        .max(0.08);
    let amplitude = velocity * position_amplitude / harmonic.powf(0.55 + brightness * 0.7);
    let low = frequency * 0.99975;
    let high = frequency * 1.00025;
    let strings = ((std::f64::consts::TAU * frequency * time).sin()
        + (std::f64::consts::TAU * low * time).sin()
        + (std::f64::consts::TAU * high * time).sin())
        / 3.0;
    strings * envelope * amplitude * 0.18
}

fn osc_partials(
    pitch: f64,
    velocity: f64,
    time: f64,
    partials: usize,
    inharmonicity: f64,
    brightness: f64,
    decay: f64,
) -> f64 {
    let mut output = 0.0;
    let velocity = velocity.clamp(0.0, 1.0).powf(brightness + 0.5);
    for partial in 1..=partials {
        let frequency =
            pitch * partial as f64 * (1.0 + inharmonicity * (partial * partial) as f64).sqrt();
        if frequency >= 20_000.0 {
            break;
        }
        let envelope = (-time * (0.8 + partial as f64 * 0.35) / decay.max(0.05)).exp();
        output +=
            (std::f64::consts::TAU * frequency * time).sin() * envelope * velocity / partial as f64;
    }
    output * 0.18
}

/// 加法钢琴音色。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PianoPlan {
    pub partials: usize,
    pub decay: f64,
    pub brightness: f64,
    pub hammer: f64,
    pub voice: Option<VoicePlan>,
}

impl PianoPlan {
    pub fn new(partials: usize, decay: f64, brightness: f64, hammer: f64) -> Self {
        Self {
            partials,
            decay,
            brightness,
            hammer,
            voice: None,
        }
    }

    pub fn sample(&self, notes: &[NotePlan], time: f64) -> f64 {
        let mut output = 0.0;
        for (index, note) in notes.iter().enumerate() {
            let local = time - note.start;
            if local < 0.0 || local > note.duration + 0.5 {
                continue;
            }
            if let Some(voice) = &self.voice {
                output += voice.sample(VoiceContext {
                    pitch: note.frequency,
                    velocity: note.velocity,
                    local_time: local,
                    gate: local <= note.duration,
                    seed: note.start.to_bits() ^ note.frequency.to_bits() ^ note.velocity.to_bits(),
                    note_index: index as u64,
                    iteration: 0,
                });
                continue;
            }
            let attack = (local / 0.004).min(1.0);
            let brightness = note.velocity.clamp(0.0, 1.0).powf(self.brightness + 0.5);
            for partial in 1..=self.partials {
                let frequency = note.frequency * partial as f64;
                if frequency >= 20_000.0 {
                    break;
                }
                let envelope =
                    (-local * (0.8 + partial as f64 * 0.35) / self.decay.max(0.05)).exp();
                let amplitude = brightness / partial as f64;
                output += (std::f64::consts::TAU * frequency * local).sin()
                    * envelope
                    * amplitude
                    * attack;
            }
            if local < 0.012 {
                let seed =
                    note.start.to_bits() ^ note.frequency.to_bits() ^ note.velocity.to_bits();
                let noise = ((seed.wrapping_mul(6_364_136_223_846_793_005) >> 40) as f64
                    / 16_777_216.0)
                    * 2.0
                    - 1.0;
                output += noise * (-local * 180.0).exp() * self.hammer * note.velocity;
            }
        }
        output * 0.18
    }
}

/// 轨道插件引用；实际处理器由宿主线程持有。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPlan {
    pub path: String,
    pub id: String,
    pub state: Option<String>,
}

impl PluginPlan {
    pub fn new(path: String, id: String, state: Option<String>) -> Self {
        Self { path, id, state }
    }
}

/// 宿主音频处理器的内部契约。
///
/// 插件格式由 `aurtrack-host` 适配，引擎只认这个块处理接口。实现必须保证
/// `process` 不在音频线程分配、加锁、日志或做系统调用。
pub trait BlockProcessor: Send {
    fn prepare(&mut self, sample_rate: u32, max_block_frames: usize) -> Result<(), String>;
    fn reset(&mut self);
    fn process(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) -> Result<(), String>;
}

/// 低通滤波器参数。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterSettings {
    pub cutoff: f64,
    pub resonance: f64,
}

impl FilterSettings {
    pub fn new(cutoff: f64, resonance: f64) -> Self {
        Self { cutoff, resonance }
    }
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self::new(1200.0, 0.2)
    }
}

/// 离线渲染使用的滤波器状态。
#[derive(Debug, Clone, Copy, Default)]
pub struct FilterState {
    z1: f64,
    z2: f64,
}

impl FilterState {
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

impl FilterSettings {
    /// 单采样 TPT 低通；共振 0 时接近一阶，接近 1 时更尖。
    pub fn process(&self, sample_rate: u32, state: &mut FilterState, input: f64) -> f64 {
        let nyquist_limit = f64::from(sample_rate) * 0.45;
        let cutoff = self.cutoff.clamp(20.0, nyquist_limit.max(20.0));
        let resonance = self.resonance.clamp(0.0, 0.95);
        let g = (std::f64::consts::PI * cutoff / f64::from(sample_rate)).tan();
        let k = (2.0 - 2.0 * resonance).max(0.05);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = input - state.z2;
        let v1 = a1 * state.z1 + a2 * v3;
        let v2 = state.z2 + a2 * state.z1 + a3 * v3;
        state.z1 = 2.0 * v1 - state.z1;
        state.z2 = 2.0 * v2 - state.z2;
        v2
    }
}

#[derive(Debug, Default)]
struct FilterStateAtomics {
    z1: AtomicU64,
    z2: AtomicU64,
}

/// 实时计划里的一个有状态滤波器。
#[derive(Debug, Clone)]
pub struct FilterPlan {
    pub settings: FilterSettings,
    state: Arc<FilterStateAtomics>,
}

impl FilterPlan {
    pub fn new(settings: FilterSettings) -> Self {
        Self {
            settings,
            state: Arc::new(FilterStateAtomics::default()),
        }
    }

    pub fn process(&self, sample_rate: u32, input: f64) -> f64 {
        self.process_with(
            sample_rate,
            input,
            self.settings.cutoff,
            self.settings.resonance,
        )
    }

    pub fn process_with(&self, sample_rate: u32, input: f64, cutoff: f64, resonance: f64) -> f64 {
        let mut state = FilterState {
            z1: f64::from_bits(self.state.z1.load(Ordering::Relaxed)),
            z2: f64::from_bits(self.state.z2.load(Ordering::Relaxed)),
        };
        let settings = FilterSettings::new(cutoff, resonance);
        let output = settings.process(sample_rate, &mut state, input);
        self.state.z1.store(state.z1.to_bits(), Ordering::Relaxed);
        self.state.z2.store(state.z2.to_bits(), Ordering::Relaxed);
        output
    }

    pub fn reset(&self) {
        self.state.z1.store(0.0_f64.to_bits(), Ordering::Relaxed);
        self.state.z2.store(0.0_f64.to_bits(), Ordering::Relaxed);
    }
}

/// 一阶高通，使用原子状态以便实时线程无锁。
#[derive(Debug, Clone)]
pub struct HighpassPlan {
    pub cutoff: f64,
    pub resonance: f64,
    state: Arc<AtomicU64>,
}

impl HighpassPlan {
    pub fn new(cutoff: f64, resonance: f64) -> Self {
        Self {
            cutoff: cutoff.clamp(20.0, 20_000.0),
            resonance: resonance.clamp(0.0, 0.95),
            state: Arc::new(AtomicU64::new(0.0_f64.to_bits())),
        }
    }

    pub fn process(&self, sample_rate: u32, input: f64) -> f64 {
        self.process_with(sample_rate, input, self.cutoff, self.resonance)
    }

    pub fn process_with(&self, sample_rate: u32, input: f64, cutoff: f64, resonance: f64) -> f64 {
        let rate = f64::from(sample_rate.max(1));
        let cutoff = cutoff.clamp(20.0, 20_000.0);
        let alpha = (1.0 - (-std::f64::consts::TAU * cutoff / rate).exp()).clamp(0.0, 1.0);
        let low = f64::from_bits(self.state.load(Ordering::Relaxed));
        let low = low + alpha * (input - low);
        self.state.store(low.to_bits(), Ordering::Relaxed);
        (input - low) * (1.0 + resonance.clamp(0.0, 0.95))
    }

    pub fn reset(&self) {
        self.state.store(0.0_f64.to_bits(), Ordering::Relaxed);
    }
}

/// 无锁延迟线；缓冲区在构建计划时分配，实时线程只读写原子槽。
#[derive(Debug, Clone)]
pub struct DelayPlan {
    pub time: f64,
    pub feedback: f64,
    buffer: Arc<Vec<AtomicU64>>,
    write: Arc<AtomicUsize>,
}

impl DelayPlan {
    pub fn new(time: f64, feedback: f64) -> Self {
        Self {
            time: time.clamp(0.001, 2.0),
            feedback: feedback.clamp(0.0, 0.95),
            buffer: Arc::new((0..DELAY_SLOTS).map(|_| AtomicU64::new(0)).collect()),
            write: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn process(&self, sample_rate: u32, input: f64) -> f64 {
        self.process_with(sample_rate, input, self.time, self.feedback)
    }

    pub fn process_with(&self, sample_rate: u32, input: f64, time: f64, feedback: f64) -> f64 {
        let rate = f64::from(sample_rate.max(1));
        let delay = (time.clamp(0.001, 2.0) * rate).round() as usize;
        let delay = delay.clamp(1, DELAY_SLOTS - 1);
        let write = self.write.load(Ordering::Relaxed) % DELAY_SLOTS;
        let read = (write + DELAY_SLOTS - delay) % DELAY_SLOTS;
        let delayed = f64::from_bits(self.buffer[read].load(Ordering::Relaxed));
        self.buffer[write].store(
            (input + delayed * feedback.clamp(0.0, 0.95)).to_bits(),
            Ordering::Relaxed,
        );
        self.write
            .store((write + 1) % DELAY_SLOTS, Ordering::Relaxed);
        delayed
    }

    pub fn reset(&self) {
        for slot in self.buffer.iter() {
            slot.store(0.0_f64.to_bits(), Ordering::Relaxed);
        }
        self.write.store(0, Ordering::Relaxed);
    }
}

/// 单个音频域模态谐振器，状态固定，音频线程无分配。
#[derive(Debug, Clone)]
pub struct ResonatorPlan {
    state: Arc<[AtomicU64; 4]>,
    frequency: f64,
    q: f64,
    gain: f64,
}

impl ResonatorPlan {
    pub fn new(frequency: f64, q: f64, gain: f64) -> Self {
        Self {
            state: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            frequency: frequency.clamp(20.0, 20_000.0),
            q: q.clamp(0.1, 40.0),
            gain: gain.clamp(0.0, 4.0),
        }
    }

    pub fn process(&self, sample_rate: u32, input: f64) -> f64 {
        let rate = f64::from(sample_rate.max(1));
        let w0 = std::f64::consts::TAU * self.frequency / rate;
        let alpha = w0.sin() / (2.0 * self.q);
        let a0 = 1.0 + alpha;
        let b0 = alpha / a0;
        let b2 = -alpha / a0;
        let a1 = -2.0 * w0.cos() / a0;
        let a2 = (1.0 - alpha) / a0;
        let x1 = f64::from_bits(self.state[0].load(Ordering::Relaxed));
        let x2 = f64::from_bits(self.state[1].load(Ordering::Relaxed));
        let y1 = f64::from_bits(self.state[2].load(Ordering::Relaxed));
        let y2 = f64::from_bits(self.state[3].load(Ordering::Relaxed));
        let y = b0 * input + b2 * x2 - a1 * y1 - a2 * y2;
        self.state[0].store(input.to_bits(), Ordering::Relaxed);
        self.state[1].store(x1.to_bits(), Ordering::Relaxed);
        self.state[2].store(y.to_bits(), Ordering::Relaxed);
        self.state[3].store(y1.to_bits(), Ordering::Relaxed);
        y * self.gain
    }

    pub fn reset(&self) {
        for slot in self.state.iter() {
            slot.store(0.0_f64.to_bits(), Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Clone)]
struct SoundboardMode {
    state: Arc<[AtomicU64; 4]>,
    frequency: f64,
    q: f64,
    gain: f64,
}

/// 音频域共享音板：所有声部先混合，再进入一组固定模态。
#[derive(Debug, Clone)]
pub struct SoundboardPlan {
    modes: Vec<SoundboardMode>,
}

impl SoundboardPlan {
    pub fn new() -> Self {
        let modes = [
            (110.0, 4.0, 1.2),
            (190.0, 5.0, 1.0),
            (320.0, 6.0, 0.9),
            (540.0, 7.0, 0.8),
            (860.0, 8.0, 0.7),
            (1_280.0, 7.0, 0.6),
            (1_900.0, 5.0, 0.5),
            (2_800.0, 4.0, 0.4),
        ];
        Self {
            modes: modes
                .iter()
                .map(|(frequency, q, gain)| SoundboardMode {
                    state: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
                    frequency: *frequency,
                    q: *q,
                    gain: *gain,
                })
                .collect(),
        }
    }

    pub fn process(&self, sample_rate: u32, input: f64) -> f64 {
        let rate = f64::from(sample_rate.max(1));
        let mut wet = 0.0;
        for mode in &self.modes {
            let w0 = std::f64::consts::TAU * mode.frequency / rate;
            let alpha = w0.sin() / (2.0 * mode.q);
            let a0 = 1.0 + alpha;
            let b0 = alpha / a0;
            let b2 = -alpha / a0;
            let a1 = -2.0 * w0.cos() / a0;
            let a2 = (1.0 - alpha) / a0;
            let x1 = f64::from_bits(mode.state[0].load(Ordering::Relaxed));
            let x2 = f64::from_bits(mode.state[1].load(Ordering::Relaxed));
            let y1 = f64::from_bits(mode.state[2].load(Ordering::Relaxed));
            let y2 = f64::from_bits(mode.state[3].load(Ordering::Relaxed));
            let y = b0 * input + b2 * x2 - a1 * y1 - a2 * y2;
            mode.state[0].store(input.to_bits(), Ordering::Relaxed);
            mode.state[1].store(x1.to_bits(), Ordering::Relaxed);
            mode.state[2].store(y.to_bits(), Ordering::Relaxed);
            mode.state[3].store(y1.to_bits(), Ordering::Relaxed);
            wet += y * mode.gain;
        }
        input * 0.72 + wet * 0.065
    }

    pub fn reset(&self) {
        for mode in &self.modes {
            for slot in mode.state.iter() {
                slot.store(0.0_f64.to_bits(), Ordering::Relaxed);
            }
        }
    }
}

impl Default for SoundboardPlan {
    fn default() -> Self {
        Self::new()
    }
}

/// 低频振荡器，输出 Float 控制值。
#[derive(Debug, Clone, Copy)]
pub struct LfoPlan {
    pub rate: f64,
    pub depth: f64,
    pub offset: f64,
}

impl LfoPlan {
    pub fn new(rate: f64, depth: f64, offset: f64) -> Self {
        Self {
            rate: rate.clamp(0.01, 40.0),
            depth: depth.clamp(0.0, 10.0),
            offset: offset.clamp(-10.0, 10.0),
        }
    }

    pub fn at(&self, time: f64) -> f64 {
        self.offset + (std::f64::consts::TAU * self.rate * time).sin() * self.depth
    }
}

/// DAG 计划最多允许的节点数。
pub const MAX_GRAPH_NODES: usize = 256;

/// 单个 Voice 子图的节点计划上限。
pub const MAX_VOICE_NODES: usize = 256;

/// 一个音频图里同时混合的活乐器上限。
pub const MAX_VOICES: usize = 8;

/// 每个 Voice 节点的状态槽数；Biquad 每音符占两槽。
pub const VOICE_STATE_SLOTS: usize = 128;

/// 一个 Repeat Zone 最多携带的输出值数量。
pub const MAX_REPEAT_OUTPUTS: usize = 64;

/// 每个音符可用的 Voice 延迟采样数。
pub const VOICE_DELAY_SAMPLES: usize = 4096;

const DELAY_SLOTS: usize = 384_000;

/// DAG 中的一个节点。
#[derive(Debug, Clone)]
pub struct GraphNodePlan {
    pub op: GraphOp,
    pub inputs: Vec<usize>,
    /// Float 控制输入的节点索引，顺序与注册表端口顺序一致。
    pub controls: Vec<usize>,
}

/// DAG 节点操作。
#[derive(Debug, Clone)]
pub enum GraphOp {
    Voice { index: usize },
    Sine,
    Constant(f64),
    Lfo(LfoPlan),
    Add,
    Multiply,
    MapRange,
    Clamp,
    Filter(FilterPlan),
    Highpass(HighpassPlan),
    Delay(DelayPlan),
    Soundboard(SoundboardPlan),
    Resonator(ResonatorPlan),
    Gain,
    Drive,
    Mix,
    Plugin,
    Output,
}

/// 已排序的 DAG 计划；索引 0 固定是声部。
#[derive(Debug, Clone)]
pub struct GraphPlan {
    pub nodes: Vec<GraphNodePlan>,
    pub output: usize,
    pub plugin: Option<PluginPlan>,
    pub pianos: Vec<PianoPlan>,
    pub fallback_sine: Option<f64>,
}

fn control_value(
    node: &GraphNodePlan,
    index: usize,
    values: &[f64; MAX_GRAPH_NODES],
    default: f64,
) -> f64 {
    node.controls
        .get(index)
        .map_or(default, |control| values[*control])
}

impl GraphPlan {
    pub fn sample(&self, voices: &[f64], sample_rate: u32, time: f64) -> f64 {
        if self.nodes.len() > MAX_GRAPH_NODES || self.output >= self.nodes.len() {
            return 0.0;
        }
        let mut values = [0.0_f64; MAX_GRAPH_NODES];
        for (index, node) in self.nodes.iter().enumerate() {
            let value = match &node.op {
                GraphOp::Voice { index } => voices.get(*index).copied().unwrap_or(0.0),
                GraphOp::Sine => {
                    let frequency = control_value(node, 0, &values, 440.0).clamp(20.0, 20_000.0);
                    let phase = control_value(node, 1, &values, 0.0);
                    (std::f64::consts::TAU * frequency * time + phase).sin() * 0.25
                },
                GraphOp::Constant(value) => *value,
                GraphOp::Lfo(lfo) => lfo.at(time),
                GraphOp::Add => node.controls.iter().map(|control| values[*control]).sum(),
                GraphOp::Multiply => node
                    .controls
                    .iter()
                    .map(|control| values[*control])
                    .product(),
                GraphOp::MapRange => {
                    let value = control_value(node, 0, &values, 0.0);
                    let from_min = control_value(node, 1, &values, 0.0);
                    let from_max = control_value(node, 2, &values, 1.0);
                    let to_min = control_value(node, 3, &values, 0.0);
                    let to_max = control_value(node, 4, &values, 1.0);
                    let span = from_max - from_min;
                    if span.abs() < 1e-12 {
                        to_min
                    } else {
                        to_min + (value - from_min) / span * (to_max - to_min)
                    }
                },
                GraphOp::Clamp => {
                    let value = control_value(node, 0, &values, 0.0);
                    value.clamp(
                        control_value(node, 1, &values, 0.0),
                        control_value(node, 2, &values, 1.0),
                    )
                },
                GraphOp::Filter(filter) => {
                    let value = node.inputs.first().map_or(0.0, |input| values[*input]);
                    filter.process_with(
                        sample_rate,
                        value,
                        control_value(node, 0, &values, filter.settings.cutoff),
                        control_value(node, 1, &values, filter.settings.resonance),
                    )
                },
                GraphOp::Highpass(filter) => {
                    let value = node.inputs.first().map_or(0.0, |input| values[*input]);
                    filter.process_with(
                        sample_rate,
                        value,
                        control_value(node, 0, &values, filter.cutoff),
                        control_value(node, 1, &values, filter.resonance),
                    )
                },
                GraphOp::Soundboard(soundboard) => {
                    let value = node.inputs.first().map_or(0.0, |input| values[*input]);
                    soundboard.process(sample_rate, value)
                },
                GraphOp::Resonator(resonator) => {
                    let value = node.inputs.first().map_or(0.0, |input| values[*input]);
                    resonator.process(sample_rate, value)
                },
                GraphOp::Delay(delay) => {
                    let value = node.inputs.first().map_or(0.0, |input| values[*input]);
                    delay.process_with(
                        sample_rate,
                        value,
                        control_value(node, 0, &values, delay.time),
                        control_value(node, 1, &values, delay.feedback),
                    )
                },
                GraphOp::Gain => {
                    let input = node.inputs.first().map_or(0.0, |input| values[*input]);
                    input * control_value(node, 0, &values, 1.0)
                },
                GraphOp::Drive => {
                    let input = node.inputs.first().map_or(0.0, |input| values[*input]);
                    (input * control_value(node, 0, &values, 1.0).max(1.0)).tanh()
                },
                GraphOp::Mix => node.inputs.iter().map(|input| values[*input]).sum(),
                GraphOp::Plugin => node.inputs.first().map_or(0.0, |input| values[*input]),
                GraphOp::Output => node.inputs.first().map_or(0.0, |input| values[*input]),
            };
            values[index] = value;
        }
        values[self.output]
    }
}

/// 每个音符可用的 ADSR 包络参数，时间单位秒。
#[derive(Debug, Clone, Copy)]
pub struct AdsrPlan {
    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
}

impl Default for AdsrPlan {
    fn default() -> Self {
        Self {
            attack: DEFAULT_ATTACK,
            decay: DEFAULT_DECAY,
            sustain: DEFAULT_SUSTAIN,
            release: DEFAULT_RELEASE,
        }
    }
}

pub const DEFAULT_ATTACK: f64 = 0.01;
pub const DEFAULT_DECAY: f64 = 0.12;
pub const DEFAULT_SUSTAIN: f64 = 0.75;
pub const DEFAULT_RELEASE: f64 = 0.18;
const MIN_ENVELOPE_SECONDS: f64 = 0.001;

/// 一个已排入时间线的音符。
#[derive(Debug, Clone)]
pub struct NotePlan {
    pub start: f64,
    pub duration: f64,
    pub frequency: f64,
    pub velocity: f64,
}

/// MIDI 音高转频率，A4 = 69 = 440 Hz。
pub fn midi_to_hz(pitch: u8) -> f64 {
    440.0 * 2_f64.powf((f64::from(pitch) - 69.0) / 12.0)
}

/// 把无状态子计划构建成融合表达式；失败时保留原计划。
#[allow(dead_code)]
fn build_expression(plan: &VoicePlan, index: usize, depth: usize) -> Option<VoiceExpr> {
    if depth > 64 {
        return None;
    }
    let node = plan.nodes.get(index)?;
    let input = |slot: usize| -> Option<VoiceExpr> {
        let child = *node.inputs.get(slot)?;
        build_expression(plan, child, depth + 1)
    };
    let binary = |f: fn(Box<VoiceExpr>, Box<VoiceExpr>) -> VoiceExpr| -> Option<VoiceExpr> {
        Some(f(Box::new(input(0)?), Box::new(input(1)?)))
    };
    let fold = |f: fn(Box<VoiceExpr>, Box<VoiceExpr>) -> VoiceExpr| -> Option<VoiceExpr> {
        let mut iter = node.inputs.iter();
        let first = iter.next()?;
        let mut result = build_expression(plan, *first, depth + 1)?;
        for child in iter {
            result = f(
                Box::new(result),
                Box::new(build_expression(plan, *child, depth + 1)?),
            );
        }
        Some(result)
    };
    match &node.op {
        VoiceOp::Output | VoiceOp::IntToFloat => input(0),
        VoiceOp::Vca => Some(VoiceExpr::Multiply(
            Box::new(input(0)?),
            Box::new(input(1).unwrap_or(VoiceExpr::Constant(1.0))),
        )),
        VoiceOp::Input(slot) => Some(VoiceExpr::External(*slot)),
        VoiceOp::Constant(value) => Some(VoiceExpr::Constant(*value)),
        VoiceOp::Pitch => Some(VoiceExpr::ContextPitch),
        VoiceOp::Velocity => Some(VoiceExpr::ContextVelocity),
        VoiceOp::Time => Some(VoiceExpr::ContextTime),
        VoiceOp::Gate => Some(VoiceExpr::ContextGate),
        VoiceOp::Add => fold(VoiceExpr::Add),
        VoiceOp::Multiply => fold(VoiceExpr::Multiply),
        VoiceOp::Subtract => binary(VoiceExpr::Subtract),
        VoiceOp::Divide => binary(VoiceExpr::Divide),
        VoiceOp::Power => binary(VoiceExpr::Power),
        VoiceOp::Modulo => binary(VoiceExpr::Modulo),
        VoiceOp::GreaterThan => binary(VoiceExpr::GreaterThan),
        VoiceOp::LessThan => binary(VoiceExpr::LessThan),
        VoiceOp::Floor => Some(VoiceExpr::Floor(Box::new(input(0)?))),
        VoiceOp::Ceil => Some(VoiceExpr::Ceil(Box::new(input(0)?))),
        VoiceOp::Round => Some(VoiceExpr::Round(Box::new(input(0)?))),
        VoiceOp::Exp => Some(VoiceExpr::Exp(Box::new(input(0)?))),
        VoiceOp::Sqrt => Some(VoiceExpr::Sqrt(Box::new(input(0)?))),
        VoiceOp::Sin => Some(VoiceExpr::Sin(Box::new(input(0)?))),
        VoiceOp::Abs => Some(VoiceExpr::Abs(Box::new(input(0)?))),
        VoiceOp::Clamp => Some(VoiceExpr::Clamp(
            Box::new(input(0)?),
            Box::new(input(1)?),
            Box::new(input(2)?),
        )),
        VoiceOp::MapRange => Some(VoiceExpr::MapRange(
            Box::new(input(0)?),
            Box::new(input(1)?),
            Box::new(input(2)?),
            Box::new(input(3)?),
            Box::new(input(4)?),
        )),
        VoiceOp::Sine => Some(VoiceExpr::Sine {
            frequency: Box::new(input(0).unwrap_or(VoiceExpr::Constant(440.0))),
            phase: Box::new(input(1).unwrap_or(VoiceExpr::Constant(0.0))),
            time: Box::new(input(2).unwrap_or(VoiceExpr::ContextTime)),
        }),
        _ => None,
    }
}

fn eval_constant(op: &VoiceOp, args: &[f64]) -> Option<f64> {
    let at = |index: usize| args.get(index).copied();
    match op {
        VoiceOp::Add => Some(args.iter().sum()),
        VoiceOp::Multiply => Some(args.iter().product()),
        VoiceOp::Subtract => Some(at(0)? - at(1)?),
        VoiceOp::Divide => {
            let divisor = at(1)?;
            (divisor != 0.0).then(|| at(0).unwrap_or(0.0) / divisor)
        },
        VoiceOp::Power => Some(at(0)?.powf(at(1)?)),
        VoiceOp::Modulo => {
            let divisor = at(1)?;
            (divisor != 0.0).then(|| at(0).unwrap_or(0.0) % divisor)
        },
        VoiceOp::Floor => Some(at(0)?.floor()),
        VoiceOp::Ceil => Some(at(0)?.ceil()),
        VoiceOp::Round => Some(at(0)?.round()),
        VoiceOp::Exp => Some(at(0)?.exp()),
        VoiceOp::Sqrt => Some(at(0)?.max(0.0).sqrt()),
        VoiceOp::IntToFloat => Some(at(0)?.round()),
        VoiceOp::GreaterThan => Some(f64::from(u8::from(at(0)? > at(1)?))),
        VoiceOp::LessThan => Some(f64::from(u8::from(at(0)? < at(1)?))),
        VoiceOp::MapRange => {
            let value = at(0)?;
            let in_min = at(1)?;
            let in_max = at(2)?;
            let out_min = at(3)?;
            let out_max = at(4)?;
            let span = in_max - in_min;
            if span == 0.0 {
                Some(out_min)
            } else {
                Some(out_min + (value - in_min) / span * (out_max - out_min))
            }
        },
        VoiceOp::Clamp => Some(at(0)?.clamp(at(1)?, at(2)?)),
        VoiceOp::Sin => Some(at(0)?.sin()),
        VoiceOp::Abs => Some(at(0)?.abs()),
        _ => None,
    }
}

fn pure_key(node: &VoiceNodePlan) -> Option<String> {
    match &node.op {
        VoiceOp::Add
        | VoiceOp::Multiply
        | VoiceOp::Subtract
        | VoiceOp::Divide
        | VoiceOp::Power
        | VoiceOp::Modulo
        | VoiceOp::Floor
        | VoiceOp::Ceil
        | VoiceOp::Round
        | VoiceOp::Exp
        | VoiceOp::Sqrt
        | VoiceOp::IntToFloat
        | VoiceOp::GreaterThan
        | VoiceOp::LessThan
        | VoiceOp::MapRange
        | VoiceOp::Clamp
        | VoiceOp::Sin
        | VoiceOp::Abs
        | VoiceOp::Constant(_) => Some(format!("{:?}{:?}", node.op, node.inputs)),
        _ => None,
    }
}

fn note_envelope(local: f64, duration: f64) -> f64 {
    let attack = (local / NOTE_FADE_SECONDS).clamp(0.0, 1.0);
    let release = ((duration - local) / NOTE_FADE_SECONDS).clamp(0.0, 1.0);
    attack.min(release)
}

/// ADSR：gate 从 0 开始，`duration` 是按键时长，release 从 gate 结束处开始。
pub fn adsr_envelope(gate: f64, duration: f64, adsr: AdsrPlan) -> f64 {
    if !gate.is_finite() || gate < 0.0 {
        return 0.0;
    }
    let attack = adsr.attack.max(MIN_ENVELOPE_SECONDS);
    let decay = adsr.decay.max(0.0);
    let sustain = adsr.sustain.clamp(0.0, 1.0);
    let release = adsr.release.max(MIN_ENVELOPE_SECONDS);
    let peak_at_gate_off = if duration < attack {
        duration / attack
    } else if duration < attack + decay {
        1.0 - (1.0 - sustain) * ((duration - attack) / decay)
    } else {
        sustain
    };
    if gate < duration {
        if gate < attack {
            gate / attack
        } else if gate < attack + decay {
            1.0 - (1.0 - sustain) * ((gate - attack) / decay)
        } else {
            sustain
        }
    } else if gate < duration + release {
        peak_at_gate_off * (1.0 - (gate - duration) / release)
    } else {
        0.0
    }
}

/// 一次播放使用的不可变计划。
#[derive(Debug, Clone)]
pub struct PlaybackPlan {
    pub sample_rate: u32,
    pub loop_start_seconds: f64,
    pub loop_seconds: f64,
    pub length_seconds: f64,
    pub tracks: Vec<TrackPlan>,
}

impl PlaybackPlan {
    pub fn silent(sample_rate: u32, loop_seconds: f64) -> Self {
        Self {
            sample_rate,
            loop_start_seconds: 0.0,
            loop_seconds,
            length_seconds: loop_seconds,
            tracks: Vec::new(),
        }
    }
}

/// 实时走带状态。音频回调只调用 `render`。
#[derive(Debug)]
pub struct Playback {
    plan: ArcSwap<PlaybackPlan>,
    playing: AtomicBool,
    stop_requested: AtomicBool,
    position: AtomicU64,
    master_gain: AtomicU64,
    transport_gain: AtomicU64,
    seek_target: AtomicU64,
    fade_target: AtomicU64,
    seek_stage: AtomicU8,
    fade_elapsed: AtomicU64,
}

impl Playback {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            plan: ArcSwap::from_pointee(PlaybackPlan::silent(sample_rate, 5.0)),
            playing: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            position: AtomicU64::new(0),
            master_gain: AtomicU64::new(1.0_f64.to_bits()),
            transport_gain: AtomicU64::new(0.0_f64.to_bits()),
            seek_target: AtomicU64::new(NO_SEEK),
            fade_target: AtomicU64::new(NO_SEEK),
            seek_stage: AtomicU8::new(FADE_IDLE),
            fade_elapsed: AtomicU64::new(0),
        }
    }

    pub fn set_plan(&self, plan: PlaybackPlan) {
        self.plan.store(Arc::new(plan));
    }

    pub fn play(&self) {
        let was_stopping = self.stop_requested.swap(false, Ordering::AcqRel);
        let was_playing = self.playing.swap(true, Ordering::AcqRel);
        if !was_playing || was_stopping {
            self.transport_gain
                .store(0.0_f64.to_bits(), Ordering::Release);
            self.reset_filters();
        }
    }

    fn reset_filters(&self) {
        let plan = self.plan.load();
        for track in &plan.tracks {
            for filter in &track.filters {
                filter.reset();
            }
            if let Some(graph) = &track.graph {
                for node in &graph.nodes {
                    if let GraphOp::Filter(filter) = &node.op {
                        filter.reset();
                    }
                    if let GraphOp::Soundboard(soundboard) = &node.op {
                        soundboard.reset();
                    }
                    if let GraphOp::Resonator(resonator) = &node.op {
                        resonator.reset();
                    }
                }
            }
        }
    }

    pub fn stop(&self) {
        if self.playing.load(Ordering::Acquire) {
            self.stop_requested.store(true, Ordering::Release);
        }
        self.seek_target.store(NO_SEEK, Ordering::Release);
        self.fade_target.store(NO_SEEK, Ordering::Release);
        self.seek_stage.store(FADE_IDLE, Ordering::Release);
        self.fade_elapsed.store(0, Ordering::Release);
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire) && !self.stop_requested.load(Ordering::Acquire)
    }

    pub fn seek(&self, seconds: f64) {
        let plan = self.plan.load();
        let length_seconds = plan.length_seconds.max(plan.loop_seconds).max(0.001);
        let wrapped = seconds.rem_euclid(length_seconds);
        let frames = (wrapped * f64::from(plan.sample_rate)).round() as u64;
        self.reset_filters();
        if !self.playing.load(Ordering::Acquire) {
            self.position.store(frames, Ordering::Release);
            self.seek_target.store(NO_SEEK, Ordering::Release);
            self.fade_target.store(NO_SEEK, Ordering::Release);
            self.seek_stage.store(FADE_IDLE, Ordering::Release);
            self.fade_elapsed.store(0, Ordering::Release);
            return;
        }
        self.seek_target.store(frames, Ordering::Release);
        if self.seek_stage.load(Ordering::Acquire) == FADE_IDLE {
            self.fade_elapsed.store(0, Ordering::Release);
            self.seek_stage.store(FADE_OUT, Ordering::Release);
        }
    }

    pub fn position_seconds(&self) -> f64 {
        let plan = self.plan.load();
        if plan.sample_rate == 0 {
            return 0.0;
        }
        let target = self.seek_target.load(Ordering::Acquire);
        if self.seek_stage.load(Ordering::Acquire) != FADE_IDLE && target != NO_SEEK {
            return target as f64 / f64::from(plan.sample_rate);
        }
        self.position.load(Ordering::Acquire) as f64 / f64::from(plan.sample_rate)
    }

    pub fn set_master_gain(&self, gain: f64) {
        if gain.is_finite() {
            self.master_gain
                .store(gain.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// 由音频回调调用。只做算术，不分配、不加锁。
    pub fn render(&self, out: &mut [f32]) {
        if !self.playing.load(Ordering::Acquire) {
            out.fill(0.0);
            return;
        }

        let plan = self.plan.load();
        let sample_rate = f64::from(plan.sample_rate).max(1.0);
        let loop_start_frames = (plan.loop_start_seconds * sample_rate).round() as u64;
        let loop_end_frames = (plan.loop_seconds * sample_rate)
            .round()
            .max(loop_start_frames.saturating_add(1) as f64) as u64;
        let fade_frames = (DECLICK_FADE_SECONDS * sample_rate).round().max(1.0) as u64;
        let loop_fade = if loop_end_frames.saturating_sub(loop_start_frames) > fade_frames * 2 {
            fade_frames
        } else {
            0
        };
        let loop_advance_start = loop_start_frames + loop_fade;
        let master_gain = f64::from_bits(self.master_gain.load(Ordering::Relaxed));
        let stop_requested = self.stop_requested.load(Ordering::Acquire);
        let target_gain = if stop_requested { 0.0 } else { 1.0 };
        let gain_step = 1.0 / fade_frames as f64;
        let mut gain = f64::from_bits(self.transport_gain.load(Ordering::Relaxed));
        let mut position = self.position.load(Ordering::Relaxed);
        let mut stage = self.seek_stage.load(Ordering::Acquire);
        let mut target = if stage == FADE_IN {
            self.fade_target.load(Ordering::Acquire)
        } else {
            self.seek_target.load(Ordering::Acquire)
        };
        let mut elapsed = self.fade_elapsed.load(Ordering::Relaxed);

        if stop_requested && gain <= 0.0 {
            self.playing.store(false, Ordering::Release);
            self.stop_requested.store(false, Ordering::Release);
            out.fill(0.0);
            return;
        }

        if stage == FADE_IDLE && target != NO_SEEK {
            stage = FADE_OUT;
            elapsed = 0;
        } else if stage == FADE_OUT && target == NO_SEEK {
            stage = FADE_IDLE;
            elapsed = 0;
        }

        for (index, slot) in out.iter_mut().enumerate() {
            let seek_gain = if stage == FADE_OUT {
                Some(1.0 - (elapsed + 1) as f64 / fade_frames as f64)
            } else if stage == FADE_IN {
                Some((elapsed + 1) as f64 / fade_frames as f64)
            } else {
                None
            };
            let sample = Self::sample_with_loop(
                &plan,
                position,
                loop_start_frames,
                loop_end_frames,
                loop_fade,
                master_gain,
            );
            *slot = (sample * seek_gain.unwrap_or(1.0) * gain) as f32;

            if seek_gain.is_some() {
                elapsed += 1;
                if elapsed >= fade_frames {
                    if stage == FADE_OUT {
                        target = self.seek_target.load(Ordering::Acquire);
                        if target == NO_SEEK {
                            stage = FADE_IDLE;
                            elapsed = 0;
                        } else {
                            position = target;
                            self.fade_target.store(target, Ordering::Release);
                            stage = FADE_IN;
                            elapsed = 0;
                        }
                    } else {
                        stage = FADE_IDLE;
                        elapsed = 0;
                        let latest = self.seek_target.load(Ordering::Acquire);
                        if latest != NO_SEEK && latest != target {
                            target = latest;
                            stage = FADE_OUT;
                        } else {
                            target = NO_SEEK;
                            self.seek_target.store(NO_SEEK, Ordering::Release);
                            self.fade_target.store(NO_SEEK, Ordering::Release);
                        }
                    }
                }
            }

            if stop_requested {
                gain = (gain - gain_step).max(0.0);
                if gain <= 0.0 {
                    self.playing.store(false, Ordering::Release);
                    self.stop_requested.store(false, Ordering::Release);
                    out[index + 1..].fill(0.0);
                    break;
                }
            } else if gain < target_gain {
                gain = (gain + gain_step).min(target_gain);
            }
            position = advance(position, loop_end_frames, loop_advance_start);
        }

        self.position.store(position, Ordering::Release);
        self.transport_gain.store(gain.to_bits(), Ordering::Release);
        if stage != FADE_IDLE {
            self.seek_stage.store(stage, Ordering::Release);
            self.fade_elapsed.store(elapsed, Ordering::Release);
        } else if self.seek_stage.load(Ordering::Acquire) == FADE_IDLE {
            self.fade_elapsed.store(0, Ordering::Release);
        }
    }

    /// 分轨块渲染：输出按轨道优先排列，`stems[track * frames + frame]`。
    ///
    /// 音频回调在每轨插件处理后再混音。轨道数超过计划时，多出的槽保持静音。
    pub fn render_stems(&self, stems: &mut [f32], frames: usize, track_count: usize) {
        let needed = frames.saturating_mul(track_count);
        if needed == 0 || stems.len() < needed {
            return;
        }
        if !self.playing.load(Ordering::Acquire) {
            stems[..needed].fill(0.0);
            return;
        }
        let plan = self.plan.load();
        let sample_rate = f64::from(plan.sample_rate).max(1.0);
        let loop_start_frames = (plan.loop_start_seconds * sample_rate).round() as u64;
        let loop_end_frames = (plan.loop_seconds * sample_rate)
            .round()
            .max(loop_start_frames.saturating_add(1) as f64) as u64;
        let fade_frames = (DECLICK_FADE_SECONDS * sample_rate).round().max(1.0) as u64;
        let loop_fade = if loop_end_frames.saturating_sub(loop_start_frames) > fade_frames * 2 {
            fade_frames
        } else {
            0
        };
        let loop_advance_start = loop_start_frames + loop_fade;
        let master_gain = f64::from_bits(self.master_gain.load(Ordering::Relaxed));
        let stop_requested = self.stop_requested.load(Ordering::Acquire);
        let target_gain = if stop_requested { 0.0 } else { 1.0 };
        let gain_step = 1.0 / fade_frames as f64;
        let mut gain = f64::from_bits(self.transport_gain.load(Ordering::Relaxed));
        let mut position = self.position.load(Ordering::Relaxed);
        let mut stage = self.seek_stage.load(Ordering::Acquire);
        let mut target = if stage == FADE_IN {
            self.fade_target.load(Ordering::Acquire)
        } else {
            self.seek_target.load(Ordering::Acquire)
        };
        let mut elapsed = self.fade_elapsed.load(Ordering::Relaxed);
        if stop_requested && gain <= 0.0 {
            self.playing.store(false, Ordering::Release);
            self.stop_requested.store(false, Ordering::Release);
            stems[..needed].fill(0.0);
            return;
        }
        if stage == FADE_IDLE && target != NO_SEEK {
            stage = FADE_OUT;
            elapsed = 0;
        } else if stage == FADE_OUT && target == NO_SEEK {
            stage = FADE_IDLE;
            elapsed = 0;
        }
        for frame in 0..frames {
            let seek_gain = if stage == FADE_OUT {
                Some(1.0 - (elapsed + 1) as f64 / fade_frames as f64)
            } else if stage == FADE_IN {
                Some((elapsed + 1) as f64 / fade_frames as f64)
            } else {
                None
            };
            let frame_gain = seek_gain.unwrap_or(1.0) * gain * master_gain;
            for track_index in 0..track_count {
                let sample = plan.tracks.get(track_index).map_or(0.0, |track| {
                    Self::sample_track_with_loop(
                        &plan,
                        track,
                        position,
                        loop_start_frames,
                        loop_end_frames,
                        loop_fade,
                    )
                });
                stems[track_index * frames + frame] = (sample * frame_gain) as f32;
            }
            if seek_gain.is_some() {
                elapsed += 1;
                if elapsed >= fade_frames {
                    if stage == FADE_OUT {
                        target = self.seek_target.load(Ordering::Acquire);
                        if target == NO_SEEK {
                            stage = FADE_IDLE;
                            elapsed = 0;
                        } else {
                            position = target;
                            self.fade_target.store(target, Ordering::Release);
                            stage = FADE_IN;
                            elapsed = 0;
                        }
                    } else {
                        stage = FADE_IDLE;
                        elapsed = 0;
                        let latest = self.seek_target.load(Ordering::Acquire);
                        if latest != NO_SEEK && latest != target {
                            target = latest;
                            stage = FADE_OUT;
                        } else {
                            target = NO_SEEK;
                            self.seek_target.store(NO_SEEK, Ordering::Release);
                            self.fade_target.store(NO_SEEK, Ordering::Release);
                        }
                    }
                }
            }
            if stop_requested {
                gain = (gain - gain_step).max(0.0);
                if gain <= 0.0 {
                    self.playing.store(false, Ordering::Release);
                    self.stop_requested.store(false, Ordering::Release);
                    for track_index in 0..track_count {
                        stems[track_index * frames + frame + 1..(track_index + 1) * frames]
                            .fill(0.0);
                    }
                    break;
                }
            } else if gain < target_gain {
                gain = (gain + gain_step).min(target_gain);
            }
            position = advance(position, loop_end_frames, loop_advance_start);
        }
        self.position.store(position, Ordering::Release);
        self.transport_gain.store(gain.to_bits(), Ordering::Release);
        if stage != FADE_IDLE {
            self.seek_stage.store(stage, Ordering::Release);
            self.fade_elapsed.store(elapsed, Ordering::Release);
        } else if self.seek_stage.load(Ordering::Acquire) == FADE_IDLE {
            self.fade_elapsed.store(0, Ordering::Release);
        }
    }

    fn sample_with_loop(
        plan: &PlaybackPlan,
        position: u64,
        loop_start: u64,
        loop_end: u64,
        loop_fade: u64,
        master_gain: f64,
    ) -> f64 {
        if loop_fade == 0 {
            return Self::sample_at(plan, position, master_gain);
        }
        let start = loop_end.saturating_sub(loop_fade);
        if position < start || position >= loop_end {
            return Self::sample_at(plan, position, master_gain);
        }
        let offset = position - start;
        let blend = (offset + 1) as f64 / loop_fade as f64;
        let tail = Self::sample_at(plan, position, master_gain);
        let head = Self::sample_at(plan, loop_start + offset, master_gain);
        tail * (1.0 - blend) + head * blend
    }

    fn sample_at(plan: &PlaybackPlan, position: u64, master_gain: f64) -> f64 {
        let sample = plan
            .tracks
            .iter()
            .map(|track| Self::sample_track(plan, track, position))
            .sum::<f64>();
        (sample * master_gain).clamp(-1.0, 1.0)
    }

    fn sample_track(plan: &PlaybackPlan, track: &TrackPlan, position: u64) -> f64 {
        let sample_rate = f64::from(plan.sample_rate).max(1.0);
        let time = position as f64 / sample_rate;
        let mut voices = [0.0_f64; MAX_VOICES];
        let mut voice = 0.0;
        let mut has_voices = false;
        if let Some(graph) = &track.graph {
            for (index, piano) in graph.pianos.iter().enumerate().take(MAX_VOICES) {
                voices[index] = piano.sample(&track.notes, time);
                voice += voices[index];
                has_voices = true;
            }
        }
        if let Some(graph) = &track.graph
            && let Some(frequency) = graph.fallback_sine
        {
            let sine = (std::f64::consts::TAU * frequency * time).sin() * 0.25;
            voices[0] += sine;
            voice += sine;
            has_voices = true;
        }
        if !has_voices && let Some(piano) = &track.piano {
            voices[0] = piano.sample(&track.notes, time);
            voice = voices[0];
            has_voices = true;
        }
        if !has_voices {
            if let Some(frequency) = track.frequency {
                voice += (std::f64::consts::TAU * frequency * time).sin() * 0.25;
            }
            let release = track
                .adsr
                .map_or(0.0, |adsr| adsr.release.max(MIN_ENVELOPE_SECONDS));
            for note in &track.notes {
                if time >= note.start && time < note.start + note.duration + release {
                    let local = time - note.start;
                    let envelope = track.adsr.map_or_else(
                        || note_envelope(local, note.duration),
                        |adsr| adsr_envelope(local, note.duration, adsr),
                    );
                    voice += (std::f64::consts::TAU * note.frequency * local).sin()
                        * note.velocity
                        * envelope
                        * 0.25;
                }
            }
        }
        if let Some(graph) = &track.graph {
            return graph.sample(&voices, plan.sample_rate, time);
        }
        for filter in &track.filters {
            voice = filter.process(plan.sample_rate, voice);
        }
        voice * track.gain
    }

    fn sample_track_with_loop(
        plan: &PlaybackPlan,
        track: &TrackPlan,
        position: u64,
        loop_start: u64,
        loop_end: u64,
        loop_fade: u64,
    ) -> f64 {
        if loop_fade == 0 {
            return Self::sample_track(plan, track, position);
        }
        let start = loop_end.saturating_sub(loop_fade);
        if position < start || position >= loop_end {
            return Self::sample_track(plan, track, position);
        }
        let offset = position - start;
        let blend = (offset + 1) as f64 / loop_fade as f64;
        let tail = Self::sample_track(plan, track, position);
        let head = Self::sample_track(plan, track, loop_start + offset);
        tail * (1.0 - blend) + head * blend
    }
}

fn advance(position: u64, loop_frames: u64, loop_start: u64) -> u64 {
    let next = position + 1;
    if next >= loop_frames {
        loop_start
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context(local_time: f64) -> VoiceContext {
        VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time,
            gate: true,
            seed: 1,
            note_index: 0,
            iteration: 0,
        }
    }

    #[test]
    fn exp_and_sqrt_primitives_sample() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(0.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Exp, vec![0]),
                VoiceNodePlan::new(VoiceOp::Constant(9.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Sqrt, vec![2]),
                VoiceNodePlan::new(VoiceOp::Add, vec![1, 3]),
                VoiceNodePlan::new(VoiceOp::Output, vec![4]),
            ],
            5,
        );
        let context = VoiceContext {
            pitch: 440.0,
            velocity: 1.0,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn delay_line_reads_one_sample_back() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(1.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(1.0 / 48_000.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(48_000.0), vec![]),
                VoiceNodePlan::with_buffer(VoiceOp::Delay, vec![0, 1, 2, 3], 8),
                VoiceNodePlan::new(VoiceOp::Output, vec![4]),
            ],
            5,
        );
        let context = VoiceContext {
            pitch: 440.0,
            velocity: 1.0,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context)).abs() < 1e-9);
        assert!((plan.sample(context) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn audio_sine_honors_phase_control() {
        let plan = GraphPlan {
            nodes: vec![
                GraphNodePlan {
                    op: GraphOp::Constant(440.0),
                    inputs: Vec::new(),
                    controls: Vec::new(),
                },
                GraphNodePlan {
                    op: GraphOp::Constant(std::f64::consts::FRAC_PI_2),
                    inputs: Vec::new(),
                    controls: Vec::new(),
                },
                GraphNodePlan {
                    op: GraphOp::Sine,
                    inputs: Vec::new(),
                    controls: vec![0, 1],
                },
            ],
            output: 2,
            plugin: None,
            pianos: Vec::new(),
            fallback_sine: None,
        };
        assert!((plan.sample(&[0.0], 48_000, 0.0) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn biquad_passes_or_blocks_dc() {
        let context = VoiceContext {
            pitch: 440.0,
            velocity: 1.0,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        let sample = |highpass| {
            let plan = VoicePlan::new(
                vec![
                    VoiceNodePlan::new(VoiceOp::Constant(1.0), vec![]),
                    VoiceNodePlan::new(VoiceOp::Constant(500.0), vec![]),
                    VoiceNodePlan::new(VoiceOp::Constant(0.707), vec![]),
                    VoiceNodePlan::new(VoiceOp::Constant(48_000.0), vec![]),
                    VoiceNodePlan::new(VoiceOp::Biquad { highpass }, vec![0, 1, 2, 3]),
                    VoiceNodePlan::new(VoiceOp::Output, vec![4]),
                ],
                5,
            );
            let mut output = 0.0;
            for _ in 0..2048 {
                output = plan.sample(context);
            }
            output
        };
        assert!(sample(false) > 0.9);
        assert!(sample(true).abs() < 0.1);
    }

    #[test]
    fn adsr_releases_after_gate_closes() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Gate, vec![]),
                VoiceNodePlan::new(VoiceOp::Time, vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.1), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.1), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.5), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.1), vec![]),
                VoiceNodePlan::new(VoiceOp::Adsr, vec![0, 1, 2, 3, 4, 5]),
                VoiceNodePlan::new(VoiceOp::Output, vec![6]),
            ],
            7,
        );
        let mut context = VoiceContext {
            pitch: 440.0,
            velocity: 1.0,
            local_time: 0.5,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 0.5).abs() < 1e-9);
        context.gate = false;
        context.local_time = 0.55;
        assert!((plan.sample(context) - 0.5).abs() < 1e-9);
        context.local_time = 0.6;
        assert!((plan.sample(context) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn sine_oscillator_samples_frequency_and_phase() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(1.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.25), vec![]),
                VoiceNodePlan::new(VoiceOp::Sine, vec![0, 1, 2]),
                VoiceNodePlan::new(VoiceOp::Output, vec![3]),
            ],
            4,
        );
        let context = VoiceContext {
            pitch: 440.0,
            velocity: 1.0,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn math_primitives_sample() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(8.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(2.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Divide, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Constant(3.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Power, vec![3, 1]),
                VoiceNodePlan::new(VoiceOp::Subtract, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Constant(7.6), vec![]),
                VoiceNodePlan::new(VoiceOp::Floor, vec![6]),
                VoiceNodePlan::new(VoiceOp::Add, vec![2, 4]),
                VoiceNodePlan::new(VoiceOp::Output, vec![8]),
            ],
            9,
        );
        let context = VoiceContext {
            pitch: 440.0,
            velocity: 1.0,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 13.0).abs() < 1e-9);
    }

    fn plan() -> PlaybackPlan {
        PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: Some(440.0),
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: Vec::new(),
            }],
        }
    }

    #[test]
    fn voice_plan_renders_partials_and_noise() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(
                    VoiceOp::OscPartials {
                        partials: 8,
                        inharmonicity: 0.0004,
                        brightness: 0.5,
                        decay: 1.4,
                    },
                    vec![],
                ),
                VoiceNodePlan::new(
                    VoiceOp::Envelope {
                        attack: 0.004,
                        decay: 1.8,
                    },
                    vec![],
                ),
                VoiceNodePlan::new(VoiceOp::Vca, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Noise, vec![]),
                VoiceNodePlan::new(
                    VoiceOp::Envelope {
                        attack: 0.0001,
                        decay: 0.05,
                    },
                    vec![],
                ),
                VoiceNodePlan::new(VoiceOp::Vca, vec![3, 4]),
                VoiceNodePlan::new(VoiceOp::Add, vec![2, 5]),
                VoiceNodePlan::new(VoiceOp::Output, vec![6]),
            ],
            7,
        );
        let sample = plan.sample(VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.002,
            gate: true,
            seed: 7,
            note_index: 0,
            iteration: 0,
        });
        assert!(sample.abs() > 0.0001);
    }

    #[test]
    fn noise_source_varies_with_time() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Noise, vec![]),
                VoiceNodePlan::new(VoiceOp::Output, vec![0]),
            ],
            1,
        );
        let sample = |local_time| {
            plan.sample(VoiceContext {
                pitch: 60.0,
                velocity: 1.0,
                local_time,
                gate: true,
                seed: 42,
                note_index: 0,
                iteration: 0,
            })
        };
        assert!((sample(0.001) - sample(0.002)).abs() > 1e-9);
    }

    #[test]
    fn voice_index_and_white_noise_are_explicit_nodes() {
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.25,
            gate: true,
            seed: 42,
            note_index: 3,
            iteration: 0,
        };
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::NoteIndex, vec![]),
                VoiceNodePlan::new(VoiceOp::WhiteNoise, vec![0]),
                VoiceNodePlan::new(VoiceOp::Time, vec![]),
                VoiceNodePlan::new(VoiceOp::Add, vec![1, 2]),
                VoiceNodePlan::new(VoiceOp::Output, vec![3]),
            ],
            4,
        );
        let expected = white_noise(3.0) + 0.25;
        assert!((plan.sample(context) - expected).abs() < 1e-9);
    }

    #[test]
    fn switch_selects_audio_branch() {
        for (switch, expected) in [(0.0, 0.25), (1.0, 0.75)] {
            let plan = VoicePlan::new(
                vec![
                    VoiceNodePlan::new(VoiceOp::Constant(switch), vec![]),
                    VoiceNodePlan::new(VoiceOp::Constant(0.25), vec![]),
                    VoiceNodePlan::new(VoiceOp::Constant(0.75), vec![]),
                    VoiceNodePlan::new(VoiceOp::Switch, vec![0, 1, 2]),
                    VoiceNodePlan::new(VoiceOp::Output, vec![3]),
                ],
                4,
            );
            let context = VoiceContext {
                pitch: 261.63,
                velocity: 0.8,
                local_time: 0.0,
                gate: true,
                seed: 0,
                note_index: 0,
                iteration: 0,
            };
            assert!((plan.sample(context) - expected).abs() < 1e-9);
        }
    }

    #[test]
    fn group_call_reads_inputs_by_slot() {
        let body = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), vec![]),
                VoiceNodePlan::new(VoiceOp::Input(1), vec![]),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Output, vec![2]),
            ],
            3,
        );
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(2.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(3.0), vec![]),
                VoiceNodePlan::new(
                    VoiceOp::GroupCall {
                        body: Box::new(body),
                        outputs: vec![3],
                    },
                    vec![0, 1],
                ),
                VoiceNodePlan::new(VoiceOp::Output, vec![2]),
            ],
            3,
        );
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn sample_hold_keeps_previous_sample() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(0.5), vec![]),
                VoiceNodePlan::new(VoiceOp::SampleHold, vec![0]),
                VoiceNodePlan::new(VoiceOp::Output, vec![1]),
            ],
            2,
        );
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 4,
            iteration: 0,
        };
        assert!((plan.sample(context) - 0.0).abs() < 1e-9);
        assert!((plan.sample(context) - 0.5).abs() < 1e-9);
        assert!((plan.sample(context) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn broadcast_passes_control_value_as_audio() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(0.25), vec![]),
                VoiceNodePlan::new(VoiceOp::Broadcast, vec![0]),
                VoiceNodePlan::new(VoiceOp::Output, vec![1]),
            ],
            2,
        );
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn index_switch_selects_numbered_input() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(2.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.1), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.2), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.3), vec![]),
                VoiceNodePlan::new(VoiceOp::IndexSwitch { slots: 3 }, vec![0, 1, 2, 3]),
                VoiceNodePlan::new(VoiceOp::Output, vec![4]),
            ],
            5,
        );
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 0.3).abs() < 1e-9);
    }

    #[test]
    fn repeat_carries_multiple_values_across_iterations() {
        let body = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), vec![]),
                VoiceNodePlan::new(VoiceOp::Input(1), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(1.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 2]),
                VoiceNodePlan::new(VoiceOp::Add, vec![1, 2]),
            ],
            0,
        );
        let mut body = body;
        body.set_outputs(BTreeMap::from([("a".to_string(), 3), ("b".to_string(), 4)]));
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(3.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(1.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(2.0), vec![]),
                VoiceNodePlan::new(
                    VoiceOp::Repeat {
                        zone: "test".to_string(),
                        body: Box::new(body),
                        outputs: vec![3, 4],
                        carried: 2,
                    },
                    vec![0, 1, 2],
                ),
                VoiceNodePlan::new(VoiceOp::RepeatResult { source: 3, slot: 0 }, vec![]),
                VoiceNodePlan::new(VoiceOp::RepeatResult { source: 3, slot: 1 }, vec![]),
                VoiceNodePlan::new(VoiceOp::Add, vec![4, 5]),
                VoiceNodePlan::new(VoiceOp::Output, vec![6]),
            ],
            7,
        );
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 9.0).abs() < 1e-9);
        let traces = plan.trace_repeats(context);
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].zone, "test");
        assert_eq!(traces[0].iterations.len(), 3);
        assert_eq!(traces[0].iterations[0], vec![2.0, 3.0]);
        assert_eq!(traces[0].iterations[2], vec![4.0, 5.0]);
    }

    #[test]
    fn repeat_free_input_is_constant_across_iterations() {
        let body = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), vec![]),
                VoiceNodePlan::new(VoiceOp::Input(1), vec![]),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 1]),
            ],
            0,
        );
        let mut body = body;
        body.set_outputs(BTreeMap::from([("audio".to_string(), 2)]));
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(3.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(10.0), vec![]),
                VoiceNodePlan::new(
                    VoiceOp::Repeat {
                        zone: "test".to_string(),
                        body: Box::new(body),
                        outputs: vec![2],
                        carried: 1,
                    },
                    vec![0, 1, 2],
                ),
                VoiceNodePlan::new(VoiceOp::RepeatResult { source: 3, slot: 0 }, vec![]),
                VoiceNodePlan::new(VoiceOp::Output, vec![4]),
            ],
            5,
        );
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn repeat_result_reads_cached_slot() {
        let body = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(2.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 1]),
            ],
            0,
        );
        let mut body = body;
        body.set_outputs(BTreeMap::from([("a".to_string(), 2)]));
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(2.0), vec![]),
                VoiceNodePlan::new(VoiceOp::Constant(0.0), vec![]),
                VoiceNodePlan::new(
                    VoiceOp::Repeat {
                        zone: "test".to_string(),
                        body: Box::new(body),
                        outputs: vec![2],
                        carried: 1,
                    },
                    vec![0, 1],
                ),
                VoiceNodePlan::new(VoiceOp::RepeatResult { source: 2, slot: 0 }, vec![]),
                VoiceNodePlan::new(VoiceOp::Output, vec![3]),
            ],
            4,
        );
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn voice_gate_node_uses_explicit_gate_input() {
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 7,
            note_index: 0,
            iteration: 0,
        };
        for (gate, expected) in [(0.0, 0.0), (1.0, 0.5)] {
            let plan = VoicePlan::new(
                vec![
                    VoiceNodePlan::new(VoiceOp::Constant(0.5), vec![]),
                    VoiceNodePlan::new(VoiceOp::Constant(gate), vec![]),
                    VoiceNodePlan::new(VoiceOp::GateNode, vec![0, 1]),
                    VoiceNodePlan::new(VoiceOp::Output, vec![2]),
                ],
                3,
            );
            let sample = plan.sample(context);
            assert!((sample - expected).abs() < 1e-9);
        }
    }

    #[test]
    fn voice_mix_uses_factor_input() {
        let context = VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.0,
            gate: true,
            seed: 7,
            note_index: 0,
            iteration: 0,
        };
        for (factor, expected) in [(0.0, 0.25), (0.25, 0.375), (1.0, 0.75)] {
            let plan = VoicePlan::new(
                vec![
                    VoiceNodePlan::new(VoiceOp::Constant(factor), vec![]),
                    VoiceNodePlan::new(VoiceOp::Constant(0.25), vec![]),
                    VoiceNodePlan::new(VoiceOp::Constant(0.75), vec![]),
                    VoiceNodePlan::new(VoiceOp::Mix, vec![0, 1, 2]),
                    VoiceNodePlan::new(VoiceOp::Output, vec![3]),
                ],
                4,
            );
            let sample = plan.sample(context);
            assert!((sample - expected).abs() < 1e-9);
        }
    }

    #[test]
    fn piano_voice_produces_harmonics_and_decays() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 2.0,
            length_seconds: 2.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                notes: vec![NotePlan {
                    start: 0.0,
                    duration: 1.0,
                    frequency: midi_to_hz(60),
                    velocity: 0.8,
                }],
                plugin: None,
                piano: Some(PianoPlan::new(8, 1.0, 0.5, 0.2)),
            }],
        });
        playback.play();
        let mut early = vec![0.0_f32; 4800];
        playback.render(&mut early);
        let mut late = vec![0.0_f32; 4800];
        playback.render(&mut late);
        assert!(early.iter().any(|sample| sample.abs() > 0.001));
        let early_energy: f32 = early.iter().map(|sample| sample.abs()).sum();
        let late_energy: f32 = late.iter().map(|sample| sample.abs()).sum();
        assert!(late_energy < early_energy);
    }

    fn fused_body_sample(body: VoicePlan, inputs: &[f64]) -> (f64, f64) {
        let body_output = body.output;
        let constants: Vec<VoiceNodePlan> = inputs
            .iter()
            .map(|value| VoiceNodePlan::new(VoiceOp::Constant(*value), Vec::new()))
            .collect();
        let input_count = constants.len();
        let group_call = input_count;
        let group_result = input_count + 1;
        let output = input_count + 2;
        let mut nodes = constants;
        nodes.push(VoiceNodePlan::new(
            VoiceOp::GroupCall {
                body: Box::new(body),
                outputs: vec![body_output],
            },
            (0..input_count).collect(),
        ));
        nodes.push(VoiceNodePlan::new(
            VoiceOp::GroupResult {
                source: group_call,
                slot: 0,
            },
            Vec::new(),
        ));
        nodes.push(VoiceNodePlan::new(VoiceOp::Output, vec![group_result]));
        let mut plan = VoicePlan::new(nodes, output);
        let before = plan.sample(test_context(0.0));
        plan.fuse_group_calls();
        let after = plan.sample(test_context(0.0));
        (before, after)
    }

    #[test]
    fn group_fusion_preserves_map_range() {
        let body = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(0.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(1.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(0.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::MapRange, vec![0, 1, 2, 3, 4]),
                VoiceNodePlan::new(VoiceOp::Output, vec![5]),
            ],
            6,
        );
        let (before, after) = fused_body_sample(body, &[0.5]);
        assert!((before - 1.0).abs() < 1e-12, "before={before}");
        assert!((after - 1.0).abs() < 1e-12, "after={after}");
    }

    #[test]
    fn group_fusion_preserves_sine() {
        let body = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Input(1), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Input(2), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Sine, vec![0, 1, 2]),
                VoiceNodePlan::new(VoiceOp::Output, vec![3]),
            ],
            4,
        );
        let (before, after) = fused_body_sample(body, &[2.0, 0.5, 0.25]);
        assert!(
            (before - after).abs() < 1e-12,
            "before={before} after={after}"
        );
    }

    #[test]
    fn group_fusion_preserves_int_to_float() {
        let body = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::IntToFloat, vec![0]),
                VoiceNodePlan::new(VoiceOp::Output, vec![1]),
            ],
            2,
        );
        let (before, after) = fused_body_sample(body, &[1.6]);
        assert!(
            (before - after).abs() < 1e-12,
            "before={before} after={after}"
        );
    }

    #[test]
    fn optimize_remaps_nested_repeat_outputs() {
        let body = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(99.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(5.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Output, vec![1]),
            ],
            2,
        );
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(
                    VoiceOp::Repeat {
                        zone: "test".to_string(),
                        body: Box::new(body),
                        outputs: vec![2],
                        carried: 1,
                    },
                    vec![0],
                ),
                VoiceNodePlan::new(VoiceOp::Output, vec![1]),
            ],
            2,
        );
        let plan = plan.optimize();
        let VoiceOp::Repeat { body, outputs, .. } = &plan.nodes[1].op else {
            panic!("应有 Repeat");
        };
        let output = outputs[0];
        assert!(output < body.nodes.len());
        assert!(matches!(body.nodes[output].op, VoiceOp::Output));
    }

    #[test]
    fn group_fusion_preserves_input_order() {
        let body = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Input(1), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Multiply, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Output, vec![2]),
            ],
            3,
        );
        let mut plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(3.0), Vec::new()),
                VoiceNodePlan::new(
                    VoiceOp::GroupCall {
                        body: Box::new(body),
                        outputs: vec![3],
                    },
                    vec![0, 1],
                ),
                VoiceNodePlan::new(VoiceOp::GroupResult { source: 2, slot: 0 }, Vec::new()),
                VoiceNodePlan::new(VoiceOp::Output, vec![3]),
            ],
            4,
        );
        let before = plan.sample(test_context(0.0));
        plan.fuse_group_calls();
        let after = plan.sample(test_context(0.0));
        assert!((before - 6.0).abs() < 1e-9, "before={before}");
        assert!((after - 6.0).abs() < 1e-9, "after={after}");
    }

    #[test]
    fn voice_optimizer_folds_constants_and_dce() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(3.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Output, vec![2]),
            ],
            3,
        );
        let optimized = plan.optimize();
        assert!(optimized.nodes.len() <= 2);
        assert!((optimized.sample(test_context(0.0)) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn voice_optimizer_eliminates_duplicate_subexpressions() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 0]),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 0]),
                VoiceNodePlan::new(VoiceOp::Output, vec![2]),
            ],
            3,
        );
        let optimized = plan.optimize();
        assert!(optimized.nodes.len() <= 3);
        assert!((optimized.sample(test_context(0.0)) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn graph_mixes_multiple_voice_indices() {
        let plan = GraphPlan {
            nodes: vec![
                GraphNodePlan {
                    op: GraphOp::Voice { index: 0 },
                    inputs: Vec::new(),
                    controls: Vec::new(),
                },
                GraphNodePlan {
                    op: GraphOp::Voice { index: 1 },
                    inputs: Vec::new(),
                    controls: Vec::new(),
                },
                GraphNodePlan {
                    op: GraphOp::Mix,
                    inputs: vec![0, 1],
                    controls: Vec::new(),
                },
                GraphNodePlan {
                    op: GraphOp::Output,
                    inputs: vec![2],
                    controls: Vec::new(),
                },
            ],
            output: 3,
            plugin: None,
            pianos: Vec::new(),
            fallback_sine: None,
        };
        assert!((plan.sample(&[0.25, 0.5], 48_000, 0.0) - 0.75).abs() < 1e-9);
    }

    #[test]
    fn resonator_rings_after_voice_stops() {
        let plan = GraphPlan {
            nodes: vec![
                GraphNodePlan {
                    op: GraphOp::Voice { index: 0 },
                    inputs: Vec::new(),
                    controls: Vec::new(),
                },
                GraphNodePlan {
                    op: GraphOp::Resonator(ResonatorPlan::new(440.0, 12.0, 1.0)),
                    inputs: vec![0],
                    controls: Vec::new(),
                },
                GraphNodePlan {
                    op: GraphOp::Output,
                    inputs: vec![1],
                    controls: Vec::new(),
                },
            ],
            output: 2,
            plugin: None,
            pianos: Vec::new(),
            fallback_sine: None,
        };
        let mut tail = 0.0;
        for index in 0..480 {
            let _ = plan.sample(&[1.0], 48_000, index as f64 / 48_000.0);
        }
        for index in 0..480 {
            tail += plan
                .sample(&[0.0], 48_000, (480 + index) as f64 / 48_000.0)
                .abs();
        }
        assert!(tail > 0.01, "tail={tail}");
    }

    #[test]
    fn soundboard_rings_after_voice_stops() {
        let plan = GraphPlan {
            nodes: vec![
                GraphNodePlan {
                    op: GraphOp::Voice { index: 0 },
                    inputs: Vec::new(),
                    controls: Vec::new(),
                },
                GraphNodePlan {
                    op: GraphOp::Soundboard(SoundboardPlan::new()),
                    inputs: vec![0],
                    controls: Vec::new(),
                },
                GraphNodePlan {
                    op: GraphOp::Output,
                    inputs: vec![1],
                    controls: Vec::new(),
                },
            ],
            output: 2,
            plugin: None,
            pianos: Vec::new(),
            fallback_sine: None,
        };
        for index in 0..500 {
            let _ = plan.sample(&[1.0], 48_000, index as f64 / 48_000.0);
        }
        let mut tail = 0.0_f64;
        for index in 0..500 {
            tail += plan
                .sample(&[0.0], 48_000, (500 + index) as f64 / 48_000.0)
                .abs();
        }
        assert!(tail > 1e-6);
    }

    #[test]
    fn damper_follows_continuous_pedal() {
        let nodes = vec![
            VoiceNodePlan::new(VoiceOp::Constant(1.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(0.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(1.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(0.01), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Damper, vec![0, 1, 2, 3]),
            VoiceNodePlan::new(VoiceOp::Output, vec![4]),
        ];
        let plan = VoicePlan::new(nodes, 5);
        let mut context = VoiceContext {
            pitch: 440.0,
            velocity: 1.0,
            local_time: 0.0,
            gate: false,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        for _ in 0..200 {
            context.local_time += 1.0 / 48_000.0;
            let _ = plan.sample(context);
        }
        assert!(plan.sample(context) > 0.5);
    }

    #[test]
    fn waveguide_delays_hammer_excitation() {
        let nodes = vec![
            VoiceNodePlan::new(VoiceOp::Constant(1.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(100.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(48_000.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(0.99), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(1.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(0.5), Vec::new()),
            VoiceNodePlan::with_buffer(
                VoiceOp::Waveguide,
                vec![0, 1, 2, 3, 4, 5],
                VOICE_DELAY_SAMPLES + 4,
            ),
            VoiceNodePlan::new(VoiceOp::Output, vec![6]),
        ];
        let plan = VoicePlan::new(nodes, 7);
        let context = |time| VoiceContext {
            pitch: 100.0,
            velocity: 1.0,
            local_time: time,
            gate: true,
            seed: 1,
            note_index: 0,
            iteration: 0,
        };
        let _ = plan.sample(context(0.0));
        let mut late = 0.0;
        for frame in 1..=960 {
            late = plan.sample(context(f64::from(frame) / 48_000.0));
        }
        assert!(late > 0.2, "late={late}");
    }

    #[test]
    fn voice_plan_scratch_grows_with_node_count() {
        let mut nodes: Vec<VoiceNodePlan> = (0..300)
            .map(|index| VoiceNodePlan::new(VoiceOp::Constant(index as f64), Vec::new()))
            .collect();
        nodes.push(VoiceNodePlan::new(VoiceOp::Output, vec![299]));
        let plan = VoicePlan::new(nodes, 300);
        let context = VoiceContext {
            pitch: 440.0,
            velocity: 1.0,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        };
        assert!((plan.sample(context) - 299.0).abs() < 1e-9);
    }

    #[test]
    fn render_stems_keeps_tracks_separate() {
        let mut plan = plan();
        plan.tracks.push(TrackPlan {
            frequency: Some(220.0),
            gain: 0.5,
            adsr: None,
            filters: Vec::new(),
            graph: None,
            plugin: None,
            piano: None,
            notes: Vec::new(),
        });
        let playback = Playback::new(48_000);
        playback.set_plan(plan);
        playback.play();
        let frames = 256;
        let mut stems = vec![0.0_f32; frames * 2];
        playback.render_stems(&mut stems, frames, 2);
        assert!(stems[..frames].iter().any(|sample| *sample != 0.0));
        assert!(stems[frames..].iter().any(|sample| *sample != 0.0));
        assert!((stems[10] - stems[frames + 10]).abs() > 1.0e-6);
        assert!(playback.position_seconds() > 0.0);
    }

    #[test]
    fn render_produces_signal_and_advances_position() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().any(|sample| sample.abs() > 0.0));
        assert!(playback.position_seconds() > 0.0);
    }

    #[test]
    fn stop_fades_then_outputs_silence() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.play();
        playback.stop();
        assert!(!playback.is_playing());
        let mut buffer = [1.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer[200..].iter().all(|sample| *sample == 0.0));
        let mut silent = [1.0_f32; 480];
        playback.render(&mut silent);
        assert!(silent.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn play_fades_in() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.seek(1.0);
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer[0].abs() < 1e-12);
        assert!(buffer[300..].iter().any(|sample| sample.abs() > 0.0));
    }

    #[test]
    fn loop_seam_crossfades() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 0.1,
            length_seconds: 0.1,
            tracks: vec![TrackPlan {
                frequency: Some(331.0),
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: Vec::new(),
            }],
        });
        playback.play();
        let mut buffer = vec![0.0_f32; 6_000];
        playback.render(&mut buffer);
        let max_delta = buffer
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_delta < 0.03, "loop step too large: {max_delta}");
    }

    #[test]
    fn master_gain_zero_mutes() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.set_master_gain(0.0);
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn seek_wraps_into_loop() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.seek(2.5);
        assert!((playback.position_seconds() - 2.5).abs() < 0.001);
        playback.seek(6.0);
        assert!((playback.position_seconds() - 1.0).abs() < 0.001);
    }

    #[test]
    fn note_plan_produces_signal() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: vec![NotePlan {
                    start: 0.0,
                    duration: 1.0,
                    frequency: midi_to_hz(69),
                    velocity: 1.0,
                }],
            }],
        });
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().any(|sample| sample.abs() > 0.0));
    }

    #[test]
    fn note_envelope_has_soft_edges() {
        assert!(note_envelope(0.0, 1.0).abs() < 1e-12);
        assert!(note_envelope(0.002, 1.0) > 0.0);
        assert!(note_envelope(0.002, 1.0) < 0.5);
        assert!((note_envelope(0.5, 1.0) - 1.0).abs() < 1e-12);
        assert!(note_envelope(1.0, 1.0).abs() < 1e-12);
    }

    #[test]
    fn adsr_has_sustain_and_release_tail() {
        let adsr = AdsrPlan {
            attack: 0.01,
            decay: 0.01,
            sustain: 0.5,
            release: 0.1,
        };
        assert!(adsr_envelope(0.0, 0.1, adsr).abs() < 1e-12);
        assert!((adsr_envelope(0.005, 0.1, adsr) - 0.5).abs() < 1e-9);
        assert!((adsr_envelope(0.02, 0.1, adsr) - 0.5).abs() < 1e-9);
        assert!((adsr_envelope(0.15, 0.1, adsr) - 0.25).abs() < 1e-9);
        assert!(adsr_envelope(0.21, 0.1, adsr).abs() < 1e-12);
    }

    #[test]
    fn lowpass_attenuates_high_frequency() {
        let settings = FilterSettings {
            cutoff: 500.0,
            resonance: 0.2,
        };
        let mut low_state = FilterState::default();
        let mut high_state = FilterState::default();
        let mut low_energy = 0.0;
        let mut high_energy = 0.0;
        for index in 0..48_000 {
            let time = index as f64 / 48_000.0;
            let low = settings.process(
                48_000,
                &mut low_state,
                (std::f64::consts::TAU * 100.0 * time).sin(),
            );
            let high = settings.process(
                48_000,
                &mut high_state,
                (std::f64::consts::TAU * 8_000.0 * time).sin(),
            );
            if index > 4_800 {
                low_energy += low * low;
                high_energy += high * high;
            }
        }
        assert!(low_energy > high_energy * 4.0);
    }

    #[test]
    fn adsr_release_tail_is_rendered() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 1.0,
            length_seconds: 1.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: Some(AdsrPlan {
                    attack: 0.005,
                    decay: 0.005,
                    sustain: 0.8,
                    release: 0.2,
                }),
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: vec![NotePlan {
                    start: 0.0,
                    duration: 0.05,
                    frequency: 440.0,
                    velocity: 1.0,
                }],
            }],
        });
        playback.play();
        let mut buffer = vec![0.0_f32; 9_600];
        playback.render(&mut buffer);
        assert!(buffer[3_000].abs() > 0.0);
    }

    #[test]
    fn note_tail_is_declicked() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: vec![NotePlan {
                    start: 0.0,
                    duration: 0.25,
                    frequency: midi_to_hz(72),
                    velocity: 1.0,
                }],
            }],
        });
        playback.play();
        let mut buffer = vec![0.0_f32; 12_000];
        playback.render(&mut buffer);
        assert!(buffer[0].abs() < 1e-9);
        assert!(buffer[11_999].abs() < 0.005);
    }

    #[test]
    fn seek_crossfades_instead_of_stepping() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.play();
        let mut before = [0.0_f32; 480];
        playback.render(&mut before);
        playback.seek(1.234);
        let mut buffer = [0.0_f32; 960];
        playback.render(&mut buffer);
        let mut max_delta = (buffer[0] - before[479]).abs();
        for pair in buffer.windows(2) {
            max_delta = max_delta.max((pair[1] - pair[0]).abs());
        }
        assert!(max_delta < 0.03, "seek step too large: {max_delta}");
        assert!(playback.position_seconds() > 1.23);
    }

    #[test]
    fn seek_during_fade_in_is_not_lost() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.play();
        let mut warmup = [0.0_f32; 100];
        playback.render(&mut warmup);
        playback.seek(1.0);
        let mut first = [0.0_f32; 100];
        playback.render(&mut first);
        let mut second = [0.0_f32; 100];
        playback.render(&mut second);
        playback.seek(2.0);
        let mut third = [0.0_f32; 400];
        playback.render(&mut third);
        assert!((playback.position_seconds() - 2.0).abs() < 0.03);
    }

    #[test]
    fn custom_loop_range_crossfades() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.1,
            loop_seconds: 0.2,
            length_seconds: 0.3,
            tracks: vec![TrackPlan {
                frequency: Some(331.0),
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: Vec::new(),
            }],
        });
        playback.play();
        let mut buffer = vec![0.0_f32; 12_000];
        playback.render(&mut buffer);
        let max_delta = buffer
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_delta < 0.03, "custom loop step too large: {max_delta}");
    }

    #[test]
    fn output_is_clamped() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: (0..8)
                .map(|_| TrackPlan {
                    frequency: Some(440.0),
                    gain: 1.0,
                    adsr: None,
                    filters: Vec::new(),
                    graph: None,
                    plugin: None,
                    piano: None,
                    notes: Vec::new(),
                })
                .collect(),
        });
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().all(|sample| sample.abs() <= 1.0));
    }

    #[test]
    fn note_outside_window_is_silent() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: vec![NotePlan {
                    start: 2.0,
                    duration: 1.0,
                    frequency: midi_to_hz(69),
                    velocity: 1.0,
                }],
            }],
        });
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().all(|sample| *sample == 0.0));
    }
}
