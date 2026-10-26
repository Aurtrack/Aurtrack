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
          { "id": "pitch", "name": "Pitch", "type": "float" },
          { "id": "velocity", "name": "Velocity", "type": "float" },
          { "id": "gate", "name": "Gate", "type": "gate" }
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

Voice 节点实例用 `type_id` 引用注册表，端口和参数由 `node.types` 声明。当前类型：`group.input`、`osc.partials`、`env.percussive`、`noise.burst`、`voice.mix`、`voice.group`、`group.output`、`math.add`、`math.multiply`、`math.map_range`、`math.clamp`、`value.float`。

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
