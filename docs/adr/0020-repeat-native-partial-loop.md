# 0020 Repeat 原生 partial 循环

## 状态

已接受，取代 ADR 0018 的组内核决策。

## 背景

ADR 0018 让 `piano.partial` 组显式声明 `GroupKernel::PianoPartial`，编译器按
内核端口直接生成 `VoiceOp::PianoPartial`，Repeat 再用 `partial_spec` 融合。
这条路径把物理钢琴和开放钢琴从通用展开路径里摘了出来，代价是：

1. 组数据多出一套内核端口表、校验、序列化和旧工程迁移；
2. 组图被内核绕过，用户打开 `piano.partial` 改图不会影响声音；
3. 每加一种音色就多一条特化路径；ADR 0015、0016、0017 都把删除它列为后续。

通用展开路径本身可用，但 block 性能不足：8 音 2 秒离线渲染约 1.58 秒，
瓶颈是 Repeat 每次迭代都重新进入整张 partial 图。

## 决定

### 删除组内核

- 删除 `GroupKernel`、`NodeGroupData::kernel`、`with_kernel`、内核端口校验，
  以及 `group.voice` 清除内核声明的分支；
- 删除 `project.open` 的 `declare_partial_kernel` 迁移；旧工程 JSON 里的
  `kernel` 字段被忽略，不再是错误；
- `NodeGroup::partial_default()` 只保留可编辑组图，不再声明内核；
- 所有 `voice.group` 都走 `GroupCall` 展开或内联，不再有编译期旁路。

### Repeat 累加器

`VoicePlan::optimize` 为 Repeat body 编译 `RepeatAccumulator`（engine/repeat.rs）。
它只 structural 地依赖 body 图，不认识钢琴：

```text
body.output（跳过 Output/IntToFloat）
→ Add(Input(0), rest)
```

条件：

- 至少一个直接 `Input(0)`，且 `rest` 不引用 carried；
- 使用时要求 `carried == 1`、`outputs[slot] == body.output`；
- `rest` 只含无状态、可降为 SSA 的节点；`GroupCall`/`GroupResult` 按显式
  输入映射递归展开。

累加器把 SSA 值按依赖分四层：

```text
整块常量      常量 / pitch / velocity / 常量输入
整帧常量      只依赖块首已确定的值
逐迭代标量    依赖 Iteration，但不依赖 Time 或逐帧输入
逐迭代逐帧    依赖 Time 或逐帧输入
```

block 执行：

```text
sum = 初始 carried
for iteration in 0..count
    for value in 程序
        按依赖层决定本迭代是否重算
    sum += rest
output = sum
```

这样每层只算一次：频率、幅度、衰减、位置梳状滤波等逐迭代常量在迭代外
完成；三个弦振荡器、包络指数等逐帧量才进入帧循环。整张 partial 图不再
按迭代重入，也不需要专用内核公式。

不满足条件时保留原逐迭代 `body.eval_top_block` 路径；逐采样标量路径不变。

### 诊断

`BlockStats.partial_loops` 改为 `repeat_accumulations`，统计累加器调用次数，
用于证明基准真的命中新路径。`project.open` 响应保留 `declared_kernels: 0`
字段，避免旧脚本读响应时缺键。

## 后果

同一台机器、release、2 秒离线渲染：

| 场景 | 旧组内核 | 通用展开（无内核） | Repeat 累加器 |
| --- | --- | --- | --- |
| 8 音 | 0.468 秒 | 1.584 秒 | 0.66–0.68 秒 |
| 12 音 | 0.674 秒 | 未测 | 0.98–1.03 秒 |

- 8 音门槛 1.0 秒、12 音门槛 1.5 秒均满足；`accumulations` 为 9016/13524，
  `piano_block_fallbacks = 0`；
- `physical_piano_block_matches_scalar_render` 与混合起始测试的误差仍小于
  `1e-4`；
- 物理钢琴与开放钢琴的音色改由 `piano.partial` 组图决定，不再等于
  `piano_partial_sample` 的近似公式；8 音峰值 0.223、12 音峰值 0.291；
- 组图、端口和命令行语义保持显式：没有新增隐式上下文或“第一个匹配节点”。

## 已知限制

- 只处理 `Add(Input(0), rest)` 的线性 carried 累加；其他 carried 用法回退
  逐迭代求值；
- `rest` 含状态节点、`Partial`/`PianoPartial`、`OscPartials` 等未支持操作时
  不编译累加器；
- 帧循环仍是 f64 标量数学，sin/exp 是主要成本；没有 SIMD 或近似三角函数
  （ADR 0025 已补上多项式 `sin`/`exp` 并去掉帧循环的逐帧边界检查）；
- `voice.piano_partial` 节点和 `VoiceOp::PianoPartial` 曾保留给旧的内联钢琴图；
  ADR 0021 已删除它们，默认钢琴图改为显式组调用。
