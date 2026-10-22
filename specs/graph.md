# 节点图规范 v0.4

这份文档定义 `aurtrack-graph` 的 DAG 节点图。已接入工程格式 v4；支持 fan-out、`mix` 和自由连线。

## 节点

节点有稳定 `id`、`kind` 和参数。ID 只允许字母、数字、`-`、`_`、`.`，最长 64 字节。节点可以带可选的 `position: [x, y]`，只用于编辑器布局，不参与音频编译。

| kind | 输入 | 输出 | 参数 |
| --- | --- | --- | --- |
| `instrument.sine` | 无 | `out` | `frequency` 20–20000，默认 440 |
| `instrument.silence` | 无 | `out` | 无 |
| `amp.adsr` | `in` | `out` | `attack` 0.001–10，`decay` 0–10，`sustain` 0–1，`release` 0.001–10 |
| `filter.lowpass` | `in` | `out` | `cutoff` 20–20000 Hz，`resonance` 0–0.95 |
| `gain` | `in` | `out` | `gain` 0–4，默认 1 |
| `mix` | `in1`, `in2` | `out` | 无；两个输入相加 |
| `output` | `in` | 无 | 无 |

默认轨道链：

```text
instrument.sine -> amp.adsr -> gain -> output
```

空轨道链：

```text
instrument.silence -> output
```

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
- 图必须无环，且所有设备都在从乐器到 `output` 的路径上

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
