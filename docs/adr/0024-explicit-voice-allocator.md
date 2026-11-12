# 0024 显式声部分配器

## 状态

已接受并实施；block 路径不再按全局 note index 取状态槽。

## 背景

`PianoPlan::sample_block` 之前用

```rust
notes.iter().enumerate().take(self.states.len())
```

遍历音符，`VoiceState` 按音符在整条轨道里的下标索引，容量固定为
`VOICE_STATE_SLOTS = 128`。这带来三个问题：

1. 一条轨道超过 128 个音符时，`notes.len() <= states.len()` 不成立，
   整个 block 内核被跳过，只剩逐采样回退；音符越多回退越贵，形成
   性能悬崖。
2. “同时发声数”没有显式上限，只由 128 个状态槽隐式决定；超过后没有
   确定的回收策略。
3. 音符下标与状态槽的绑定是隐式的：两个不同音符可能映射到同一个槽，
   状态容量和音乐语义（复音）被混为一谈。

## 根因

状态槽是“音符下标 % 状态容量”的隐式映射，而不是“活动声部”的显式池。
block 路径把“音符数不超过状态槽数”当作能否走内核的前提，于是状态容量
变成了音乐上的复音上限，并且一旦越界就整体退回逐采样。

## 设计

### 显式复音上限

- `PianoPlan` 增加 `polyphony` 字段，默认 `DEFAULT_POLYPHONY = 64`，
  上限 `MAX_POLYPHONY = 256`；槽池在计划构建期按上限预分配。
- 复音上限可通过命令注册表表达：`track.polyphony`
  （`{"track_id": "...", "polyphony": 4}` 设置，省略 `polyphony` 时查询），
  写到该轨道主乐器节点的 `polyphony` 参数；`track.polyphony` 是普通文档
  参数，不进入节点图端口求值，不改变端口语义。
- 构建播放计划时把该参数传给 `PianoPlan::set_polyphony`，缺省用引擎
  默认值；任何情况下都不根据 CPU 负载隐式减少声部。

### 声部槽与块内事件

- `VoiceSlot` 记录：音符下标、力度、起始时刻、gate 关闭时刻、分配序号、
  是否已渲染过。空闲槽 `note_index == NO_NOTE`。
- `sample_block` 改为事件驱动：
  1. 释放尾音已结束的槽位；
  2. 按 `(起始时刻, 音符下标)` 顺序取块内 note-on，在**精确帧**先把
     已有声部推进到该帧，再分配新声部；
  3. 块末推进剩余帧；
  4. 释放本块内结束的尾音。
- 尾音继续占槽直到结束；释放后的槽位直接复用，不走抢占。
- 块不连续（首次调用、seek 后恢复、静音段之后）时按块边界重建分配表，
  把窗口覆盖当前块的音符补分配，保证中途进入播放位置仍与逐采样路径
  一致；连续块用 `(起始时刻, 音符下标)` 游标保证同一音符只分配一次，
  块末帧的起音顺延到下一块，不会因块边界浮点舍入丢失或重复。

### 抢占规则

复音上限用尽时按固定顺序回收，逐级降级：

1. 空闲槽；
2. 已释放最旧：gate 已关闭的声部中释放时刻最早者（同时刻比分配序号）；
3. 最轻：力度最小者（同力度比起始时刻，再比分配序号）；
4. 最旧：起始时刻最早者（同时刻比分配序号）。

被抢占的槽位在分配时重置 `VoiceState`，旧包络不会污染新音符。

### 尾音截断

可证明的制音器尾音会被跳过：当声部输出经 `Output`/`IntToFloat` 直通后
就是 `Damper`、gate 直接来自上下文、pedal 常量为零、damping 为正常量
时，电平在 `1 / damping` 个采样后精确为零。block 段边界收紧到该点
（仍以 0.5 秒封顶），只跳过输出必然为零的帧；无法证明的声部保留
0.5 秒保守窗口。

### 可观测性

`PianoPlan::voice_stats()` 返回 `VoiceStats`：

- `active_voices`：当前占用的槽位数；
- `voice_starts`：分配过的 note-on 总数（含后来被抢占的）；
- `voice_steals`：抢占次数；
- `voice_drops`：尚未渲染任何一帧就被抢占的音符数（正常密度下为 0）。

`Playback::plan()` 提供只读计划快照，控制线程与测试通过它读取统计。
统计按块发布到 Relaxed 原子快照，控制线程可随时读取，最多落后一个
block；热路径仍是零分配、零锁、零日志。

## 测试

引擎内新增回归：

- `piano_block_processes_more_notes_than_voice_state_slots`：300 个顺序
  音符全部进入分配，无整块回退、无丢音；
- `piano_block_matches_scalar_for_sixty_four_simultaneous_notes`：64 个
  同时音符的 block 输出与逐采样一致；
- `voice_allocator_steals_released_voice_before_lighter_held_voice`：
  已释放声部先于更轻的按住声部被抢占；
- `voice_allocator_falls_back_to_lightest_then_oldest`：无释放声部时先
  抢最轻，再抢最旧；
- `voice_slot_is_reused_after_tail_release`：尾音结束后槽位直接复用，
  不产生抢占；
- `piano_polyphony_is_explicit_and_bounded`、
  `render_tail_truncates_only_provable_damper_silence`：上限与尾音截断
  的边界行为。

既有混合起始、gate 中段翻转、seek/stop 状态重置与
`physical_piano_block_matches_scalar_render` 全部继续通过。

## 性能

本机 release、48kHz、64 帧 block、2 秒离线渲染：

| 场景 | 改造前 | 改造后 | 实时系数 |
| --- | --- | --- | --- |
| 8 音 | 249–265ms | 199–201ms | 0.10 |
| 12 音 | 376–387ms | 287–311ms | 0.14 |
| 32 同时 | 约 0.99s | 0.73s | 0.36 |
| 64 同时 | 约 1.98s | 1.40–1.52s | 0.70–0.76 |
| 128 同时 | 未测（>128 整块回退） | 2.90–2.97s | 1.45–1.49 |
| 300 顺序 | 整块回退 | 1.15–1.17s | 0.58 |

64 同时音符的目标是 ≤2.0 秒，多次实测 1.40–1.50 秒（本机有调度抖动）。
8 音 deadline 报告从 740–782ms 降到约 0.59–0.60s（P99 315.6–321.1µs，
仅为 1.333ms 预算的 24%），12 音从 1118–1160ms 降到约 0.88–0.93s
（P99 476.1–530.5µs），全部 block 无回退。

## 已知限制

- 128 个同时音符仍超过单核实时（约 1.45×），但这是显式配置的容量，
  不是隐式丢弃；需要更多复音时由轨道显式提高上限并承担成本。
- 同一块内、尚未渲染就被抢占的音符计入 `voice_drops`；这是抢占语义
  的边界，正常密度下为 0。
- 块不连续时按 `(起始时刻, 音符下标)` 顺序重建分配，后分配的音符优先；
  与连续播放的抢占顺序可能不同，但仍然是确定且可测试的。
- 尾音截断只覆盖“输出即制音器”的声部；其他声部仍渲染完整 0.5 秒窗口。
- 每块发现 note-on 仍是 O(N) 扫描，块内事件数为 E 时为 O(E×N)；数千个
  音符同帧起音会让首块明显变重，常规轨道规模下可忽略。
- `PianoPlan::sample`（逐采样路径）仍使用 0.5 秒窗口；截断只发生在
  block 段边界，两条路径在静音段输出一致。
- 逐采样路径的声部状态仍按 `note_index % VOICE_STATE_SLOTS` 取槽；超过
  128 个音符时相隔 128 的两个音符会共享同一份 Damper 状态。正常播放、
  seek 与停止恢复都走 block 路径，只有循环和 seek 淡入淡出会退回逐采样，
  大工程在这些短窗口内可能出现轻微状态串扰；彻底修复需要把逐采样路径
  也改成显式槽位，属于后续工作。
- `track.polyphony` 只作用于轨道主乐器节点；UI 尚未接入该命令。

## 复现

```sh
cargo test -p aurtrack-engine
cargo test -p aurtrack-session
cargo test --release -p aurtrack-session --test block_realtime \
  polyphony_stress_benchmark -- --ignored --nocapture
cargo test --release -p aurtrack-session --test block_realtime \
  block_deadline_report -- --ignored --nocapture
AURTRACK_BENCH_NOTES=64 cargo test --release -p aurtrack-session \
  playback_block_benchmark -- --ignored --nocapture
```
