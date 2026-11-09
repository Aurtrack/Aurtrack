# 0016 VoicePlan block ABI v2

## 状态

已接受，开始实现。

## 背景

现有：

```rust
VoicePlan::process_block(
    state: &mut VoiceState,
    context: VoiceContext,
    inputs: &[f64],
    sample_rate: f64,
    output: &mut [f32],
) -> bool
```

`inputs` 是整块常量，无法表达 Repeat 的逐帧 carried。

真实钢琴图：

```text
Output
→ Add
→ RepeatResult
→ Repeat { outputs: [8], carried: 1 }
```

所以需要新的声部 block ABI。

## ABI

```rust
pub struct VoiceBlockInputs<'a> {
    values: &'a [[f64; 64]],
    frames: usize,
}

impl VoiceBlockInputs<'_> {
    pub fn frame(&self, input: usize, frame: usize) -> f64;
}

pub fn process_block_v2(
    &self,
    state: &mut VoiceState,
    context: VoiceContext,
    inputs: &VoiceBlockInputs<'_>,
    sample_rate: f64,
    output: &mut [f32],
) -> bool;
```

约束：

- 最多 64 帧；
- 输入数量固定，调用方分配；
- 音频线程不分配。

## 无状态节点

`VoiceSsa` 增加块输入入口：

```text
SsaOp::External(index)
→ inputs.frame(index, frame)
```

现有 `&[f64]` 入口保留，内部把常量广播成 1 帧输入。

## Repeat 内核

```text
count = 求值第一个输入
carried[frame] = 输入 1 的块值
for iteration in 0..count:
    body_inputs[0] = carried
    body.process_block_v2(...)
    carried = body_output[..carried_count]
    result = body_output[outputs]
```

`carried` 是 `[[f64; 64]; MAX_REPEAT_OUTPUTS]`，每帧独立推进。

## GroupCall

```text
body.process_block_v2
→ outputs 映射
```

只支持恒等输出先落地；非恒等映射回退。

## 回退

以下情况返回 false，由 `PianoPlan::sample_block` 回退逐采样：

```text
非 64 帧
输入超过上限
非恒等 GroupCall 输出
body 有状态但未实现
```

## 实施顺序

1. `VoiceBlockInputs` 和 `VoiceSsa::process_block_inputs`；
2. `VoicePlan::process_block_v2` 无状态路径；
3. Repeat carried=1 单层；
4. GroupCall 恒等；
5. PianoPlan 接入；
6. 真实钢琴差分和性能；
7. 删除 `PianoPartial`。
