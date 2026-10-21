# 节点图规范 v0.1

这份文档定义 `aurtrack-graph` 的第一版线性节点图。当前只做骨架，尚未接入工程文件；工程格式接入时升到 v4。

## 节点

节点有稳定 `id`、`kind` 和参数。ID 只允许字母、数字、`-`、`_`、`.`，最长 64 字节。

| kind | 输入 | 输出 | 参数 |
| --- | --- | --- | --- |
| `instrument.sine` | 无 | `out` | `frequency` 20–20000，默认 440 |
| `amp.adsr` | `in` | `out` | `attack` 0.001–10，`decay` 0–10，`sustain` 0–1，`release` 0.001–10 |
| `gain` | `in` | `out` | `gain` 0–4，默认 1 |
| `output` | `in` | 无 | 无 |

默认轨道链：

```text
instrument.sine -> amp.adsr -> gain -> output
```

ADSR 默认值：

```text
attack  0.01
decay   0.12
sustain 0.75
release 0.18
```

## 连接

连接只能从 `out` 到 `in`。第一版限制：

- 每个输出端口最多一条连接
- 每个输入端口最多一条连接
- 恰好一个乐器节点、一个 `output` 节点
- 最多一个 `amp.adsr`
- 图必须无环，且所有节点都在主链上

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

线性图编译成：

```text
source_frequency: Option<f64>
adsr:             Option<AdsrSettings>
gain:             f64
```

`gain` 节点可以串联，编译时相乘；`adsr` 只允许一个；`instrument.sine` 的频率进入 `source_frequency`。
