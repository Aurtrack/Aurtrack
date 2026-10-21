# 命令规范 v1.2

这份文档定义 Aurtrack 的可运行命令面：34 条命令、统一的返回结构、寻址规则，以及 MCP 的映射方式。代码必须与本文一致。

工程格式 v4 用 music tick 存储位置（960 PPQ），轨道保存 `aurtrack-graph` 节点图。命令边界仍用秒，避免 UI 和 CLI 直接操作 tick。旧 v3 工程会被拒绝。

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

`devices` 是轨道节点图中设备节点的投影，节点 ID 即 `device_id`；当前支持 `gain` 和 `amp.adsr` 两种设备。

## 命令

### 1. `project.create`

创建或替换当前工程。撤销会恢复旧工程。

| 参数 | 类型 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- | --- |
| `name` | string | 否 | `Untitled` | 工程名，不能为空 |
| `sample_rate` | integer | 否 | `48000` | 8000 到 192000 |
| `tempo_bpm` | number | 否 | `120` | 20 到 300 |
| `time_signature` | object | 否 | `4/4` | `{ "numerator": 4, "denominator": 4 }` |
| `length_bars` | integer | 否 | `4` | 1 到 512 |

返回：

```json
{ "project": { "name": "demo", "sample_rate": 48000, "tempo_bpm": 120, "time_signature": { "numerator": 4, "denominator": 4 }, "length_bars": 4, "length_seconds": 8.0, "track_count": 0, "tracks": [] } }
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
| `kind` | string | 是 | | `gain` 或 `amp.adsr` |
| `params` | object | 否 | | `gain` 范围 0 到 4；ADSR 参数见下表 |

返回：

```json
{ "device_id": "device-1", "path": "/tracks/track-1/devices/device-1" }
```

`amp.adsr` 默认参数：

| 参数 | 默认 | 范围 |
| --- | --- | --- |
| `attack` | `0.01` | 0.001 到 10 秒 |
| `decay` | `0.12` | 0 到 10 秒 |
| `sustain` | `0.75` | 0 到 1 |
| `release` | `0.18` | 0.001 到 10 秒 |

### 4. `param.set`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 参数路径 |
| `value` | number | 是 | `gain` 范围 0 到 4；`amp.adsr` 参数见上表 |

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

- 每条轨道先用 `aurtrack-graph` 编译节点链，得到信号源、ADSR 和总增益；
- 正弦幅度固定为 `0.25`；
- 链上的 `gain` 节点依次相乘；
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

## 走带命令

走带命令由 `aurtrack-runtime` 执行：它组合会话、实时引擎与音频输出。音频设备在第一次 `transport.play` 时惰性打开。

### 8. `transport.play`

无参数。要求已有工程。返回：

```json
{ "playing": true, "sample_rate": 48000 }
```

### 9. `transport.stop`

无参数。返回：

```json
{ "playing": false }
```

### 10. `transport.seek`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `seconds` | number | 是 | 目标位置，按工程长度取模 |

返回：

```json
{ "position": 2.5 }
```

### 11. `transport.state`

无参数。返回：

```json
{ "playing": false, "position": 0.0, "sample_rate": 48000 }
```

### 12. `transport.gain`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `gain` | number | 是 | 0 到 4 的实时音量倍数 |

返回：

```json
{ "gain": 0.5 }
```

走带命令都不产生撤销快照。工程修改不会自动重建正在播放的计划；改完工程后重新 `transport.play`。

## 会话视图命令

### 13. `redo`

| 参数 | 类型 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- | --- |
| `steps` | integer | 否 | `1` | 1 到 100 |

返回：

```json
{ "redone": ["track.remove"] }
```

`undo` 与 `redo` 共享一条历史栈；新的可撤销命令会清空 redo 栈。

### 14. `project.state`

无参数。返回完整工程，包括每条轨道的节点图、设备投影与参数：

```json
{
  "project": {
    "name": "demo",
    "sample_rate": 48000,
    "tracks": [
      {
        "id": "track-1",
        "name": "bass",
        "source": { "kind": "sine", "frequency": 110.0 },
        "devices": [
          { "id": "device-1", "kind": "gain", "params": { "gain": 0.5 } }
        ],
        "clips": []
      }
    ]
  }
}
```

`source` 和 `devices` 是 `graph` 的投影，节点 ID 就是设备 ID。没有工程时 `project` 为 `null`。这是 UI 轮询同步与会话视图的主要读取命令。

### 15. `track.remove`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `track_id` | string | 是 | 要删除的轨道 ID |

返回：

```json
{ "track_id": "track-1" }
```

### 16. `device.remove`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | `/tracks/{id}/devices/{id}` |

返回：

```json
{ "path": "/tracks/track-1/devices/device-1" }
```

`track.remove` 与 `device.remove` 都可撤销。

## 工程文件

工程文件格式见 `docs/adr/0002-project-file.md`、`docs/adr/0003-clip-note.md` 与 `specs/graph.md`：UTF-8 JSON，当前 `format_version: 4`；只接受当前版本，旧文件直接拒绝，不做兼容迁移。

### 17. `project.save`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 目标路径，建议 `.aurtrack.json` |

返回：

```json
{ "path": "...", "bytes": 1234 }
```

保存不产生撤销快照；父目录不存在时会创建。保存统一写入 `format_version: 4`。

### 18. `project.open`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 工程文件路径 |

返回：

```json
{ "path": "...", "project": { ... } }
```

打开会校验 `format_version`、采样率、ID 命名与唯一性；成功后才替换当前工程，可撤销。版本不匹配直接拒绝。

## 片段与音符

工程长度由 `length_bars`、拍号和速度决定；命令边界仍以秒为单位，内部按 960 PPQ 的 tick 存储。音符 `start` 相对片段起点。

### 19. `clip.create`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `track_id` | string | 是 | 目标轨道 |
| `name` | string | 否 | 省略时自动生成 |
| `start` | number | 是 | 片段起点，单位秒，必须在工程长度内 |
| `duration` | number | 是 | 片段长度，单位秒，终点不能超过工程长度 |

返回：

```json
{ "track_id": "track-1", "clip_id": "clip-1", "path": "/tracks/track-1/clips/clip-1", "clip": { } }
```

### 20. `clip.remove`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | `/tracks/{id}/clips/{id}` |

返回：

```json
{ "path": "/tracks/track-1/clips/clip-1" }
```

### 21. `note.add`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 片段路径 |
| `pitch` | integer | 是 | MIDI 音高，0 到 127 |
| `start` | number | 是 | 相对片段起点，0 到片段长度 |
| `duration` | number | 是 | 音符长度，必须落在片段内 |
| `velocity` | number | 否 | 0 到 1，默认 `0.8` |

返回：

```json
{ "path": "/tracks/track-1/clips/clip-1/notes/note-1", "note_id": "note-1", "note": { } }
```

### 22. `note.remove`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | `/tracks/{id}/clips/{id}/notes/{id}` |

返回：

```json
{ "path": "/tracks/track-1/clips/clip-1/notes/note-1" }
```

片段与音符命令都可撤销。工程文件版本 2 起包含 `clips` 与 `notes`；当前不做旧版本兼容，旧文件直接拒绝。

### 23. `clip.update`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 片段路径 |
| `name` | string | 否 | 新名称 |
| `start` | number | 否 | 新起点 |
| `duration` | number | 否 | 新长度 |

至少提供一个字段。缩片段时如果有音符超出新长度，命令直接拒绝。

返回：

```json
{ "path": "/tracks/track-1/clips/clip-1", "clip": { } }
```

### 24. `note.update`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 音符路径 |
| `pitch` | integer | 否 | 新 MIDI 音高 |
| `start` | number | 否 | 新相对起点 |
| `duration` | number | 否 | 新长度 |
| `velocity` | number | 否 | 新力度 |

至少提供一个字段；更新后必须仍落在片段内。

返回：

```json
{ "path": "/tracks/track-1/clips/clip-1/notes/note-1", "note": { } }
```

### 25. `clip.duplicate`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | `/tracks/{id}/clips/{id}` |

在原片段后面复制一份新片段，新起点为原片段终点；如果超出工程长度则拒绝。片段里的音符会分配新的 `note-*` ID。

返回：

```json
{ "track_id": "track-1", "clip_id": "clip-2", "path": "/tracks/track-1/clips/clip-2", "clip": { } }
```

### 26. `note.duplicate`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | `/tracks/{id}/clips/{id}/notes/{id}` |

在原音符后面复制一份新音符，新起点为原音符终点；超出片段长度则拒绝。

返回：

```json
{ "path": "/tracks/track-1/clips/clip-1/notes/note-2", "note_id": "note-2", "note": { } }
```

### 27. `note.quantize`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | `/tracks/{id}/clips/{id}/notes/{id}` |
| `grid` | number | 是 | 量化网格，单位秒，必须大于 0 且落在工程长度内 |

把音符起点吸附到最近的网格点，并夹在片段范围内；音符长度和力度不变。`grid: 0.125` 表示 1/16。

返回：

```json
{ "path": "/tracks/track-1/clips/clip-1/notes/note-1", "note": { }, "grid": 0.125 }
```

### 28. `note.paste`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | 目标片段路径 |
| `notes` | array | 是 | 音符草稿数组，至少 1 个 |
| `notes[].pitch` | integer | 是 | MIDI 音高 |
| `notes[].start` | number | 是 | 相对片段起点 |
| `notes[].duration` | number | 是 | 音符长度 |
| `notes[].velocity` | number | 否 | 0 到 1，默认 `0.8` |

一次批量写入多个音符，只产生一个撤销快照。所有音符先统一校验，任意一个非法则整批拒绝。

返回：

```json
{ "path": "/tracks/track-1/clips/clip-1", "note_ids": ["note-1"], "notes": [ ] }
```

### 29. `clip.paste`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `track_id` | string | 是 | 目标轨道 |
| `name` | string | 否 | 片段名，省略时自动生成 |
| `start` | number | 是 | 片段起点，单位秒，必须在工程长度内 |
| `duration` | number | 是 | 片段长度，单位秒，终点不能超过工程长度 |
| `notes` | array | 是 | 音符草稿数组，可为空 |

一次批量写入片段和音符，只产生一个撤销快照。

返回：

```json
{ "track_id": "track-1", "clip_id": "clip-2", "path": "/tracks/track-1/clips/clip-2", "clip": { } }
```

### 30. `note.remove_many`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `paths` | array | 是 | 音符路径数组，1 到 512 个，不能重复 |

先校验全部路径；任意一个不存在就整批拒绝，不产生部分删除。全部存在时只产生一个撤销快照。

返回：

```json
{ "paths": ["/tracks/track-1/clips/clip-1/notes/note-1"], "removed": 1 }
```

### 31. `note.quantize_many`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `paths` | array | 是 | 音符路径数组，1 到 512 个，不能重复 |
| `grid` | number | 是 | 量化网格，单位秒，必须大于 0 且落在工程长度内 |

先校验全部路径，再按与 `note.quantize` 相同的规则计算新起点；任意一个不存在就整批拒绝。只产生一个撤销快照。

返回：

```json
{ "paths": ["/tracks/track-1/clips/clip-1/notes/note-1"], "grid": 0.125, "count": 1 }
```

### 32. `clip.move`

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `path` | string | 是 | `/tracks/{id}/clips/{id}` |
| `track_id` | string | 是 | 目标轨道 ID |
| `start` | number | 否 | 新起点；省略时保持原起点 |

把片段移到另一条轨道或新起点，片段 ID、名称、长度和全部音符保持不变。目标轨道必须存在，移动后片段仍须落在工程长度内。跨轨移动只产生一个撤销快照。

返回：

```json
{ "path": "/tracks/track-2/clips/clip-1", "track_id": "track-2", "clip": { } }
```

### 33. `project.update`

修改速度、拍号、工程长度或循环区间，至少提供一个字段。改变拍号或长度后，现有片段必须仍落在工程长度内，否则整条命令拒绝。循环区间使用工程秒数；起点必须早于终点，终点不能超过工程长度。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `tempo_bpm` | number | 否 | 20 到 300 |
| `time_signature` | object | 否 | `{ "numerator": 1..32, "denominator": 1/2/4/8/16/32 }` |
| `length_bars` | integer | 否 | 1 到 512 |
| `loop_start` | number | 否 | 循环起点，单位秒 |
| `loop_end` | number | 否 | 循环终点，单位秒 |

返回：

```json
{ "project": { "tempo_bpm": 90, "time_signature": { "numerator": 3, "denominator": 4 }, "length_bars": 8, "loop_start": 0.0, "loop_end": 16.0 } }
```

### 34. `transport.metronome`

开关实时节拍器。节拍器不是工程数据，不进入撤销栈；播放中切换会重建播放计划。离线渲染不包含节拍器。

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `enabled` | boolean | 是 | 是否启用 |

返回：

```json
{ "metronome": true }
```

## MCP 映射

工具名是命令 ID 把 `.` 换成 `_`：

| 命令 | 工具 |
| --- | --- |
| `project.create` | `project_create` |
| `project.update` | `project_update` |
| `track.create` | `track_create` |
| `device.add` | `device_add` |
| `param.set` | `param_set` |
| `render.segment` | `render_segment` |
| `transport.play` | `transport_play` |
| `transport.stop` | `transport_stop` |
| `transport.seek` | `transport_seek` |
| `transport.state` | `transport_state` |
| `transport.gain` | `transport_gain` |
| `transport.metronome` | `transport_metronome` |
| `describe` | `describe` |
| `undo` | `undo` |
| `redo` | `redo` |
| `project.state` | `project_state` |
| `project.save` | `project_save` |
| `project.open` | `project_open` |
| `track.remove` | `track_remove` |
| `clip.create` | `clip_create` |
| `clip.update` | `clip_update` |
| `clip.remove` | `clip_remove` |
| `clip.duplicate` | `clip_duplicate` |
| `note.add` | `note_add` |
| `note.update` | `note_update` |
| `note.remove` | `note_remove` |
| `note.duplicate` | `note_duplicate` |
| `note.quantize` | `note_quantize` |
| `note.remove_many` | `note_remove_many` |
| `note.quantize_many` | `note_quantize_many` |
| `note.paste` | `note_paste` |
| `clip.paste` | `clip_paste` |
| `clip.move` | `clip_move` |
| `device.remove` | `device_remove` |

- `tools/list` 的 `inputSchema` 直接来自命令注册表；
- `tools/call` 的 `arguments` 原样作为 `args`；
- 执行失败时返回 `content` + `isError: true`，不使用 JSON-RPC 错误；
- 协议版本：`2024-11-05`。

## 会话共享

桌面宿主启动时会绑定 `127.0.0.1` 的随机端口，并把端口写到系统缓存目录下的 `Aurtrack/aurtrack-session.json`。该路径使用系统 known folder，不读取 shell 的 `TEMP`，因此在 Git Bash / WSL 等 POSIX 风格环境下也能稳定定位。

- CLI 与 MCP 默认先连接这个服务；连接成功就共享桌面的文档与走带；
- CLI 连接失败会打印警告和会话文件路径，然后退回本地一次性运行时；
- MCP 连接失败会直接退出，不再静默退回本地；必须显式加 `--local` 才使用本地运行时；
- `AURTRACK_SESSION_FILE` 可显式指定会话文件路径（建议绝对路径）；
- 服务只监听回环地址，暂未做鉴权。

因此桌面运行时，`aurtrack transport.play` 控制的是桌面那个 Runtime，而不是再起一个。

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

实时播放需要进程保持存活，`--hold` 会在读完 stdin 后继续运行，按 Ctrl+C 退出：

```sh
printf '%s\n' \
  '{"cmd":"project.create","args":{"name":"demo"}}' \
  '{"cmd":"track.create","args":{"name":"bass","source":"sine","frequency":110}}' \
  '{"cmd":"transport.play","args":{}}' \
  | aurtrack --stdin --hold
```
