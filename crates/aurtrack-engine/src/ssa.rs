//! 职责：
//! - 把无状态 VoicePlan 降为 SSA 值
//! - 做常量传播、CSE 和 DCE
//!
//! 禁止：
//! - 在音频线程构建 IR
//! - 处理有状态节点或组调用

use std::collections::HashMap;

use crate::{VoiceContext, VoiceOp, VoicePlan};

/// SSA 指令操作符。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsaOp {
    Constant,
    External,
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
pub(crate) struct SsaValue {
    pub(crate) op: SsaOp,
    pub(crate) args: Vec<usize>,
    pub(crate) constant: Option<f64>,
    pub(crate) external: Option<usize>,
}

/// block 处理的编译期 scratch；由计划持有，音频线程只读写。
#[derive(Debug, Clone)]
pub struct BlockScratch {
    values: Vec<[f64; 64]>,
}

impl BlockScratch {
    /// 按 SSA 值数量分配，调用点不在音频线程。
    pub fn new(value_count: usize) -> Self {
        Self {
            values: vec![[0.0_f64; 64]; value_count],
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }
}

/// 无状态 VoicePlan 的 SSA 程序。
#[derive(Debug, Clone)]
pub struct VoiceSsa {
    values: Vec<SsaValue>,
    output: usize,
}

impl VoiceSsa {
    /// 构建 SSA；遇到有状态节点、组调用或未支持操作时返回 `None`。
    pub fn compile(plan: &VoicePlan) -> Option<Self> {
        let mut compiler = SsaCompiler {
            plan,
            values: Vec::new(),
            memo: vec![None; plan.nodes.len()],
            building: vec![false; plan.nodes.len()],
            constants: Vec::new(),
            cache: HashMap::new(),
        };
        let output = compiler.build(plan.output)?;
        let mut result = Self {
            values: compiler.values,
            output,
        };
        result.remove_dead();
        Some(result)
    }

    /// 指令数，用于优化测试和性能诊断。
    pub fn instruction_count(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn values(&self) -> &[SsaValue] {
        &self.values
    }

    pub(crate) fn root(&self) -> usize {
        self.output
    }

    /// block 接口：按 SSA 值处理整块，而不是逐帧重跑整张图。
    pub fn process_block(
        &self,
        context: VoiceContext,
        inputs: &[f64],
        sample_rate: f64,
        scratch: &mut BlockScratch,
        output: &mut [f32],
    ) {
        const BLOCK: usize = 64;
        if output.len() > BLOCK
            || self.values.len() > scratch.values.len()
            || self.output >= scratch.values.len()
        {
            output.fill(0.0);
            return;
        }
        let frames = output.len();
        let values = &mut scratch.values;
        for (index, value) in self.values.iter().enumerate() {
            for frame in 0..frames {
                values[index][frame] = match value.op {
                    SsaOp::Constant => value.constant.unwrap_or(0.0),
                    SsaOp::External => inputs
                        .get(value.external.unwrap_or(usize::MAX))
                        .copied()
                        .unwrap_or(0.0),
                    SsaOp::ContextPitch => context.pitch,
                    SsaOp::ContextVelocity => context.velocity,
                    SsaOp::ContextTime => context.local_time + frame as f64 / sample_rate,
                    SsaOp::ContextGate => f64::from(u8::from(context.gate)),
                    SsaOp::Add => values[value.args[0]][frame] + values[value.args[1]][frame],
                    SsaOp::Subtract => values[value.args[0]][frame] - values[value.args[1]][frame],
                    SsaOp::Multiply => values[value.args[0]][frame] * values[value.args[1]][frame],
                    SsaOp::Divide => {
                        if values[value.args[1]][frame] == 0.0 {
                            0.0
                        } else {
                            values[value.args[0]][frame] / values[value.args[1]][frame]
                        }
                    },
                    SsaOp::Power => values[value.args[0]][frame].powf(values[value.args[1]][frame]),
                    SsaOp::Modulo => {
                        if values[value.args[1]][frame] == 0.0 {
                            0.0
                        } else {
                            values[value.args[0]][frame] % values[value.args[1]][frame]
                        }
                    },
                    SsaOp::Floor => values[value.args[0]][frame].floor(),
                    SsaOp::Ceil => values[value.args[0]][frame].ceil(),
                    SsaOp::Round => values[value.args[0]][frame].round(),
                    SsaOp::Exp => values[value.args[0]][frame].exp(),
                    SsaOp::Sqrt => values[value.args[0]][frame].max(0.0).sqrt(),
                    SsaOp::Sin => values[value.args[0]][frame].sin(),
                    SsaOp::Abs => values[value.args[0]][frame].abs(),
                    SsaOp::GreaterThan => f64::from(u8::from(
                        values[value.args[0]][frame] > values[value.args[1]][frame],
                    )),
                    SsaOp::LessThan => f64::from(u8::from(
                        values[value.args[0]][frame] < values[value.args[1]][frame],
                    )),
                    SsaOp::MapRange => {
                        let input = values[value.args[0]][frame];
                        let in_min = values[value.args[1]][frame];
                        let in_max = values[value.args[2]][frame];
                        let out_min = values[value.args[3]][frame];
                        let out_max = values[value.args[4]][frame];
                        let span = in_max - in_min;
                        if span.abs() < 1e-12 {
                            out_min
                        } else {
                            out_min + (input - in_min) / span * (out_max - out_min)
                        }
                    },
                    SsaOp::Clamp => values[value.args[0]][frame]
                        .clamp(values[value.args[1]][frame], values[value.args[2]][frame]),
                    SsaOp::Sine => {
                        let frequency = values[value.args[0]][frame];
                        let time = values[value.args[1]][frame];
                        let phase = values[value.args[2]][frame];
                        (std::f64::consts::TAU * frequency * time + phase).sin()
                    },
                };
            }
        }
        for (frame, slot) in output.iter_mut().enumerate() {
            *slot = values[self.output][frame] as f32;
        }
    }

    /// 在音频线程求值；固定栈，无分配。
    pub fn eval(&self, context: &VoiceContext, inputs: &[f64]) -> f64 {
        let mut values = [0.0_f64; 256];
        for (index, value) in self.values.iter().enumerate() {
            values[index] = match value.op {
                SsaOp::Constant => value.constant.unwrap_or(0.0),
                SsaOp::External => inputs
                    .get(value.external.unwrap_or(usize::MAX))
                    .copied()
                    .unwrap_or(0.0),
                SsaOp::ContextPitch => context.pitch,
                SsaOp::ContextVelocity => context.velocity,
                SsaOp::ContextTime => context.local_time,
                SsaOp::ContextGate => f64::from(u8::from(context.gate)),
                SsaOp::Add => values[value.args[0]] + values[value.args[1]],
                SsaOp::Subtract => values[value.args[0]] - values[value.args[1]],
                SsaOp::Multiply => values[value.args[0]] * values[value.args[1]],
                SsaOp::Divide => {
                    if values[value.args[1]] == 0.0 {
                        0.0
                    } else {
                        values[value.args[0]] / values[value.args[1]]
                    }
                },
                SsaOp::Power => values[value.args[0]].powf(values[value.args[1]]),
                SsaOp::Modulo => {
                    if values[value.args[1]] == 0.0 {
                        0.0
                    } else {
                        values[value.args[0]] % values[value.args[1]]
                    }
                },
                SsaOp::Floor => values[value.args[0]].floor(),
                SsaOp::Ceil => values[value.args[0]].ceil(),
                SsaOp::Round => values[value.args[0]].round(),
                SsaOp::Exp => values[value.args[0]].exp(),
                SsaOp::Sqrt => values[value.args[0]].max(0.0).sqrt(),
                SsaOp::Sin => values[value.args[0]].sin(),
                SsaOp::Abs => values[value.args[0]].abs(),
                SsaOp::GreaterThan => {
                    f64::from(u8::from(values[value.args[0]] > values[value.args[1]]))
                },
                SsaOp::LessThan => {
                    f64::from(u8::from(values[value.args[0]] < values[value.args[1]]))
                },
                SsaOp::MapRange => {
                    let input = values[value.args[0]];
                    let in_min = values[value.args[1]];
                    let in_max = values[value.args[2]];
                    let out_min = values[value.args[3]];
                    let out_max = values[value.args[4]];
                    let span = in_max - in_min;
                    if span.abs() < 1e-12 {
                        out_min
                    } else {
                        out_min + (input - in_min) / span * (out_max - out_min)
                    }
                },
                SsaOp::Clamp => {
                    values[value.args[0]].clamp(values[value.args[1]], values[value.args[2]])
                },
                SsaOp::Sine => {
                    let frequency = values[value.args[0]];
                    let time = values[value.args[1]];
                    let phase = values[value.args[2]];
                    (std::f64::consts::TAU * frequency * time + phase).sin()
                },
            };
        }
        values[self.output]
    }

    fn remove_dead(&mut self) {
        let mut live = vec![false; self.values.len()];
        let mut stack = vec![self.output];
        while let Some(index) = stack.pop() {
            if live.get(index) == Some(&true) {
                continue;
            }
            if let Some(slot) = live.get_mut(index) {
                *slot = true;
            }
            if let Some(value) = self.values.get(index) {
                stack.extend(value.args.iter().copied());
            }
        }
        let mut mapping = vec![usize::MAX; self.values.len()];
        let mut compact = Vec::with_capacity(self.values.len());
        for (index, value) in self.values.iter().enumerate() {
            if live[index] {
                mapping[index] = compact.len();
                compact.push(value.clone());
            }
        }
        for value in &mut compact {
            for argument in &mut value.args {
                *argument = mapping[*argument];
            }
        }
        self.output = mapping.get(self.output).copied().unwrap_or(0);
        self.values = compact;
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{VoiceNodePlan, VoicePlan};

    fn context() -> VoiceContext {
        VoiceContext {
            pitch: 440.0,
            velocity: 1.0,
            local_time: 0.0,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        }
    }

    #[test]
    fn block_matches_scalar_eval() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Time, Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Multiply, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Output, vec![2]),
            ],
            3,
        );
        let ssa = VoiceSsa::compile(&plan).unwrap();
        let mut block = [0.0_f32; 64];
        let mut scratch = BlockScratch::new(ssa.instruction_count());
        ssa.process_block(context(), &[], 48_000.0, &mut scratch, &mut block);
        for (offset, value) in block.iter().enumerate() {
            let mut frame = context();
            frame.local_time = offset as f64 / 48_000.0;
            assert!((f64::from(*value) - ssa.eval(&frame, &[])).abs() < 1e-6);
        }
    }

    #[test]
    fn folds_constants_and_removes_dead_values() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(3.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(99.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Output, vec![3]),
            ],
            4,
        );
        let ssa = VoiceSsa::compile(&plan).unwrap();
        assert_eq!(ssa.instruction_count(), 1);
        assert!((ssa.eval(&context(), &[]) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn eliminates_common_subexpressions() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(3.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Multiply, vec![2, 3]),
                VoiceNodePlan::new(VoiceOp::Output, vec![4]),
            ],
            5,
        );
        let ssa = VoiceSsa::compile(&plan).unwrap();
        assert_eq!(ssa.instruction_count(), 1);
        assert!((ssa.eval(&context(), &[]) - 25.0).abs() < 1e-12);
    }
}

struct SsaCompiler<'a> {
    plan: &'a VoicePlan,
    values: Vec<SsaValue>,
    memo: Vec<Option<usize>>,
    building: Vec<bool>,
    constants: Vec<Option<f64>>,
    cache: HashMap<String, usize>,
}

impl SsaCompiler<'_> {
    fn build(&mut self, index: usize) -> Option<usize> {
        if let Some(value) = self.memo.get(index).copied().flatten() {
            return Some(value);
        }
        if *self.building.get(index)? {
            return None;
        }
        *self.building.get_mut(index)? = true;
        let node = self.plan.nodes.get(index)?;
        let result = match &node.op {
            VoiceOp::Output => self.build(*node.inputs.first()?)?,
            VoiceOp::Input(slot) => self.push_leaf(SsaOp::External, None, Some(*slot))?,
            VoiceOp::Constant(value) => self.push_leaf(SsaOp::Constant, Some(*value), None)?,
            VoiceOp::Pitch => self.push_leaf(SsaOp::ContextPitch, None, None)?,
            VoiceOp::Velocity => self.push_leaf(SsaOp::ContextVelocity, None, None)?,
            VoiceOp::Time => self.push_leaf(SsaOp::ContextTime, None, None)?,
            VoiceOp::Gate => self.push_leaf(SsaOp::ContextGate, None, None)?,
            VoiceOp::Add => self.fold(node.inputs.iter().copied(), SsaOp::Add, 0.0)?,
            VoiceOp::Multiply => self.fold(node.inputs.iter().copied(), SsaOp::Multiply, 1.0)?,
            VoiceOp::Subtract => self.binary(node.inputs.as_slice(), SsaOp::Subtract)?,
            VoiceOp::Divide => self.binary(node.inputs.as_slice(), SsaOp::Divide)?,
            VoiceOp::Power => self.binary(node.inputs.as_slice(), SsaOp::Power)?,
            VoiceOp::Modulo => self.binary(node.inputs.as_slice(), SsaOp::Modulo)?,
            VoiceOp::GreaterThan => self.binary(node.inputs.as_slice(), SsaOp::GreaterThan)?,
            VoiceOp::LessThan => self.binary(node.inputs.as_slice(), SsaOp::LessThan)?,
            VoiceOp::Floor => self.unary(node.inputs.first().copied(), SsaOp::Floor)?,
            VoiceOp::Ceil => self.unary(node.inputs.first().copied(), SsaOp::Ceil)?,
            VoiceOp::Round => self.unary(node.inputs.first().copied(), SsaOp::Round)?,
            VoiceOp::Exp => self.unary(node.inputs.first().copied(), SsaOp::Exp)?,
            VoiceOp::Sqrt => self.unary(node.inputs.first().copied(), SsaOp::Sqrt)?,
            VoiceOp::Sin => self.unary(node.inputs.first().copied(), SsaOp::Sin)?,
            VoiceOp::Abs => self.unary(node.inputs.first().copied(), SsaOp::Abs)?,
            VoiceOp::IntToFloat => self.build(*node.inputs.first()?)?,
            VoiceOp::Vca => self.binary(node.inputs.as_slice(), SsaOp::Multiply)?,
            VoiceOp::Clamp => self.nary(node.inputs.as_slice(), SsaOp::Clamp)?,
            VoiceOp::MapRange => self.nary(node.inputs.as_slice(), SsaOp::MapRange)?,
            VoiceOp::Sine => self.sine(node.inputs.as_slice())?,
            _ => return None,
        };
        *self.building.get_mut(index)? = false;
        self.memo[index] = Some(result);
        Some(result)
    }

    #[allow(clippy::unnecessary_wraps)]
    fn push_leaf(
        &mut self,
        op: SsaOp,
        constant: Option<f64>,
        external: Option<usize>,
    ) -> Option<usize> {
        let key = format!("{op:?}:{constant:?}:{external:?}");
        if let Some(existing) = self.cache.get(&key).copied() {
            return Some(existing);
        }
        let index = self.values.len();
        self.values.push(SsaValue {
            op,
            args: Vec::new(),
            constant,
            external,
        });
        self.constants.push(constant);
        self.cache.insert(key, index);
        Some(index)
    }

    fn unary(&mut self, input: Option<usize>, op: SsaOp) -> Option<usize> {
        let input = self.build(input?)?;
        if let Some(value) = self.constants[input] {
            let folded = match op {
                SsaOp::Floor => value.floor(),
                SsaOp::Ceil => value.ceil(),
                SsaOp::Round => value.round(),
                SsaOp::Exp => value.exp(),
                SsaOp::Sqrt => value.max(0.0).sqrt(),
                SsaOp::Sin => value.sin(),
                SsaOp::Abs => value.abs(),
                _ => return None,
            };
            return self.push_leaf(SsaOp::Constant, Some(folded), None);
        }
        self.push_op(op, vec![input])
    }

    fn binary(&mut self, inputs: &[usize], op: SsaOp) -> Option<usize> {
        if inputs.len() < 2 {
            return None;
        }
        let left = self.build(inputs[0])?;
        let right = self.build(inputs[1])?;
        self.fold_binary(left, right, op)
    }

    fn fold(
        &mut self,
        inputs: impl Iterator<Item = usize>,
        op: SsaOp,
        empty: f64,
    ) -> Option<usize> {
        let mut result = None;
        for input in inputs {
            let value = self.build(input)?;
            result = Some(match result {
                Some(previous) => self.fold_binary(previous, value, op)?,
                None => value,
            });
        }
        result.or_else(|| self.push_const(empty))
    }

    fn fold_binary(&mut self, left: usize, right: usize, op: SsaOp) -> Option<usize> {
        if let (Some(a), Some(b)) = (self.constants[left], self.constants[right]) {
            let folded = match op {
                SsaOp::Add => a + b,
                SsaOp::Subtract => a - b,
                SsaOp::Multiply => a * b,
                SsaOp::Divide => {
                    if b == 0.0 {
                        0.0
                    } else {
                        a / b
                    }
                },
                SsaOp::Power => a.powf(b),
                SsaOp::Modulo => {
                    if b == 0.0 {
                        0.0
                    } else {
                        a % b
                    }
                },
                SsaOp::GreaterThan => f64::from(u8::from(a > b)),
                SsaOp::LessThan => f64::from(u8::from(a < b)),
                _ => return None,
            };
            return self.push_leaf(SsaOp::Constant, Some(folded), None);
        }
        self.push_op(op, vec![left, right])
    }

    fn sine(&mut self, inputs: &[usize]) -> Option<usize> {
        if inputs.len() < 3 {
            return None;
        }
        let frequency = self.build(inputs[0])?;
        let phase = self.build(inputs[1])?;
        let time = self.build(inputs[2])?;
        self.push_op(SsaOp::Sine, vec![frequency, time, phase])
    }

    fn nary(&mut self, inputs: &[usize], op: SsaOp) -> Option<usize> {
        if inputs.len() < 2 {
            return None;
        }
        let mut args = Vec::with_capacity(inputs.len());
        for input in inputs {
            args.push(self.build(*input)?);
        }
        if args.iter().all(|index| self.constants[*index].is_some()) {
            return None;
        }
        self.push_op(op, args)
    }

    #[allow(clippy::unnecessary_wraps)]
    fn push_op(&mut self, op: SsaOp, args: Vec<usize>) -> Option<usize> {
        let key = format!("{op:?}:{args:?}");
        if let Some(existing) = self.cache.get(&key).copied() {
            return Some(existing);
        }
        let index = self.values.len();
        self.values.push(SsaValue {
            op,
            args,
            constant: None,
            external: None,
        });
        self.constants.push(None);
        self.cache.insert(key, index);
        Some(index)
    }

    fn push_const(&mut self, value: f64) -> Option<usize> {
        self.push_leaf(SsaOp::Constant, Some(value), None)
    }
}
