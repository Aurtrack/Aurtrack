# ADR 0005：轨道节点图与工程格式 v4

- **状态：** 已接受
- **日期：** 2026-10-21
- **决策者：** 项目维护者

## 背景

v3 的轨道直接保存 `source` 和扁平的 `devices` 数组，`gain` 与 `amp.adsr` 靠设备类型被运行时单独解释。这个模型能跑，但不能表达设备顺序、连接和后续的节点图。

ADSR 已经验证了 Voice 域包络；下一步需要让轨道真正保存一张图。

## 决定

### 1. 数据模型

- `Track` 用 `graph: aurtrack-graph::Graph` 替代 `source` 与 `devices`。
- 图节点有稳定 ID、类型和参数；连接只从 `out` 到 `in`。
- 第一版只支持线性链，恰好一个乐器节点和一个 `output`，最多一个 `amp.adsr`。
- 乐器：`instrument.sine`、`instrument.silence`。
- 设备：`amp.adsr`、`gain`；设备节点 ID 继续使用 `device-{n}`。

### 2. 命令与 UI

- 现有 `device.add`、`device.remove`、`param.set` 继续保留，作为图设备节点的投影。
- `project.state` 同时返回 `graph`、`source` 和 `devices`；UI 暂时只读投影。
- 这样 v3 的命令面和 UI 不需要同时重写，节点图命令可以下一步再加。

### 3. 渲染

- 轨道先把图编译成 `source_frequency`、`adsr`、`gain`，再进入引擎。
- 实时播放和离线 WAV 共用同一个编译结果。
- 编译器不分配音频缓冲区，只在换计划时遍历一次图。

### 4. 工程格式

- `format_version` 升到 `4`；轨道保存 `graph`。
- 打开只接受 v4；v3 直接拒绝，不做迁移。
- 打开时校验图结构、参数、设备 ID 唯一性与采样率。

## 后果

- 轨道顺序、ADSR 和增益都由图表达，下一步可以加入分支和效果节点。
- 旧的 v3 工程会打不开；当前仍是 pre-alpha，维护者明确接受。
- 图命令、图 UI 和真正的效果节点不在本次范围内。
