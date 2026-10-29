# 节点图规范 v0.5

这份文档定义 `aurtrack-graph` 的 DAG 节点图。已接入工程格式 v4；支持 fan-out、`mix` 和自由连线。

## 节点

节点有稳定 `id`、`kind` 和参数。ID 只允许字母、数字、`-`、`_`、`.`，最长 64 字节。节点可以带可选的 `position: [x, y]`，只用于编辑器布局，不参与音频编译。

| kind | 输入 | 输出 | 参数 |
| --- | --- | --- | --- |
| `instrument.sine` | 无 | `out` | `frequency` 20–20000，默认 440 |
| `instrument.silence` | 无 | `out` | 无 |
| `instrument.piano` | 无 | `out` | `partials` 1–16，`decay` 0.1–8，`brightness` 0–1，`hammer` 0–1 |
| `amp.adsr` | `in` | `out` | `attack` 0.001–10，`decay` 0–10，`sustain` 0–1，`release` 0.001–10 |
| `filter.lowpass` | `in` | `out` | `cutoff` 20–20000 Hz，`resonance` 0–0.95 |
| `gain` | `in` | `out` | `gain` 0–4，默认 1 |
| `mix` | `in1`, `in2` | `out` | 无；两个输入相加 |
| `plugin` | `in` | `out` | 无；`plugin` 字段保存 CLAP 路径、ID 和可选 state |
| `output` | `in` | 无 | 无 |

默认轨道链：

```text
instrument.sine -> amp.adsr -> gain -> output
```

空轨道链：

```text
instrument.silence -> output
```

轨道图里的钢琴是一个组节点：

```json
{
  "id": "instrument",
  "kind": "voice.group",
  "params": {},
  "group": "piano.voice"
}
```

节点组是工程级 datablock，和轨道图平级：

```json
{
  "node_groups": {
    "piano.voice": {
      "id": "piano.voice",
      "name": "Piano Voice",
      "interface": {
        "inputs": [
          { "id": "note", "name": "Note", "type": "note" }
        ],
        "outputs": [
          { "id": "audio", "name": "Audio", "type": "audio" }
        ]
      },
      "graph": {
        "nodes": {},
        "connections": []
      }
    }
  }
}
```

Piano Voice 的默认 Voice 图把外部接口收敛成一个 `Note`：

```text
group.input(note)
  └─ note.separate
       ├─ velocity ─→ vel-level.a
       ├─ index ───→ white-noise.w ──→ noise.seed
       └─ gate ────→ gate.gate

repeat("Piano Partial", count) ──→ osc-vca.in
env-attack × env-decay ──→ env-gain ──→ osc-vca.gain
osc-vca ──→ add.A

time ──→ env-attack.time / env-decay.time
time ──→ noise-env-attack.time / noise-env-decay.time / noise.time

noise ──→ noise-vca.in
noise-env-attack × noise-env-decay ──→ noise-env-gain
velocity × level × noise-env-gain ──→ noise-vca.gain
noise-vca ──→ add.B

add ──→ gate.in
gate ──→ out.audio
```

`Attack` 输出 `min(time / attack, 1)`，`Decay` 输出 `exp(-time / decay)`；两者显式相乘，不再有固定公式的包络宏节点。`VCA` 负责音频乘控制值，`Noise` 只输出噪声源。

`voice.repeat` 引用一个循环体组，重复执行 `count` 次。`count` 是一个 Float 输入 socket，可连接控制信号；未连接时使用节点自己的值，运行时限制在 1–64。节点标题固定为 Repeat，循环体组名单独显示。循环体必须有一个 `in` 音频输入和一个 `audio` 输出；`voice.iteration` 输出当前迭代序号。默认循环体 `piano.partial` 是：

```text
voice-in.in ──→ add.A
partial.audio ──→ add.B
add ──→ out.audio

pitch / velocity / time / iteration ──→ partial
```

这样 `Partials` 被拆成 `Repeat + Partial + 上下文节点`。

钢琴轨的 Audio 图默认把 `audio.note` 音符输入节点连到 `audio.group.note`。断开这条线即停止音符事件；断开 `gate.gate` 即静音输出。

`voice.add` 是纯加法；`voice.mix` 是带 `Factor` 的 Blender 式混合，输入顺序固定为 `Factor, A, B`。

组节点只引用组 ID；同一个组可以被多条轨道复用。

组内部可以再放组节点：

```json
{
  "id": "nested",
  "kind": "voice.group",
  "params": {},
  "group": "aux.voice"
}
```

嵌套组的端口来自被引用组的接口；编译器递归展开，循环引用会被拒绝。

Voice 节点实例用 `type_id` 引用注册表，端口和参数由 `node.types` 声明。节点可选保存 `label`、`color` 和 `width`，只影响编辑器显示。帧可选保存 `color`，名称和颜色都只影响编辑器显示。连线可选保存 `muted`；静音连线不参与编译、不占用输入端口，也不参与拓扑排序。轨道图使用 `AudioGraph`，同样以 `type_id` 引用 `AUDIO_NODE_TYPES`，节点不再保存固定 kind。

Audio 节点类型：`audio.group`、`audio.group.instance`、`audio.instrument.sine`、`audio.instrument.silence`、`audio.adsr`、`audio.plugin`、`audio.gain`、`audio.mix`、`audio.filter.lowpass`、`audio.filter.highpass`、`audio.drive`、`audio.delay`、`audio.value`、`audio.note`、`audio.output`。`audio.instrument.sine` 的频率是 `frequency` 输入 socket，可连接控制信号。

工程视图同时输出 `type_id` 和 UI 用的 `kind`；写入只认 `type_id`。当前 Voice 类型以 `node.types` 注册表为准，包含 `voice.repeat`、`voice.partial`、`voice.switch`、`voice.index_switch`、`voice.index_switch_float`、`voice.vector_add`、`voice.broadcast`、`voice.sample_hold`、`voice.sum`、`voice.sum_float`、`voice.iteration`、`voice.pitch`、`voice.velocity`、`voice.attack`、`voice.decay`、`voice.noise`、`voice.vca`、`voice.add`、`voice.mix`、`voice.gate`、`note.separate` 等原子节点。`voice.sum` 和 `voice.sum_float` 的 `in` 是多输入 socket，可接多条线；它们编译成把所有输入相加。`voice.index_switch` / `voice.index_switch_float` 按 `count` 动态生成端口。`voice.vector_add` 编译成三路独立加法，输出 X/Y/Z。`voice.sample_hold` 每个音符保存上一采样值。

组接口 socket 类型：`float`、`gate`、`audio`、`note`。`note` 表示逐音符事件，不参与 Audio 采样，也没有数值默认值。

Voice 节点的数值参数以输入 socket 暴露：未连接时显示并使用 socket default，连接后由连线覆盖。`value.float` 的 `value` 是节点属性，不是输入 socket。

插件节点必须带 `plugin` 配置：

```json
{
  "id": "device-1",
  "kind": "plugin",
  "params": {},
  "plugin": {
    "path": "D:/Plugins/effect.clap",
    "id": "com.example.effect",
    "state": "AAECAwQ="
  }
}
```

一个轨道最多一个插件节点；实时播放时按轨道分块处理，离线渲染仍走干声直通。

低通默认值：`cutoff = 1200`、`resonance = 0.2`。

ADSR 默认值：

```text
attack  0.01
decay   0.12
sustain 0.75
release 0.18
```

## 连接

连接从 `out` 到输入端口。限制：

- 输出端口可以 fan-out
- 每个输入端口最多一条连接；`mix` 的 `in1` 和 `in2` 各一条
- 恰好一个乐器节点、一个 `output` 节点
- 最多一个 `amp.adsr`，且必须在分支之前
- 图必须无环；未接到 `output` 的设备允许存在，但会被编译器忽略

## JSON

```json
{
  "nodes": {
    "instrument": {
      "id": "instrument",
      "kind": "instrument.sine",
      "params": { "frequency": 440.0 }
    },
    "envelope": {
      "id": "envelope",
      "kind": "amp.adsr",
      "params": { "attack": 0.01, "decay": 0.12, "sustain": 0.75, "release": 0.18 }
    },
    "gain": {
      "id": "gain",
      "kind": "gain",
      "params": { "gain": 1.0 }
    },
    "output": {
      "id": "output",
      "kind": "output",
      "params": {}
    }
  },
  "connections": [
    {
      "from": { "node": "instrument", "port": "out" },
      "to": { "node": "envelope", "port": "in" }
    },
    {
      "from": { "node": "envelope", "port": "out" },
      "to": { "node": "gain", "port": "in" }
    },
    {
      "from": { "node": "gain", "port": "out" },
      "to": { "node": "output", "port": "in" }
    }
  ]
}
```

## 编译结果

DAG 编译成按拓扑序排列的节点：

```text
source_frequency: Option<f64>
adsr:             Option<AdsrSettings>
nodes:            Vec<CompiledNode>
output:           usize
```

索引 0 固定是声部输入；后续节点是 `filter.lowpass`、`gain`、`mix`、`output`。`mix` 把两个输入相加；滤波器状态按节点保存，实时和离线共用同一拓扑序。
