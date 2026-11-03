# 0010 从输出反向收集活乐器，移除 Track.source

## 状态

已接受，取代 ADR 0008 中 `Track.source` 作为执行上下文的部分。

## 背景

`Track.source` 是一个文档字段，不是图端口。它让播放依赖一个与输出路径无关的指针：

```text
note-input → instrument A（source）
note-input → device B
B → output
```

此时 A 没接输出，却仍被当作音源；B 接了输出，却不出声。节点图本身已经表达了谁在输出路径上，`source` 是多余的第二事实来源。

## 决定

1. 编译音频图时，从 `output` 反向遍历整条路径。
2. 收集所有满足以下条件的节点：
   - 在输出路径上；
   - 是 `audio.group`；
   - 有来自 `audio.note` 的 `note` 输入；
   - 未静音。
3. 为每个活乐器编译一个 `VoicePlan`，按路径中的顺序分配 `GraphOp::Voice { index }`。
4. `GraphPlan` 持有 `Vec<PianoPlan>`，`sample` 接收 `&[f64]`；音频线程用固定数组保存 voice 值，不分配。
5. 删除 `track.source` 命令、UI 的“设为音源”入口和 `Track.source` 字段。
6. 旧工程 JSON 里的 `source` 字段由 serde 忽略，不再迁移成任何执行状态。
7. `sine.voice` 的连续正弦兜底改成 `GraphPlan.fallback_sine`，仍由显式节点组决定。

## 后果

- “接了输出但没设音源”不再可能。
- 多个乐器可以同时进入混音；`audio.mix` 增加四路音频输入。
- 不再存在“第一个匹配节点”推断执行上下文。
- 旧工程打开后，声音由实际输出路径决定，可能与旧 `source` 不一致；这是有意的行为修正。

## 测试

- `playback_collects_all_active_instruments`：两个乐器经 mix 进入输出，生成两个 Voice。
- 删除音源命令后，命令注册表数量为 76。
- `track_source_can_be_reassigned_and_cleared` 已删除。
