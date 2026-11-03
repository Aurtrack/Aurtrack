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
   - 16 个 `piano.partial` 组实例，通过 `voice.zone.repeat` 迭代；
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

- 钢琴音色现在可以在节点图里改：partial 数量、index 曲线、inharmonicity、brightness、decay、position、prompt 权重。
- 16 个 partial 的默认图明显比 10 partial 的旧钢琴更重，但仍在 Voice 计划上限内。
- 音板现在是每轨 8 个 `audio.resonator` 的网络；跨轨共享音板尚未保留。
- 双弦失谐在 `piano.partial` 内部；三弦/双极化仍未实现。

## 验证

- `physical_piano_track_renders` 检查物理组没有 `string.waveguide`、包含 `piano.partial`，`piano.partial` 有 `math.sin`，轨道有 8 个 `audio.resonator`。
- C4 渲染峰值约 0.09；基频 440 Hz，泛音延伸到 2.6 kHz 以上。
