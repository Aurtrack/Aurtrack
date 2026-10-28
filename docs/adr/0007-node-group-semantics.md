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
- Voice 组内部的 `Group Input` 输出 `Note`，由 `Separate Note` 节点拆成 `pitch`、`velocity`、`gate`，再进入振荡器、包络等节点。
- `voice.add` 是纯加法，`A + B`；`voice.mix` 是 Blender 式混合，`Factor` 在第一个输入，按 `A*(1-Factor)+B*Factor` 插值。
- `voice.gate` 有显式 `gate` 输入；默认钢琴图用 `Add → Gate → Output`，断开 gate 后输出为 0。
- `voice.time` 输出逐音符时间；`voice.note_index` 输出音符序号，经 `voice.white_noise` 的 1D `W` 输入转换成确定性种子。
- 包络和噪声从 `time`、`seed` 输入读取，不再直接读 `VoiceContext`。
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
