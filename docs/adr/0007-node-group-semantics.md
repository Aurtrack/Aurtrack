# ADR 0007：节点组语义与 Blender 对齐

- **状态：** 已接受
- **日期：** 2026-10-27
- **决策者：** 项目维护者

## 背景

节点系统里有两个容易混淆的概念：

- `audio.group`：轨道 Audio 图里的音色乐器节点，引用一个 Voice 组，为音符轨提供音色。
- 组实例：引用组数据块，外部端口由组接口动态生成的节点。

早期只有 Voice 域有真正的组实例，轨道里的 `audio.group` 却显示成“节点组”，用户会期待它像 Blender Group 一样暴露接口输入。但它的 Pitch/Velocity/Gate 是音符事件，不是 Audio 信号。

## 决策

### 1. Voice 组和 Audio 组是各自域的数据块

- `NodeGroup` 保存 `VoiceGraph`，`AudioGroup` 保存 `AudioGraph`。
- 两者都有 `NodeGroupInterface`，但接口只能在各自域内生成端口。
- 工程保存 `node_groups` 和 `audio_groups` 两张表。
- 新增真正的 `Note` socket 类型，表示逐音符事件数据，不伪装成 Audio 信号，也不把 Note 当 Float 常量。

### 2. 乐器节点和组实例彻底分开

- 轨道里的 `audio.group` 是**音色乐器**，只引用 Voice 组，默认接口只有 `Note → Audio`。
- 钢琴轨的 Audio 图默认有一个 `audio.note` 音符输入节点，连到乐器的 `Note` 端口；断开这条线就没有音符事件。
- Voice 组内部的 `Group Input` 输出 `Note`，由 `Separate Note` 节点拆成 `pitch`、`velocity`、`gate`、`index`，再进入振荡器、包络等节点。
- `voice.add` 是纯加法，`A + B`；`voice.mix` 是 Blender 式混合，`Factor` 在第一个输入，按 `A*(1-Factor)+B*Factor` 插值。
- `voice.gate` 有显式 `gate` 输入；默认钢琴图用 `Add → Gate → Output`，断开 gate 后输出为 0。
- `voice.time` 输出逐音符时间；`Separate Note` 的 `index` 输出经 `voice.white_noise` 的 1D `W` 输入转换成确定性种子。
- 包络和噪声从 `time`、`seed` 输入读取，不再直接读 `VoiceContext`。
- `voice.attack` 与 `voice.decay` 是两个独立控制曲线，默认通过 `math.multiply` 相乘；不再使用固定公式的包络宏节点。`voice.vca` 负责音频乘控制值；`voice.noise` 只输出噪声源，不再内置包络和电平。
- `voice.repeat` 是 Repeat Zone：引用一个循环体组，按 `count` 重复执行；循环体有一个音频累加器输入 `in` 和一个音频输出 `audio`，内部用 `voice.iteration` 获取迭代序号。
- `voice.partial` 是单个泛音的原子节点；默认 `Partials` 由 `Repeat("piano.partial") + Partial` 组成，不再有单体 `osc.partials` 节点。
- 节点可以有显示用的 `label`、`color` 和 `width`；帧可以有 `color`；连线可以有 `muted`。这些字段不参与 DSP。静音连线在编译时等价于不存在，输入回退到 socket 默认值。
- 组输入 socket 未连接时使用接口的 `default`，不再固定为 0 或偷父级上下文。
- 多输入 socket 允许在同一个目标端口上保留多条连线；`voice.sum` / `voice.sum_float` 用它做音频或控制求和。普通输入仍然只允许一条有效连线。
- `voice.index_switch` 和 `voice.index_switch_float` 的输入槽数量来自 `count` 参数，编译器和 UI 都从参数推导端口。
- `voice.vector_add` 用三路独立 Float 输出模拟向量，避免在引擎引入打包向量。
- `voice.sample_hold` 是显式状态节点：每个音符保存上一采样值，状态用原子存储，音频线程不分配不加锁。
- `voice.broadcast` 显式把 Float 控制值当作逐采样音频使用，避免隐式类型转换。
- 节点预览通过 `voice.preview` 编译当前组并采样目标节点，只用于 UI，不写入工程。
- 嵌套 Voice 组的 Note 输入未接线时绑定为禁用，不会从父级上下文偷音符。
- 轨道里的 `audio.group.instance` 是**Audio 组实例**，端口由 `AudioGroup.interface` 动态生成。
- Voice 图里的 `voice.group` 继续是真正的 Voice 组实例。

### 3. Audio 组实例由编译器展开

`AudioGraph` 编译为 `GraphPlan` 时递归处理 `audio.group.instance`：

- 组实例输入绑定到子图 `audio.group.input`。
- 子图 `audio.group.output` 映射回父图。
- 无效果节点的直通组把父图输入直接传给输出。
- 子计划合并时区分父图已有索引和子图新增索引，避免空组偏移错误。
- 组循环引用、缺失组和边界端口错误在校验阶段拒绝。

### 4. 组接口具备 Blender 式字段

`NodeGroupSocket` 支持：

- `default`：未连接输入默认值；只对 Float/Gate 有效。
- `minimum` / `maximum` / `step`：Float 范围与步进。
- `panel`：接口在组节点上的面板归属。
- `description`：说明文本。
- `Note` socket 只携带音符事件，没有数值默认值、范围或步进。

组接口编辑器可以新增/删除端口、修改名称、类型、默认值、范围、步进、面板和说明。未连接的 Float/Gate 接口在组实例上显示该实例的端口值控件（socket value），连接后隐藏；Note 接口只显示端口，不显示数值控件。

### 5. 编辑器和导航统一

- 双击组标题或点击 `↗` 进入组。
- Audio 组编辑器有面包屑和关闭按钮；支持嵌套组。
- 组编辑器复用节点画布的连线、选择、移动、帧、参数和快捷键。
- 组输入/输出边界节点可移动，但不可删除或复制。
- Audio 组图必须恰好有一个组输入和一个组输出。
- 轨道图在组编辑器打开时停用键盘，避免两个画布同时响应。

### 6. 打包成组

`audio_group.from_nodes` 把选中的 Audio 效果/控制节点打包：

- 从跨边界连线推导组输入/输出接口。
- 子图保留内部连线。
- 父图用组实例替换选中节点并重接外部连线。
- 轨道图 `Ctrl+G` 触发打包。

## 后果

- Audio 和 Voice 各有正确的域模型和编译器，不再互相伪装。
- 轨道中的音色乐器和 Audio 组实例在数据和 UI 上都能区分。
- 组接口默认值、范围、面板和说明可编辑并参与编译。
- 音色乐器只暴露一个 `Note` 输入，逐音符拆分在 Voice 组内部完成。
- Note 源、Note 连线和 Gate 都是显式连线，拔掉后引擎立即停止对应事件/声音。
- Audio 组可嵌套、可展开、可保存、可撤销。

## 剩余限制

- Voice 组接口还没有 Audio 组这样的可视化编辑器面板（字段已支持，UI 仍是内嵌表单）。
- 组编辑器里的嵌套打包还没有接 `Ctrl+G`。
- `audio.note` 目前只代表当前轨道的钢琴卷帘；跨轨道或外部 MIDI Note 源还没有实现。
- Audio 域的 ADSR 参数仍是节点属性；它作用于轨道级包络，不是当前 Voice 图的 socket。
- Voice/Audio 共享通用 `NodeTree` 容器，端口推导、校验和编译仍按域分别实现。
