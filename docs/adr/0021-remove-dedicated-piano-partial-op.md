# 0021 删除最后的专用 piano partial 节点

## 状态

已接受，接续 ADR 0020。

## 背景

ADR 0020 删除了 `GroupKernel::PianoPartial` 和编译期旁路，物理钢琴与开放钢琴
都改由 `piano.partial` 组图 + Repeat 累加器发声。但当时仍保留了三条遗留路径：

1. `VoiceOp::PianoPartial` 及其块、逐采样两个求值分支；
2. `voice.piano_partial` 节点类型，仍出现在 `node.types` 注册表里；
3. session 把 `voice.piano_partial` 映射成该 op。

它们合起来构成 `piano.partial` 组图的第二套实现：`piano_partial_sample` 用近似
公式直接算泛音，不读组图。同一个 `piano.partial` 组，经旧节点和经组图会得到
不同音色，用户改组图对旧节点无效。内置图自 f360790 起已不再引用该节点，继续
保留只会留下永不执行的死代码，并让公开枚举长期携带一个无法解释的变体。

## 决定

### 删除专用 op 与节点

- 删除 `VoiceOp::PianoPartial`，块求值、逐采样求值两个分支和
  `piano_partial_sample` 一并删除；
- 删除 `voice.piano_partial` 节点类型与 `PIANO_PARTIAL_INPUTS`；
- session 不再把任何节点类型映射到该 op；
- `VoiceOp` 仍为 `#[non_exhaustive]`，但删除变体是公开枚举的破坏性变更。

### 默认钢琴图改为显式组调用

`VoiceGraph::piano_default()` 的 `p-partial` 从内联 `voice.partial` 改为
`voice.group`，`group = "piano.partial"`，inharmonicity、brightness、decay
作为显式端口参数；`piano_open()` 与内联版本合并，只保留一个默认图。这样：

- 默认 `piano.voice`、物理钢琴、开放钢琴都只经过 `piano.partial` 组图；
- `voice.partial` 仍作为 ADR 0009 的原子节点保留，但内置图不再引用它。

### 旧工程迁移

`project.open` 新增 `migrate_removed_piano_partial`：把节点组里残留的
`voice.piano_partial` 改写成 `voice.group` 调用 `piano.partial`，`out` 端口
重命名为 `audio`，删除组接口没有的 `position` 连线与参数，并在响应里报告
`migrated_partial_nodes`。旧工程里内联 `voice.partial` 的图不受影响，因为该
节点仍然受支持。

## 后果

- `node.types` 不再出现 `voice.piano_partial`，新工程和旧工程都只有一条
  partial 实现；
- 编译计划里不再有专用 op：`compiled_piano_plan_has_no_dedicated_partial_op`
  同时断言注册表和计划文本；
- 同一台机器、release、2 秒离线渲染：8 音 674–696 ms、12 音 1.05–1.16 s，
  `piano_block_fallbacks = 0`，`repeat_accumulations` 仍为 9016/13524，峰值
  0.223/0.291，与删除前一致，说明编译后的计划没有变化；
- `physical_piano_block_matches_scalar_render` 与混合起始测试仍要求误差
  小于 `1e-4`；
- 默认 `piano.voice` 与物理钢琴共用同一张 `piano.partial` 组图，音色只由组
  图决定。

## 已知限制

- 旧工程迁移会丢弃 `voice.piano_partial` 的 `position` 输入，音色改由组图
  决定，不再等于 `piano_partial_sample` 的近似公式；
- `voice.partial` 原子节点保留，它和 `piano.partial` 组仍是两套公式；内置图
  不再引用前者，用户自建图仍可混用；
- 帧循环仍是 f64 标量数学，sin/exp 是主要成本；没有 SIMD 或近似三角函数。
