# Blender 节点编辑器对标

本文把 Blender 的节点编辑器拆成可执行的能力面，记录 Aurtrack 的对齐状态。
目标不是复制 Geometry Nodes 的几何类型，而是复制它的**操作语义**：

- 节点是数据块，连线只表达数据流。
- 一个输入最多接一条有效连线；输出可以 fan-out。
- 组是节点的容器，也是可复用接口。
- 循环是显式的 Zone，不是隐藏的求值上下文。
- 所有编辑都通过命令和稳定 ID 表达，UI 不持有第二套真相。

参考资料：

- [Blender Manual: Editing Nodes](https://docs.blender.org/manual/en/latest/interface/controls/nodes/editing.html)
- [Blender Manual: Repeat Zone](https://docs.blender.org/manual/en/4.5/modeling/geometry_nodes/utilities/repeat_zone.html)
- [Blender Manual: Node Editor](https://docs.blender.org/manual/en/latest/interface/controls/nodes/index.html)

## 状态

| 能力 | Blender | Aurtrack | 状态 |
| --- | --- | --- | --- |
| 节点图 | Shader / Geometry / Compositor | Voice / Audio | 已对齐 |
| 节点组 | Group Input/Output、嵌套、复用 | `voice.group`、组接口、嵌套组 | 已对齐 |
| 组接口 | 输入/输出 socket、默认值、面板 | `NodeGroupSocket` 默认值、`panel` | 已对齐 |
| 循环 Zone | Repeat Zone，输入/输出 bake items | 内联 Zone、多字段、可缩放/折叠、单音频累加器 | 部分 |
| 循环序号 | Index 节点 | `voice.iteration` | 已对齐 |
| 单泛音 | 由多个 Math/Var 节点搭建 | `voice.partial` 原子节点 | 已对齐 |
| 包络 | Math 节点组合 | `voice.attack` × `voice.decay` | 已对齐 |
| 连线 | 单输入、输出 fan-out、类型检查 | 同左 | 已对齐 |
| 连线静音 | `Ctrl+Alt+RMB`，红色虚线 | `Ctrl+Alt+RMB`，红色虚线 | 已对齐 |
| 连线切割 | `Ctrl+RMB` | `Ctrl+RMB` | 已对齐 |
| 拖线到空白 | 打开搜索并自动连接 | 打开添加菜单并自动连接 | 已对齐 |
| 拖节点到连线 | 自动插入 | `insertTarget` 高亮并插入 | 已对齐 |
| 节点标签 | `F2` / Properties > Label | `F2`、N 侧栏 | 已对齐 |
| 节点颜色 | Properties > Color | N 侧栏颜色 | 已对齐 |
| 帧 | `Ctrl+J`、拖动、缩放 | `Ctrl+J` / `Ctrl+P`、拖动、缩放 | 已对齐 |
| 帧标签/颜色 | 支持 | 双击改名、颜色选择器 | 已对齐 |
| 静音节点 | `M`，连线穿过 | `M`，编译时旁路 | 已对齐 |
| 折叠节点 | `H` | `H` | 已对齐 |
| 折叠未连接 socket | `Ctrl+H` | `Ctrl+H` | 已对齐 |
| 节点预览 | `Shift+H` | N 侧栏显示采样曲线 | 部分 |
| 节点缩放 | 拖左右边界，多选联动 | 右上角拖动，多选联动 | 已对齐 |
| Swap 节点 | `Shift+S`，按名称/类型重连 | `Shift+S`，按名称/类型重连 | 已对齐 |
| 删除并重连 | `Ctrl+Delete` | `Ctrl+Delete` | 已对齐 |
| Make Links | `J` / `Shift+J` | `J` / `Shift+J` | 已对齐 |
| Detach Links | `Alt+拖动` | `Alt+拖动节点` | 已对齐 |
| 交换连线 | `Alt+拖动连线` | `Alt+拖动输入端口` | 已对齐 |
| 复制/粘贴 | `Ctrl+C/V` | 已实现 | 已对齐 |
| 复制并保持组引用 | `Alt+D` | 普通复制 | 部分 |
| 框选/套索/圈选 | `B`、`Ctrl+Alt+LMB`、`C` | `Ctrl+Alt+LMB`、`C` | 已对齐 |
| 导航 | Home / F / 中键平移 / 滚轮 | 同左 | 已对齐 |
| N 侧栏 | 节点/组属性 | 已有 | 已对齐 |
| 搜索菜单 | `F3` | `F3` / `Shift+A` 共用添加菜单 | 已对齐 |
| 节点组进入/离开 | `Tab` / `Ctrl+Tab` | 同左 | 已对齐 |
| 数组 / 字段 | Geometry Nodes Field | Float 逐采样；`broadcast`、浮点 Index Switch；无通用数组 | 部分 |
| 多输入 socket | Join Geometry 等 | `voice.sum` 音频、`voice.sum_float` 控制多输入 | 已对齐 |
| 向量/颜色/几何 socket | 有 | `voice.vector_add` 六 Float 进、三 Float 出；无 Vector 单端口 | 部分 |
| Switch / Index Switch | 有 | 二选一 + 1–16 路动态 Index Switch | 已对齐 |
| Simulation Zone | 有 | `voice.sample_hold` 按音符保存上一采样；无多帧缓存 | 部分 |
| For Each / 并行 | 有 | 无 | 缺失 |

## 已完成的 P0

### 节点标签与颜色

- `VoiceNode.label`：覆盖节点标题，保留类型 ID。
- `VoiceNode.color`：节点标题底色，使用 Blender 式“节点颜色”而不是改类型色。
- `F2` 打开 N 侧栏并聚焦标签输入。
- 空标签回退到类型名。

### 连线静音

- `VoiceConnection.muted` 是显式字段，不是 UI 临时状态。
- 编译时忽略被静音连线，输入回退到 socket 默认值。
- 校验时静音连线不占用输入端口，因此可以保留旧线并重新接线。
- 拓扑排序忽略静音连线，避免旧线造成伪环。
- `Ctrl+Alt+RMB` 划过多条线切换静音；静音线红色虚线。

## 已完成的 P1

1. **帧标签与颜色**：双击改名、颜色选择器，写入 `VoiceFrame.color`。
2. **Ctrl+Delete 删除并重连**：按端口名称/类型匹配上下游。
3. **Shift+S Swap**：选中节点替换类型，按端口名称/类型重连。
4. **J / Shift+J Make Links**：按选中顺序连接第一个兼容端口。
5. **Alt+拖动 Detach Links**：拖动节点时断开外部连线。
6. **节点宽度调整**：右上角拖动，多选联动，写入 `VoiceNode.width`。
7. **Alt+拖动连线交换**：拖动已连接输入到另一个输入，交换两条连线的目标。

## 已完成的 P2

1. **Index Switch**：`voice.index_switch` 固定 8 路音频输入。
2. **节点组接口面板**：沿用 `NodeGroupSocket.panel` 分组。
3. **多输入 socket**：`voice.sum` 的音频输入允许多条连线，编译为加法。
4. **字段化参数**：Float 本身就是逐采样字段；新增 `voice.broadcast` 显式把控制值转成音频。
5. **F3 模糊评分**：精确、前缀、包含、子序列分级排序。

## 已完成的 P3

1. **控制域多输入**：`voice.sum_float` 使用 Float 多输入。
2. **动态 Index Switch**：音频和浮点版本都按 `count` 生成 1–16 个输入槽。
3. **向量多输出**：`voice.vector_add` 六路 Float 输入、X/Y/Z 三路输出。
4. **跨采样状态**：`voice.sample_hold` 按音符保存上一采样，使用原子状态，不分配不加锁。
5. **节点预览**：`voice.preview` 编译当前组并采样节点，N 侧栏显示曲线和范围。

## 下一批 P4

1. **真正的 Vector socket**：单端口携带 X/Y/Z，而不是三根线。
2. **通用数组/列表 socket** 与 For Each。
3. **完整 Simulation Zone**：多帧缓存、重置和预览。
4. **节点组接口面板 UI**：按 panel 折叠实例参数。
5. **预览游标**：点击曲线定位到具体采样时间。

## 不做的部分

- 不引入 Geometry、Object、Collection、Material 等 Blender 场景类型；Aurtrack 的数据域是音频、控制、Note。
- 不为了“像 Blender”而把 `Add`、`Mix`、`VCA` 合并成万能节点；音频语义优先。
- 不实现完整仿真缓存；当前只提供单采样状态节点。

## 判定标准

新增节点或交互必须同时满足：

- 能通过命令总线表达，脚本和 AI 可复现。
- 能序列化进工程 v4，稳定 ID 不丢失。
- 能通过 `cargo clippy`、`cargo test` 和 `pnpm check`。
- 不把 UI 概念带进引擎。
