# 命令规范 v0.1

这份文档定义 Aurtrack 的第一条可运行竖切：7 条命令、统一的返回结构、寻址规则，以及 MCP 的映射方式。代码必须与本文一致。

## 信封

请求：

```json
{ "cmd": "track.create", "args": { "name": "bass" } }
```

成功响应：

```json
{ "ok": true, "data": { "...": "..." }, "warnings": [], "undo_token": "undo-1" }
```

失败响应：

```json
{ "ok": false, "error": { "code": "not_found", "message": "还没有工程，先执行 project.create" } }
```

- `ok` 永远存在。
- `warnings` 默认空数组。
- `undo_token` 只在可撤销命令成功后出现。
- 错误码固定为小写下划线：`invalid_args`、`not_found`、`unknown_command`、`nothing_to_undo`、`io_error`。

## 寻址

实体用稳定 ID，路径用斜杠分隔：

```text
/tracks/{track_id}
/tracks/{track_id}/devices/{device_id}
/tracks/{track_id}/devices/{device_id}/params/{param_name}
```

当前阶段只支持 `gain` 设备的 `gain` 参数。

## 命令

### 1. `project.create`

创建或替换当前工程。撤销会恢复旧工程。

| 参数 | 类型 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- | --- |
| `name` | string | 否 | `Untitled` | 工程名，不能为空 |
| `sample_rate` | integer | 否 | `48000` | 8000 到 192000 |

返回：

```json
{ "project": { "name": "demo", "sample_rate": 48000, "track_count": 0, "tracks": [] } }
```

### 2. `track.create`

| 参数 | 类型 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- | --- |
| `name` | string | 是 | | 轨道名，不能为空 |
| `source` | string | 否 | `silence` | `silence` 或 `sine` |
| `frequency` | number | 否 | `440` | 20 到 20000，且小于采样率的一半 |

返回：

```json
{ "track_id": "track-1", "path": "/tracks/track-1" }
```

### 3. `device.add`

| 参数 | 类型 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- | --- |
| `track_id` | string | 是 | | 目标轨道 |
| `kind` | string | 是 | | 当前只支持 `gain` |
| `params` | object | 否 | `{"gain":1.0}` | 当前只接受 `gain`，范围 0 到 4 |

返回：

```json
{ "device_id": "device-1", "path": "/tracks/track-1/devices/device-1" }
```

### 4. `param.set`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 参数路径 |
| `value` | number | 是 | 必须是有限数；`gain` 范围 0 到 4 |

返回：

```json
{ "path": "/tracks/track-1/devices/device-1/params/gain", "value": 0.5 }
```

### 5. `render.segment`

| 参数 | 类型 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- | --- |
| `start_seconds` | number | 否 | `0` | 不早于 0 |
| `duration_seconds` | number | 否 | `1` | 0.01 到 600 |
| `path` | string | 否 | `aurtrack-render.wav` | 输出文件路径 |

返回：

```json
{
  "path": "out.wav",
  "sample_rate": 48000,
  "frames": 4800,
  "duration_seconds": 0.1,
  "peak": 0.25,
  "rms": 0.176
}
```

渲染规则：

- 每条轨道按 `source` 生成信号，正弦幅度固定为 `0.25`；
- 轨道的所有 `gain` 设备依次相乘；
- 所有轨道求和后钳制到 `[-1, 1]`；
- 输出 16 位单声道 PCM WAV。

### 6. `describe`

无参数。返回机器可读的能力清单和当前工程摘要。

```json
{
  "version": "0.1.0",
  "commands": [
    {
      "id": "track.create",
      "summary": "新建一条轨道",
      "undo": "snapshot",
      "input_schema": { "type": "object", "properties": { "...": {} } }
    }
  ],
  "project": null
}
```

`project` 为 `null` 表示还没有工程。`input_schema` 与该命令的 MCP `inputSchema` 完全一致。

### 7. `undo`

| 参数 | 类型 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- | --- |
| `steps` | integer | 否 | `1` | 1 到 100 |

返回：

```json
{ "undone": ["param.set"] }
```

只撤销可撤销命令。`render.segment`、`describe`、`undo` 不产生快照。

## MCP 映射

工具名是命令 ID 把 `.` 换成 `_`：

| 命令 | 工具 |
| --- | --- |
| `project.create` | `project_create` |
| `track.create` | `track_create` |
| `device.add` | `device_add` |
| `param.set` | `param_set` |
| `render.segment` | `render_segment` |
| `describe` | `describe` |
| `undo` | `undo` |

- `tools/list` 的 `inputSchema` 直接来自命令注册表；
- `tools/call` 的 `arguments` 原样作为 `args`；
- 执行失败时返回 `content` + `isError: true`，不使用 JSON-RPC 错误；
- 协议版本：`2024-11-05`。

## 示例

`--json` 是一次性调用：每条命令在独立进程里执行，不保留工程状态。需要连续操作时用 `--stdin`，一个进程内共享同一个会话：

```sh
printf '%s\n' \
  '{"cmd":"project.create","args":{"name":"demo"}}' \
  '{"cmd":"track.create","args":{"name":"bass","source":"sine","frequency":110}}' \
  '{"cmd":"device.add","args":{"track_id":"track-1","kind":"gain"}}' \
  '{"cmd":"param.set","args":{"path":"/tracks/track-1/devices/device-1/params/gain","value":0.5}}' \
  '{"cmd":"render.segment","args":{"duration_seconds":5,"path":"demo.wav"}}' \
  | aurtrack --stdin
```

单条查询可以直接用 `--json`：

```sh
aurtrack --json '{"cmd":"describe","args":{}}'
```
