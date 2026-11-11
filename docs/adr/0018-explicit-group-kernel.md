# 0018 显式组内核与 block 输入映射

## 状态

部分被 ADR 0020 取代：组内核已删除，Repeat body 累加器接管性能路径。

## 背景

ADR 0012 把 `piano.partial` 的编译期降级改成结构化比较：

```text
group.interface == NodeGroupInterface::partial()
&& group.graph == VoiceGraph::partial_default()
```

这条规则有两个问题。

第一，编译器在 `voice.group` 分支里用 `incoming` 反查端口来源，但 Repeat Zone 的
body 图已经过滤掉跨 zone 连线，`incoming` 是空的。结果 `PianoPartial` 的
pitch、velocity、time、inharmonicity、brightness、decay 全部退化成
`Constant(0)` 或接口默认值。钢琴只剩 `hammer.exciter` 的瞬态；`physical_piano_track_renders`
只断言 `peak > 0`，锤击声足以让它通过，所以 partial 银行长期静音没有被测试抓住。

第二，语义藏在整张组图的内容比较里：改一个旋钮就悄悄改变编译策略，失败模式也不可见。

## 决定

### 组显式声明内核

`NodeGroupData<G>` 增加可选字段：

```rust
pub kernel: Option<GroupKernel>
```

`GroupKernel::PianoPartial` 声明内核需要的端口：

```text
必需：pitch velocity time index inharmonicity brightness decay
可选：position
输出：audio
```

- `NodeGroup::partial_default()` 声明 `PianoPartial`；
- 校验阶段检查必需端口存在、没有 Note 输入、存在 audio 输出；
- 未声明的组照常展开，保持节点可编辑；
- `group.voice` 替换组图时清除内核声明，回到展开编译；
- `project.open` 对仍是内置组图但没有内核声明的旧工程补上声明。

编译器按端口 ID 从**已经解析好的** `inputs` 取内核参数。这个 `inputs` 由通用端口
循环生成：普通连线解析成节点下标，跨 zone 连线解析成 Repeat body 的输入槽。
内核分支不再自己反查 `incoming`，因此 zone 内的 pitch/time 会稳定落到 `Input(slot)`。

### Repeat body 槽位

跨 zone 输入槽号直接取 `body_inputs.len()`：

```text
slot = body_inputs.len()
body_inputs.push(Input(slot))
```

`Repeat.inputs[1 + slot]` 与 body 初始 `Input(slot)` 节点一一对应，槽号就是节点下标。

### 引擎输入映射

- `VoiceOp::GroupResult` 不再把外层 block 输入透传给组 body，而是复用 GroupCall 的
  输入映射：先求值 GroupCall 节点自身的输入，再进 body；
- `RepeatResult` 的 PianoPartial 融合路径先按 Repeat 的槽位求值 body 输入块，
  再调用融合 kernel；
- `partial_spec` 要求 PianoPartial 的 index 输入确实是 `Iteration`，否则退回逐帧 body。

## 后果

- 物理钢琴的 partial 银行重新发声，8 音 sample/block 峰值一致；
- 结构特化消失，组是否走内核由组数据显式决定；
- 用户替换组图后仍能走完整展开路径，block 路径也通过测试；
- 性能保持：8 音约 0.47 秒、12 音约 0.69 秒（block 路径，见 ADR 0019）。

## 已知限制

- block 调度已由 ADR 0019 修复；本 ADR 只保留组内核与 Repeat 槽位决策。
- `PianoPartial` 内核仍然是 partial 的专用语义，通用展开路径性能更低；后续若实现
  Repeat 原生 partial 循环，可以再删除该内核。

## 后续

ADR 0020 已删除 `GroupKernel` 和 `PianoPartial` 融合路径，改用通用 Repeat
累加器；本 ADR 的内核端口与迁移规则不再适用。
