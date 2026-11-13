//! 职责：
//! - 把 Repeat body 编译成按迭代求和的 SSA 内核
//! - 在 block 内提升帧不变与迭代不变的计算
//!
//! 禁止：
//! - 在音频线程构建或分配
//! - 依赖钢琴或泛音专用语义

use std::cell::UnsafeCell;
use std::collections::HashMap;

use crate::fastmath;
use crate::ssa::SsaOp;
use crate::{VoiceBlockInputs, VoiceContext, VoiceNodePlan, VoiceOp, VoicePlan};

/// 一个 body 输入槽最多占用的位数。
const MAX_EXTERNALS: usize = 32;
/// 展开组调用时的最大递归深度。
const MAX_DEPTH: usize = 96;
/// 单个累加器允许的 SSA 值上限；超出时退回逐帧 body。
const MAX_LOOP_VALUES: usize = 192;

/// 循环值在 block 内的求值时机。
#[derive(Debug, Clone)]
struct LoopValue {
    op: SsaOp,
    args: Vec<usize>,
    constant: f64,
    external: usize,
    /// 依赖 `Time`，同一 block 内必须逐帧求值。
    depends_on_time: bool,
    /// 依赖 `Iteration`，每个迭代都要重新求值。
    depends_on_iteration: bool,
    /// 依赖的 body 输入槽位掩码；非常量输入也会让值逐帧求值。
    external_mask: u32,
}

/// Repeat 的迭代累加内核。
///
/// 只对 `body.output = Add(Input(0), rest)` 且 `rest` 无状态的 body 构建；
/// 每次迭代把 `rest` 加到 carried 上，等价于一次求和。SSA 值按依赖分成
/// 四层：整块常量、逐帧常量、逐迭代标量、逐迭代逐帧。
#[derive(Debug)]
pub(crate) struct RepeatAccumulator {
    values: Vec<LoopValue>,
    output: usize,
    external_mask: u32,
    scratch: UnsafeCell<Vec<[f64; 64]>>,
}

// SAFETY: 与 `VoiceStateStore` 相同的假设：计划由单个音频线程独占求值，
// 音频回调内不会并发进入同一个累加器。
unsafe impl Sync for RepeatAccumulator {}

impl Clone for RepeatAccumulator {
    fn clone(&self) -> Self {
        Self {
            values: self.values.clone(),
            output: self.output,
            external_mask: self.external_mask,
            // NOTE: 克隆发生在控制线程；scratch 不能跨计划共享。
            scratch: UnsafeCell::new(vec![[0.0_f64; 64]; self.values.len()]),
        }
    }
}

impl RepeatAccumulator {
    /// 从 Repeat body 构建累加器；结构不匹配或含状态节点时返回 `None`。
    pub(crate) fn compile(plan: &VoicePlan) -> Option<Self> {
        let root = resolve_output(plan)?;
        let node = plan.nodes.get(root)?;
        let VoiceOp::Add = node.op else {
            return None;
        };
        let mut carried = 0_usize;
        let mut rest = Vec::new();
        for input in &node.inputs {
            match plan.nodes.get(*input).map(|node| &node.op) {
                Some(VoiceOp::Input(0)) => carried += 1,
                _ => rest.push(*input),
            }
        }
        // NOTE: 累加器只支持每次迭代加一次 carried；多个 carried 项会变成
        // 倍率递推，必须退回逐迭代 body。
        if carried != 1 || rest.is_empty() {
            return None;
        }
        let mut builder = LoopBuilder::default();
        let mut output = None;
        for input in rest {
            let value = builder.build(plan, input, &Env::Top, 0)?;
            output = Some(match output {
                Some(previous) => builder.push(SsaOp::Add, vec![previous, value]),
                None => value,
            });
        }
        let output = output?;
        // NOTE: rest 一旦引用 carried 输入，就不再是可累加的纯函数。
        if builder.externals.contains_key(&0) {
            return None;
        }
        if builder.values.len() > MAX_LOOP_VALUES {
            return None;
        }
        let mut externals = 0_u32;
        let mut meta = Vec::with_capacity(builder.values.len());
        for value in &builder.values {
            let mut depends_on_time = matches!(value.op, SsaOp::ContextTime);
            let mut depends_on_iteration = matches!(value.op, SsaOp::ContextIteration);
            let mut external_mask = if matches!(value.op, SsaOp::External) {
                1_u32 << value.external
            } else {
                0
            };
            for argument in &value.args {
                let (time, iteration, mask) = meta[*argument];
                depends_on_time |= time;
                depends_on_iteration |= iteration;
                external_mask |= mask;
            }
            externals |= external_mask;
            meta.push((depends_on_time, depends_on_iteration, external_mask));
        }
        for (value, (depends_on_time, depends_on_iteration, external_mask)) in
            builder.values.iter_mut().zip(meta)
        {
            value.depends_on_time = depends_on_time;
            value.depends_on_iteration = depends_on_iteration;
            value.external_mask = external_mask;
        }
        Some(Self {
            scratch: UnsafeCell::new(vec![[0.0_f64; 64]; builder.values.len()]),
            values: builder.values,
            output,
            external_mask: externals,
        })
    }

    /// 把 `count` 次迭代的 `rest` 求和后加上初始 carried，写入 `output`。
    ///
    /// 返回 `false` 时调用方必须退回逐迭代求值。
    // NOTE: 提升判定要求整块逐位相等，容差比较会把漂移输入错误地当成常量。
    #[allow(clippy::float_cmp)]
    pub(crate) fn accumulate(
        &self,
        context: VoiceContext,
        inputs: &VoiceBlockInputs<'_>,
        frames: usize,
        count: usize,
        carried: &[f64],
        output: &mut [f32],
    ) -> bool {
        if frames == 0 || frames > 64 || output.len() < frames || self.values.is_empty() {
            return false;
        }
        let sample_rate = inputs.sample_rate();
        // SAFETY: 调用方保证音频线程独占；scratch 每块从头写满，不读取旧值。
        let values = unsafe { &mut *self.scratch.get() };
        if values.len() != self.values.len() {
            return false;
        }
        let mut const_mask = 0_u32;
        for slot in 0..MAX_EXTERNALS {
            if self.external_mask & (1_u32 << slot) == 0 {
                continue;
            }
            let first = inputs.frame(slot, 0);
            let constant = (1..frames).all(|frame| inputs.frame(slot, frame) == first);
            if constant {
                const_mask |= 1_u32 << slot;
            }
        }
        let mut sum = [0.0_f64; 64];
        for (frame, slot) in sum[..frames].iter_mut().enumerate() {
            *slot = carried.get(frame).copied().unwrap_or(0.0);
        }
        for iteration in 0..count {
            let iteration_value = iteration as f64;
            for (index, value) in self.values.iter().enumerate() {
                if !value.depends_on_iteration && iteration > 0 {
                    continue;
                }
                let frame_constant =
                    !value.depends_on_time && value.external_mask & !const_mask == 0;
                if frame_constant {
                    let scalar = evaluate(
                        value,
                        values,
                        0,
                        context,
                        sample_rate,
                        iteration_value,
                        inputs,
                    );
                    values[index][..frames].fill(scalar);
                } else {
                    let (done, rest) = values.split_at_mut(index);
                    evaluate_frames(
                        value,
                        done,
                        &mut rest[0],
                        frames,
                        context,
                        sample_rate,
                        iteration_value,
                        inputs,
                    );
                }
            }
            for (frame, slot) in sum[..frames].iter_mut().enumerate() {
                *slot += values[self.output][frame];
            }
        }
        for (frame, slot) in output[..frames].iter_mut().enumerate() {
            *slot = sum[frame] as f32;
        }
        true
    }
}

/// 依赖展开后的输入环境：顶层 body 走外部槽位，组调用 body 走实参。
enum Env<'a> {
    Top,
    Bound(&'a [usize]),
}

impl Env<'_> {
    fn key(&self) -> Option<Vec<usize>> {
        match self {
            Self::Top => None,
            Self::Bound(values) => Some(values.to_vec()),
        }
    }
}

#[derive(Default)]
struct LoopBuilder {
    values: Vec<LoopValue>,
    constants: HashMap<u64, usize>,
    externals: HashMap<usize, usize>,
    iteration: Option<usize>,
    cache: HashMap<String, usize>,
    memo: HashMap<(usize, usize, Option<Vec<usize>>), usize>,
}

impl LoopBuilder {
    fn build(
        &mut self,
        plan: &VoicePlan,
        index: usize,
        env: &Env<'_>,
        depth: usize,
    ) -> Option<usize> {
        if depth > MAX_DEPTH {
            return None;
        }
        let key = (std::ptr::from_ref(plan) as usize, index, env.key());
        if let Some(value) = self.memo.get(&key).copied() {
            return Some(value);
        }
        let node = plan.nodes.get(index)?;
        let value = self.build_node(plan, node, env, depth)?;
        self.memo.insert(key, value);
        Some(value)
    }

    fn build_node(
        &mut self,
        plan: &VoicePlan,
        node: &VoiceNodePlan,
        env: &Env<'_>,
        depth: usize,
    ) -> Option<usize> {
        match &node.op {
            VoiceOp::Output | VoiceOp::IntToFloat => self.input(plan, node, 0, env, depth),
            VoiceOp::Input(slot) => match env {
                Env::Top => self.external(*slot),
                Env::Bound(values) => values.get(*slot).copied(),
            },
            VoiceOp::Constant(value) => Some(self.constant(*value)),
            VoiceOp::Pitch => Some(self.leaf(SsaOp::ContextPitch)),
            VoiceOp::Velocity => Some(self.leaf(SsaOp::ContextVelocity)),
            VoiceOp::Time => Some(self.leaf(SsaOp::ContextTime)),
            VoiceOp::Gate => Some(self.leaf(SsaOp::ContextGate)),
            VoiceOp::Iteration => Some(self.iteration()),
            VoiceOp::Add => self.fold(plan, node, env, depth, SsaOp::Add),
            VoiceOp::Multiply => self.fold(plan, node, env, depth, SsaOp::Multiply),
            VoiceOp::Subtract => self.binary(plan, node, env, depth, SsaOp::Subtract),
            VoiceOp::Divide => self.divide(plan, node, env, depth),
            VoiceOp::Power => self.binary(plan, node, env, depth, SsaOp::Power),
            VoiceOp::Modulo => self.binary(plan, node, env, depth, SsaOp::Modulo),
            VoiceOp::GreaterThan => self.binary(plan, node, env, depth, SsaOp::GreaterThan),
            VoiceOp::LessThan => self.binary(plan, node, env, depth, SsaOp::LessThan),
            VoiceOp::Floor => self.unary(plan, node, env, depth, SsaOp::Floor),
            VoiceOp::Ceil => self.unary(plan, node, env, depth, SsaOp::Ceil),
            VoiceOp::Round => self.unary(plan, node, env, depth, SsaOp::Round),
            VoiceOp::Exp => self.unary(plan, node, env, depth, SsaOp::Exp),
            VoiceOp::Sqrt => self.unary(plan, node, env, depth, SsaOp::Sqrt),
            VoiceOp::Sin => self.unary(plan, node, env, depth, SsaOp::Sin),
            VoiceOp::Abs => self.unary(plan, node, env, depth, SsaOp::Abs),
            VoiceOp::Clamp => self.nary(plan, node, env, depth, SsaOp::Clamp, 3),
            VoiceOp::MapRange => self.nary(plan, node, env, depth, SsaOp::MapRange, 5),
            VoiceOp::Vca => {
                let input = self.input(plan, node, 0, env, depth)?;
                let gain = if node.inputs.len() > 1 {
                    self.input(plan, node, 1, env, depth)?
                } else {
                    self.constant(1.0)
                };
                Some(self.push(SsaOp::Multiply, vec![input, gain]))
            },
            VoiceOp::Sine => {
                let frequency = if node.inputs.is_empty() {
                    self.constant(440.0)
                } else {
                    self.input(plan, node, 0, env, depth)?
                };
                let phase = if node.inputs.len() > 1 {
                    self.input(plan, node, 1, env, depth)?
                } else {
                    self.constant(0.0)
                };
                let time = if node.inputs.len() > 2 {
                    self.input(plan, node, 2, env, depth)?
                } else {
                    self.leaf(SsaOp::ContextTime)
                };
                Some(self.push(SsaOp::Sine, vec![frequency, time, phase]))
            },
            VoiceOp::GroupCall { body, outputs } => {
                if outputs.len() != 1 || outputs[0] != body.output {
                    return None;
                }
                let args = self.arguments(plan, node, env, depth)?;
                self.build(body, body.output, &Env::Bound(&args), depth + 1)
            },
            VoiceOp::GroupResult { source, slot } => {
                let call = plan.nodes.get(*source)?;
                let VoiceOp::GroupCall { body, outputs } = &call.op else {
                    return None;
                };
                let target = *outputs.get(*slot)?;
                let args = self.arguments(plan, call, env, depth)?;
                self.build(body, target, &Env::Bound(&args), depth + 1)
            },
            _ => None,
        }
    }

    fn arguments(
        &mut self,
        plan: &VoicePlan,
        node: &VoiceNodePlan,
        env: &Env<'_>,
        depth: usize,
    ) -> Option<Vec<usize>> {
        let mut args = Vec::with_capacity(node.inputs.len());
        for input in &node.inputs {
            args.push(self.build(plan, *input, env, depth + 1)?);
        }
        Some(args)
    }

    fn input(
        &mut self,
        plan: &VoicePlan,
        node: &VoiceNodePlan,
        slot: usize,
        env: &Env<'_>,
        depth: usize,
    ) -> Option<usize> {
        self.build(plan, *node.inputs.get(slot)?, env, depth + 1)
    }

    fn unary(
        &mut self,
        plan: &VoicePlan,
        node: &VoiceNodePlan,
        env: &Env<'_>,
        depth: usize,
        op: SsaOp,
    ) -> Option<usize> {
        let input = self.input(plan, node, 0, env, depth)?;
        Some(self.push(op, vec![input]))
    }

    fn binary(
        &mut self,
        plan: &VoicePlan,
        node: &VoiceNodePlan,
        env: &Env<'_>,
        depth: usize,
        op: SsaOp,
    ) -> Option<usize> {
        let left = self.input(plan, node, 0, env, depth)?;
        let right = self.input(plan, node, 1, env, depth)?;
        Some(self.push(op, vec![left, right]))
    }

    /// 常量除数折成乘倒数；block 内的除法比乘法贵得多。
    fn divide(
        &mut self,
        plan: &VoicePlan,
        node: &VoiceNodePlan,
        env: &Env<'_>,
        depth: usize,
    ) -> Option<usize> {
        let left = self.input(plan, node, 0, env, depth)?;
        let right = self.input(plan, node, 1, env, depth)?;
        let (is_constant, divisor) = self.values.get(right).map_or((false, 0.0), |value| {
            (matches!(value.op, SsaOp::Constant), value.constant)
        });
        if is_constant && divisor != 0.0 {
            let reciprocal = self.constant(1.0 / divisor);
            return Some(self.push(SsaOp::Multiply, vec![left, reciprocal]));
        }
        Some(self.push(SsaOp::Divide, vec![left, right]))
    }

    fn nary(
        &mut self,
        plan: &VoicePlan,
        node: &VoiceNodePlan,
        env: &Env<'_>,
        depth: usize,
        op: SsaOp,
        count: usize,
    ) -> Option<usize> {
        if node.inputs.len() < count {
            return None;
        }
        let mut args = Vec::with_capacity(count);
        for slot in 0..count {
            args.push(self.input(plan, node, slot, env, depth)?);
        }
        Some(self.push(op, args))
    }

    fn fold(
        &mut self,
        plan: &VoicePlan,
        node: &VoiceNodePlan,
        env: &Env<'_>,
        depth: usize,
        op: SsaOp,
    ) -> Option<usize> {
        let mut result = None;
        for slot in 0..node.inputs.len() {
            let value = self.input(plan, node, slot, env, depth)?;
            result = Some(match result {
                Some(previous) => self.push(op, vec![previous, value]),
                None => value,
            });
        }
        result
    }

    fn constant(&mut self, value: f64) -> usize {
        if let Some(index) = self.constants.get(&value.to_bits()).copied() {
            return index;
        }
        let index = self.values.len();
        self.values.push(LoopValue {
            op: SsaOp::Constant,
            args: Vec::new(),
            constant: value,
            external: 0,
            depends_on_time: false,
            depends_on_iteration: false,
            external_mask: 0,
        });
        self.constants.insert(value.to_bits(), index);
        index
    }

    fn external(&mut self, slot: usize) -> Option<usize> {
        if slot >= MAX_EXTERNALS {
            return None;
        }
        if let Some(index) = self.externals.get(&slot).copied() {
            return Some(index);
        }
        let index = self.values.len();
        self.values.push(LoopValue {
            op: SsaOp::External,
            args: Vec::new(),
            constant: 0.0,
            external: slot,
            depends_on_time: false,
            depends_on_iteration: false,
            external_mask: 0,
        });
        self.externals.insert(slot, index);
        Some(index)
    }

    fn iteration(&mut self) -> usize {
        if let Some(index) = self.iteration {
            return index;
        }
        let index = self.values.len();
        self.values.push(LoopValue {
            op: SsaOp::ContextIteration,
            args: Vec::new(),
            constant: 0.0,
            external: 0,
            depends_on_time: false,
            depends_on_iteration: false,
            external_mask: 0,
        });
        self.iteration = Some(index);
        index
    }

    fn leaf(&mut self, op: SsaOp) -> usize {
        let index = self.values.len();
        self.values.push(LoopValue {
            op,
            args: Vec::new(),
            constant: 0.0,
            external: 0,
            depends_on_time: false,
            depends_on_iteration: false,
            external_mask: 0,
        });
        index
    }

    fn push(&mut self, op: SsaOp, args: Vec<usize>) -> usize {
        let key = format!("{op:?}:{args:?}");
        if let Some(index) = self.cache.get(&key).copied() {
            return index;
        }
        let index = self.values.len();
        self.values.push(LoopValue {
            op,
            args,
            constant: 0.0,
            external: 0,
            depends_on_time: false,
            depends_on_iteration: false,
            external_mask: 0,
        });
        self.cache.insert(key, index);
        index
    }
}

/// 跳过 body 末尾的 `Output`/`IntToFloat`，找到真正的输出节点。
fn resolve_output(plan: &VoicePlan) -> Option<usize> {
    let mut index = plan.output;
    for _ in 0..64 {
        let node = plan.nodes.get(index)?;
        match node.op {
            VoiceOp::Output | VoiceOp::IntToFloat => index = *node.inputs.first()?,
            _ => return Some(index),
        }
    }
    None
}

/// 逐值处理整块，和 `SsaOp` 的 block 内核一致，避免每帧重新分派。
// PERF: 循环前先把目标和操作数裁成 `frames` 长的切片，逐帧边界检查与帧内
// 的算子分派都随之消失，帧循环变成紧凑的标量代码；这是 128 复音离线渲染
// 的主要收益。LLVM 没有把这些循环向量化：切分用的下标是运行期值，无法
// 证明共享操作数与可变目标不重叠。
#[inline]
#[allow(clippy::too_many_arguments)]
fn evaluate_frames(
    value: &LoopValue,
    done: &[[f64; 64]],
    target: &mut [f64; 64],
    frames: usize,
    context: VoiceContext,
    sample_rate: f64,
    iteration: f64,
    inputs: &VoiceBlockInputs<'_>,
) {
    let target = &mut target[..frames];
    let arguments = value.args.len();
    macro_rules! binary {
        ($operation:expr) => {{
            if arguments < 2 {
                target.fill(0.0);
                return;
            }
            let left = &done[value.args[0]][..frames];
            let right = &done[value.args[1]][..frames];
            for ((slot, &a), &b) in target.iter_mut().zip(left).zip(right) {
                *slot = $operation(a, b);
            }
        }};
    }
    macro_rules! unary {
        ($operation:expr) => {{
            if arguments < 1 {
                target.fill(0.0);
                return;
            }
            let source = &done[value.args[0]][..frames];
            for (slot, &input) in target.iter_mut().zip(source) {
                *slot = $operation(input);
            }
        }};
    }
    match value.op {
        SsaOp::Constant => target.fill(value.constant),
        SsaOp::External => {
            for (frame, slot) in target.iter_mut().enumerate() {
                *slot = inputs.frame(value.external, frame);
            }
        },
        SsaOp::ContextPitch => target.fill(context.pitch),
        SsaOp::ContextVelocity => target.fill(context.velocity),
        SsaOp::ContextTime => {
            for (frame, slot) in target.iter_mut().enumerate() {
                *slot = context.local_time + frame as f64 / sample_rate;
            }
        },
        SsaOp::ContextGate => target.fill(f64::from(u8::from(context.gate))),
        SsaOp::ContextIteration => target.fill(iteration),
        SsaOp::Add => binary!(|a, b| a + b),
        SsaOp::Subtract => binary!(|a, b| a - b),
        SsaOp::Multiply => binary!(|a, b| a * b),
        SsaOp::Divide => binary!(|a, b| if b == 0.0 { 0.0 } else { a / b }),
        SsaOp::Power => binary!(|a: f64, b: f64| a.powf(b)),
        SsaOp::Modulo => binary!(|a, b| if b == 0.0 { 0.0 } else { a % b }),
        SsaOp::GreaterThan => binary!(|a, b| f64::from(u8::from(a > b))),
        SsaOp::LessThan => binary!(|a, b| f64::from(u8::from(a < b))),
        SsaOp::Floor => unary!(|input: f64| input.floor()),
        SsaOp::Ceil => unary!(|input: f64| input.ceil()),
        SsaOp::Round => unary!(|input: f64| input.round()),
        SsaOp::Exp => unary!(fastmath::exp),
        SsaOp::Sqrt => unary!(|input: f64| input.max(0.0).sqrt()),
        SsaOp::Sin => unary!(fastmath::sin),
        SsaOp::Abs => unary!(|input: f64| input.abs()),
        SsaOp::Clamp | SsaOp::MapRange => {
            let count = if matches!(value.op, SsaOp::Clamp) {
                3
            } else {
                5
            };
            if arguments < count {
                target.fill(0.0);
                return;
            }
            let clamp = matches!(value.op, SsaOp::Clamp);
            let input = &done[value.args[0]][..frames];
            let in_min = &done[value.args[1]][..frames];
            let in_max = &done[value.args[2]][..frames];
            if clamp {
                for ((slot, &value), (&min, &max)) in
                    target.iter_mut().zip(input).zip(in_min.iter().zip(in_max))
                {
                    *slot = value.clamp(min, max);
                }
            } else {
                let out_min = &done[value.args[3]][..frames];
                let out_max = &done[value.args[4]][..frames];
                for (((slot, &value), (&from_min, &from_max)), (&to_min, &to_max)) in target
                    .iter_mut()
                    .zip(input)
                    .zip(in_min.iter().zip(in_max))
                    .zip(out_min.iter().zip(out_max))
                {
                    let span = from_max - from_min;
                    *slot = if span.abs() < 1e-12 {
                        to_min
                    } else {
                        to_min + (value - from_min) / span * (to_max - to_min)
                    };
                }
            }
        },
        SsaOp::Sine => {
            if arguments < 3 {
                target.fill(0.0);
                return;
            }
            let frequency = &done[value.args[0]][..frames];
            let time = &done[value.args[1]][..frames];
            let phase = &done[value.args[2]][..frames];
            for (((slot, &frequency), &time), &phase) in
                target.iter_mut().zip(frequency).zip(time).zip(phase)
            {
                *slot = fastmath::sin(std::f64::consts::TAU * frequency * time + phase);
            }
        },
    }
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn evaluate(
    value: &LoopValue,
    values: &[[f64; 64]],
    frame: usize,
    context: VoiceContext,
    sample_rate: f64,
    iteration: f64,
    inputs: &VoiceBlockInputs<'_>,
) -> f64 {
    let at = |slot: usize| values[value.args[slot]][frame];
    match value.op {
        SsaOp::Constant => value.constant,
        SsaOp::External => inputs.frame(value.external, frame),
        SsaOp::ContextPitch => context.pitch,
        SsaOp::ContextVelocity => context.velocity,
        SsaOp::ContextTime => context.local_time + frame as f64 / sample_rate,
        SsaOp::ContextGate => f64::from(u8::from(context.gate)),
        SsaOp::ContextIteration => iteration,
        SsaOp::Add => at(0) + at(1),
        SsaOp::Subtract => at(0) - at(1),
        SsaOp::Multiply => at(0) * at(1),
        SsaOp::Divide => {
            if at(1) == 0.0 {
                0.0
            } else {
                at(0) / at(1)
            }
        },
        SsaOp::Power => at(0).powf(at(1)),
        SsaOp::Modulo => {
            if at(1) == 0.0 {
                0.0
            } else {
                at(0) % at(1)
            }
        },
        SsaOp::Floor => at(0).floor(),
        SsaOp::Ceil => at(0).ceil(),
        SsaOp::Round => at(0).round(),
        SsaOp::Exp => fastmath::exp(at(0)),
        SsaOp::Sqrt => at(0).max(0.0).sqrt(),
        SsaOp::Sin => fastmath::sin(at(0)),
        SsaOp::Abs => at(0).abs(),
        SsaOp::GreaterThan => f64::from(u8::from(at(0) > at(1))),
        SsaOp::LessThan => f64::from(u8::from(at(0) < at(1))),
        SsaOp::MapRange => {
            let input = at(0);
            let in_min = at(1);
            let in_max = at(2);
            let out_min = at(3);
            let out_max = at(4);
            let span = in_max - in_min;
            if span.abs() < 1e-12 {
                out_min
            } else {
                out_min + (input - in_min) / span * (out_max - out_min)
            }
        },
        SsaOp::Clamp => at(0).clamp(at(1), at(2)),
        SsaOp::Sine => {
            let frequency = at(0);
            let time = at(1);
            let phase = at(2);
            fastmath::sin(std::f64::consts::TAU * frequency * time + phase)
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn context() -> VoiceContext {
        VoiceContext {
            pitch: 220.0,
            velocity: 0.8,
            local_time: 0.1,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        }
    }

    /// `carried + Σ (index + constant)`；index 来自 Iteration。
    fn accumulator_body() -> VoicePlan {
        VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Iteration, Vec::new()),
                VoiceNodePlan::new(VoiceOp::Add, vec![1, 2]),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 3]),
                VoiceNodePlan::new(VoiceOp::Output, vec![4]),
            ],
            5,
        )
    }

    #[test]
    fn compiles_carried_add_body() {
        let plan = accumulator_body();
        let accumulator = RepeatAccumulator::compile(&plan).unwrap();
        let mut block = [0.0_f32; 64];
        let empty = [[0.0_f64; 64]; 0];
        let inputs = VoiceBlockInputs::new(&empty, 64, 48_000.0);
        let carried = [0.25_f64; 64];
        assert!(accumulator.accumulate(context(), &inputs, 64, 3, &carried, &mut block));
        // 0.25 + (2+0) + (2+1) + (2+2) = 9.25
        assert!(block.iter().all(|value| (*value - 9.25).abs() < 1e-6));
    }

    #[test]
    fn rejects_body_that_uses_carried_in_rest() {
        let mut plan = accumulator_body();
        plan.nodes[3].inputs = vec![0];
        plan.nodes[4].inputs = vec![0, 3];
        assert!(RepeatAccumulator::compile(&plan).is_none());
    }

    #[test]
    fn rejects_body_with_double_carried_term() {
        let mut plan = accumulator_body();
        plan.nodes[4].inputs = vec![0, 0, 3];
        assert!(RepeatAccumulator::compile(&plan).is_none());
    }

    #[test]
    fn falls_back_for_stateful_body() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Input(0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Damper, Vec::new()),
                VoiceNodePlan::new(VoiceOp::Add, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Output, vec![2]),
            ],
            3,
        );
        assert!(RepeatAccumulator::compile(&plan).is_none());
    }
}
