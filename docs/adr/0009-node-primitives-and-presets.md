# ADR 0009：底层原语与可展开预设组

- **状态：** 已接受
- **日期：** 2026-11-01
- **决策者：** 项目维护者

## 背景

Aurtrack 的节点系统里有一批“看起来是节点、实际是复合算法”的黑箱，例如 `voice.partial` 内部同时完成了正弦振荡、非谐性、亮度、衰减和归一化。用户可以调参数，但不能改内部结构，也不能把其中一部分替换成自己的相位、波形或滤波。

这和“设备即节点组”的目标冲突。Blender 和 Unreal 的经验说明：可复用预设不应该是不可打开的黑箱，而应该是可编辑的子图；真正需要优化时，由编译器处理，而不是把结构藏起来。

## 决策

### 1. 三层节点

- **原语节点**：一个明确的 DSP 操作，输入输出可见，状态显式。
- **宏 / 预设组**：由原语组成的 `NodeGroup`，可以进入、展开、复制、修改。
- **实例**：轨道或 Voice 图里引用一个组，保留自己的参数覆盖。

预设组不是黑箱。必须满足：

- 能打开看到内部节点。
- 能复制成独立组，不自动跟随上游修改。
- 有稳定输入输出。
- 展开后执行语义不变。

### 2. 原语不隐藏复合算法

原语可以内部循环，例如一个振荡器一次生成一个采样，但不能把“整个钢琴”藏进一个节点。

判断标准：

```text
用户能否用更底层节点重新搭出同样的结果？
能  → 可以是宏/预设
不能 → 它是黑箱，需要拆分或记录为例外
```

### 3. 预设组属于文档数据

钢琴、弦乐、鼓组等预设保存为 `NodeGroup`，不是 Rust 里写死的 DSP。默认工程可以自带这些组，用户可以复制后修改。

### 4. 节点上限不能静默截断

节点计划的容量是编译器/引擎的实现细节，不是音乐语义。

- 超过容量必须返回明确错误。
- 不允许 `sample()` 静默返回 0。
- 后续把固定上限改为按计划分配的 scratch，音频线程仍然不分配。

### 5. 性能由编译期解决

用户写低层图，编译器负责：

- 常量折叠
- 公共子表达式消除
- 死代码删除
- 模式融合
- 组内联或子计划调用

不能因为“低层图节点多”就退回黑箱。

## 当前节点审计

| 节点 | 分类 | 问题 | 目标 |
| --- | --- | --- | --- |
| `voice.partial` | 黑箱 | 正弦、非谐性、亮度、衰减全在内部 | `piano.partial` 已改为原语组，`piano.voice` 已引用 |
| `voice.attack` / `voice.decay` | 过粗原语 | 只有单段指数 | `env.adsr` 已补，旧节点保留兼容 |
| `audio.instrument.sine` | 黑箱 | 固定正弦源，无相位/波表 | 默认 sine 轨改用 `sine.voice` 组；旧节点保留兼容 |
| `audio.adsr` | 过粗原语 | 固定 ADSR 阶段 | 由包络原语组成预设组 |
| `audio.filter.lowpass` / `highpass` | 过粗原语 | 固定类型 | `filter.biquad.lowpass` / `highpass` 已补 |
| `voice.repeat` / `voice.group` | 宏 | 编译期内联，不能独立编译 | 保留语义，支持展开和子计划 |
| `voice.noise` / `white_noise` | 原语 | 可保留 | 补种子/颜色控制 |
| `voice.sample_hold` | 原语 | 可保留 | 明确状态语义 |
| `math.add` / `multiply` | 原语 | 已补 subtract/divide/pow/mod/floor/ceil/round/compare/exp/sqrt | 可继续补向量/位运算 |
| `audio.delay` | 原语 | 音频域有，Voice 域没有 | `delay.line` 已补，固定 4096 采样/音符 |

## 实施顺序

1. 补数学原语和节点上限错误，先让底层可表达。
2. 补 `osc.sine`、`env.adsr`、`filter.biquad`、`delay.line`、`buffer.sample`。
3. 用可打开的 `piano.partial` / `piano.string` / `piano.grand` 组替换 `voice.partial` 黑箱。
4. 组展开/子计划编译/动态槽位。
5. UI 进入组、展开、复制预设。
6. 文档和回归测试。

## 进展

- 第 1 步完成：数学原语已加入节点目录；Voice/Audio 计划上限提高到 256，超限在编译期返回 `NodeLimit`，不再静默输出 0。
- 第 2 步部分完成：`osc.sine`、`env.adsr`、`filter.biquad.lowpass/highpass`、`math.exp/sqrt`、`delay.line` 已加入；`buffer.sample` 未实现。
- 预设可复制：`group.duplicate` 和组节点侧栏的“复制为独立预设”入口已加入。
- `piano.partial` 已改用 7 路显式数值输入和原语图；默认 `piano.voice` 已通过 `voice.group` 引用它。
- 迁移中发现并修复嵌套组展开时输入索引未加偏移的问题；新增 `math.int_to_float`。
- `delay.line` 编译期为每个音符分配 4096 采样环形缓冲；状态槽为 128，约 4 MiB / delay 节点。
- 默认 sine 轨已改为 `sine.voice` 组：`note.separate → osc.sine → env.adsr → voice.vca`，可进入编辑；旧的 `audio.instrument.sine` 保留兼容。
- 新增 `track.source` 命令和节点侧栏“设为音源”入口；删除源后可以重新指定，传 null 清空。
- 数值输入的 Voice 组改为 `GroupCall` 子计划，按插槽传入父图的值；修复嵌套组把父索引误当子节点、导致 `piano.partial` 的 time 绑到内部常量的问题。
- 默认钢琴不再用 `voice.pitch` / `voice.velocity` 读取隐式上下文；`note.separate` 的 pitch/velocity 作为外部自由变量直接进入 `piano.partial`，不占 Repeat 携带端口。
- Repeat Input 现在提供 `iteration` 输出；旧的独立 `p-index` 已从默认图移除。
- Repeat 支持数值多输出、任意嵌套、自由变量和 Note 上下文端口；旧 `piano.voice` 图在打开时自动迁移。
- 当前节点目录有 55 个 Voice 节点、22 个 Audio 节点；具体清单以 `node.types` 为准。
- 新增 `buffer.import`：导入 16/24/32 位 PCM 或 32 位浮点 WAV，多声道下混为单声道。
- 新增 `buffer.sample` 节点和 `sampler.voice` 组；`track.create` 支持 `source: "sampler" | "wavetable"`，文件菜单“导入采样…”会创建采样轨。
- `buffer.sample` 的 `loop` 参数为 1 时按单周期循环，可作为波表使用。
- 多输出数值组改为 `GroupCall` 子计划加每输出 `GroupResult` 读取缓存，不再把所有输出映射到同一个槽。

## 剩余缺口

- `buffer.sample` 已支持工程 buffer、线性插值和循环；图形化 buffer 选择器尚未加入。
- `sine.voice` 已组化，但波表和采样尚未暴露。
- 节点计划仍是固定 256 上限，状态是固定 128 槽；动态 scratch 未实现。
- 带 Note 输入的组仍回退到内联展开。
- `group.duplicate` 只解决预设复制，不改变编译单元边界。

## 节点删除与组数据

- 节点是实例，组是数据块。删除节点只移除实例，不删除 `NodeGroup`。
- 删除组数据必须显式使用 `group.delete`；仍被引用时拒绝。
- 允许删除音源和输出节点。图暂时不合法时，播放/渲染返回明确错误，而不是阻止编辑。
- 删除音源节点时清空 `Track.source`；无源轨道明确按静音处理，不保留悬空指针。
- 这与 Blender 的“删节点不等于删 datablock”保持一致。

## 非目标

- 不要求所有节点都成为算术原语。
- 不要求预设组永远不被编译器融合。
- 不为了底层而牺牲音频线程的无分配约束。
