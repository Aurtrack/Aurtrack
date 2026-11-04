# 0011 物理钢琴改为节点化 partial 银行

## 状态

已接受。

## 背景

`piano.physical` 之前由 `string.waveguide` 承担音色，引擎内部还硬编码了 18 个模态 partial。结果是：

- 节点图上看不到 partial；
- 调音色必须改 Rust；
- 波导的拨弦味无法通过节点替换；
- “物理钢琴”和“节点系统”是两套模型。

## 决定

1. 新增 `math.sin`、`math.abs` 两个 Voice 数学原语。
2. `piano.partial` 组加入显式击弦位置梳：
   - `sin(pi * n * position)`；
   - 用 `math.abs` 和 `math.clamp` 取最小 0.08，避免节点完全静音；
   - `position` 是组内可编辑的 `value.float`。
3. `piano.partial` 加入 prompt / sustain 两段包络：
   - prompt 时间 0.08 秒；
   - prompt 权重 0.35，sustain 权重 0.65；
   - 两段在组内用 `math.exp`、`math.multiply`、`math.add` 相加。
4. `piano.physical` 默认图改为：
   - 12 个 `voice.piano_partial` 节点，通过 `voice.zone.repeat` 迭代；
   - 每个节点内部完成三弦、击弦位置、prompt/sustain 和按音高补偿的衰减；
   - `hammer.exciter` 作为瞬态直通；
   - `damper` 接收 gate、pedal、release；
   - 不再使用 `string.waveguide`。
5. `string.waveguide` 回到纯波导：移除引擎内硬编码的 18 partial 模态层。
6. 新增 `audio.resonator` 音频原语：输入 audio，参数 `frequency`、`q`、`gain`。
7. `track.create` 的物理钢琴不再插入 `audio.soundboard`，而是插入 8 个 `audio.resonator`、一个干声 gain 和两级 mix。
8. 旧工程打开时：
   - 内置 `piano.physical` 仍是波导版，或 `piano.partial` 没有 `math.sin`，替换为新的内置默认组；
   - 轨道里的旧 `audio.soundboard` 替换为谐振器音板网络。

## 后果

- 钢琴音色现在可以在节点图里改：partial 数量、index 曲线、inharmonicity、brightness、decay、position 和弦数。
- 16 个 `piano.partial` 组实例在 4 音和弦时已经低于实时；改成 12 个 `voice.piano_partial` 节点后，8 音和弦的 2 秒离线渲染约 0.9 秒（含启动），留出实时余量。
- 音板现在是每轨 8 个 `audio.resonator` 的网络；跨轨共享音板尚未保留。
- 双弦/三弦失谐已在一个 partial 节点内部完成，不再展开成组图；`piano.partial` 组仍保留给需要完全展开的用户。

## 验证

- `physical_piano_track_renders` 检查物理组没有 `string.waveguide`、包含 `voice.piano_partial`，`piano.partial` 有 `math.sin`，轨道有 8 个 `audio.resonator`。
- C4 渲染峰值约 0.09；基频 440 Hz，泛音延伸到 2.6 kHz 以上。
- 2 秒离线渲染：1 音约 0.42 秒、8 音约 0.90 秒、12 音约 1.36 秒；这是 CLI 进程总耗时，包含工程创建和启动开销。
