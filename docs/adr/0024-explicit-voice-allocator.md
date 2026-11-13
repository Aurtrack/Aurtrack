# 0024 显式声部分配器

## 状态

已接受并实施；block 与逐采样路径都按显式活动声部槽分配，block note-on
用排序游标裁剪，不再每块扫描全表。

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

### 逐采样声部槽

`PianoPlan::sample` 不再用 `note_index % VOICE_STATE_SLOTS` 取状态：

- 标量与 block 共用同一个 `VoiceAllocator` 和 `VoiceState` 池；槽位状态
  包含制音器电平、节点 scratch 和逐采样历史。
- 分配器维护音符下标 → 槽的定长开放寻址表（1024 槽、墓碑重建），
  note-on 在采样交给该音符时分配，槽位释放后可直接复用；复音上限仍由
  `polyphony` 显式决定，用尽时走与 block 相同的抢占顺序。
- `VoiceState` 记录 `slot` 与 `last_seen`。逐采样时间回退（seek、循环
  回卷）时只重建受影响槽的状态，不整体重置分配表；block 段末同样更新
  `last_seen`，两条路径切换时制音器和包络保持连续。
- 槽位复用时 `VoicePlan::reset_slot` 清零该槽的节点状态、逐采样 scratch
  和 Delay/Waveguide 缓冲；音频线程只遍历既有数组，不分配、不加锁。
- `VOICE_STATE_SLOTS` 与 `MAX_POLYPHONY` 对齐为 256；Biquad 每声部占
  两槽，节点状态数组按两倍容量预分配。

### note 扫描游标

- `PianoPlan` 在计划构建期用 `prepare_notes` 按 `(start, index)` 排序，
  保存 `order`、最大 duration 和首尾 start；`Playback::set_plan`、
  离线渲染和测试计划在控制线程完成该步骤。
- 连续 block 从 `VoiceAllocator::next_order` 继续，只处理 `start <
  block_end` 的新音符，遇到未来音符立即停止；块末帧的起音不消费游标。
- 块不连续时用 `start >= block_start - (max_duration + tail)` 二分出
  下界，只扫描窗口内可能活跃的音符；`audible` 判断同样走该窗口。
- 未调用 `prepare_notes` 的计划仍回退到全表扫描，行为正确但不享受裁剪。

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
- `scalar_voices_do_not_share_state_across_128_boundary`：第 129 个音符
  不读取第 1 个音符的制音器电平；
- `scalar_matches_block_across_128_boundary`：300 个顺序音符在 128 边界
  附近 block 与逐采样输出一致；
- `scalar_after_block_keeps_voice_state_continuous`：block 切到逐采样后
  制音器和包络继续推进，不在切换点重置；
- `block_note_scan_advances_with_cursor`：连续块的 note 游标只前进，
  不回到全表起点；
- `scalar_note_129_does_not_reuse_note_1_damper_state`（集成测试，仅用
  公开 API）：旧实现实测输出约 0.174，新实现约 0.0885；
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

本机 release、48kHz、64 帧 block、2 秒离线渲染。调度抖动较大，下表同时
给出固定单核的对照，避免把系统噪声当成改动收益或退化：

| 场景 | 基线（本机复测） | 本次 | 实时系数 |
| --- | --- | --- | --- |
| 32 同时 | 0.99–1.09s | 0.93–1.09s | 0.46–0.55 |
| 64 同时 | 1.66–2.27s | 1.75–2.27s | 0.87–1.14 |
| 128 同时 | 3.70s | 3.16–3.61s | 1.58–1.81 |
| 300 顺序 | 1.45s | 1.42–1.43s | 0.71 |

单核固定时 32/64 同时与基线持平（64 同时基线 2.2736s、本次 2.2740s，
差异在计时噪声内）；128 同时大致持平到略好，改善主要来自块内 note-on
不再扫全表。64 同时的目标仍是 ≤2.0 秒，本机未固定调度时 1.75–1.95s
可达，单核固定时基线与本次都约 2.27s。
8 音 deadline 报告与 12 音 SSA 路径未受本次改动影响。

## 已知限制

- 128 个同时音符仍超过单核实时，但这是显式配置的容量，不是隐式丢弃；
  需要更多复音时由轨道显式提高上限并承担成本。
- 同一块内、尚未渲染就被抢占的音符计入 `voice_drops`；这是抢占语义
  的边界，正常密度下为 0。
- 块不连续时按 `(起始时刻, 音符下标)` 顺序重建分配，后分配的音符优先；
  与连续播放的抢占顺序可能不同，但仍然是确定且可测试的。
- 尾音截断只覆盖“输出即制音器”的声部；其他声部仍渲染完整 0.5 秒窗口。
- 连续块的 note-on 已按游标裁剪；块不连续时仍要扫描
  `max_duration + 0.5s` 窗口内的音符，数千音符同帧起音会让首个 block
  变重，这是重建分配表的必要成本。
- 未调用 `prepare_notes` 的计划（例如直接构造 `TrackPlan` 的测试或外部
  调用方）仍回退到每块全表扫描；生产路径由 `Playback::set_plan` 统一
  准备。
- 逐采样路径的尾窗仍是固定 0.5 秒；截断只发生在 block 段边界，两条路径
  在静音段输出一致。
- 逐采样路径不为已结束的槽做主动清扫，而是复用分配器的抢占规则回收；
  统计里的 `active_voices` 可能包含尚未被抢占的旧槽，但不会超过显式
  复音上限。
- 循环交叉淡化会在同一采样内用两个位置渲染同一计划；两段位置各有自己的
  槽位与 `last_seen`，正常音符不会串状态。跨循环点持续按住的长音符仍会
  在回卷时重建状态，这是位置不连续时的确定行为。
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
