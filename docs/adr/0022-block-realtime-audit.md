# 0022 Block 播放路径实时安全审计

## 状态

已接受，审计完成；block 内核与整条回调路径均达标，静音保护尾段不再整块回退。

## 背景

`Playback::render_block` 在无循环、无 seek/stop 时走 block 路径：

- `TrackPlan::process_block` 按轨切分；
- `PianoPlan::sample_block` 按音符起始帧切段；
- `GraphPlan::process_block_voices` 节点主序执行；
- `VoicePlan` 的 block 内核负责声部求值。

release 基准此前只测 2 秒离线总耗时：8 音约 0.68 秒、12 音约 1.01 秒。
总耗时低不代表回调安全：48kHz 下 64 帧只有 1.333ms 预算，一次分配、
一次锁或一段未归因的逐采样回退都可能让单个 block 超时。

本 ADR 回答三个问题：

1. block 热路径是否真的零堆分配；
2. 单个 64 帧 block 的最坏耗时和 P99；
3. block 路径是否混入锁、日志或系统调用。

## 审计方法

### 分配计数

`crates/aurtrack-session/tests/block_realtime.rs` 是独立测试二进制，
在测试 crate 内定义 `#[global_allocator]`：

- 计数器是线程局部的 `Cell<u64>`，不分配、不阻塞；其他测试线程不受影响；
- 生产 crate 不包含计数代码，也不增加任何热路径分支；
- 用命令总线搭出与 `playback_block_benchmark` 同源的物理钢琴工程，
  在 `Playback::play()` 之后清零计数，再连续渲染 1000 个 64 帧 block；
- 1000 个 block 约 1.33 秒，覆盖制音器尾音和整块静音段；修复后这段
  仍走 block 内核，不再触发逐采样回退；

### Deadline 基准

同一个集成测试里提供 `block_deadline_report`：

- 每轮一个新 `Playback`，计划构建不计时；
- 8 音、12 音各跑 3 轮，每轮 1500 个 64 帧 block（2 秒音频）；
- 用 `Instant` 只包住 `Playback::render`，计时代码不进入生产路径；
- 统计最大 block、P99、超过 1ms/5ms 的 block 数；
- 通过 `block_stats().playback_block_fallbacks` 的增量标记回退 block，
  再单独统计“纯 block 内核”的最大值和 P99，避免把回退耗时混进内核。

### 静态扫描

在 `crates/aurtrack-engine/src` 检索
`Mutex`、`RwLock`、`println!`、`eprintln!`、`Instant::now`、
`SystemTime`、`std::fs`、`std::process`、`thread::sleep`：

- 锁、日志、计时、文件、进程调用均未出现在引擎中；
- `std::sync` 只有计划共享的 `Arc` 和 `Atomic*`；
- `ArcSwap::load` 是计划快照读取，实测 1000 个 block 期间零分配。

## 发现与修复

### 1. `VoicePlan::process_block` 每块编译 SSA

审计测试先命中 19 次分配/次调用。根因是公开 block API 在每次调用时执行
`VoiceSsa::compile`：构建 `Vec`、`HashMap` 并生成 `format!` 字符串。
`process_block_inputs` 的兜底分支也有同样问题，只是当前物理钢琴计划没有
走到。

修复：

- `VoicePlan::process_block` 不再编译 SSA，改为把标量输入铺成 block 输入后
  调用已有的 `eval_top_block` 求值器；
- `process_block_inputs` 删除逐块 `VoiceSsa::compile` 兜底；不支持 block 的
  图返回 `false`，由调用方回退逐采样；
- 为 `eval_top_block` 补齐 `MapRange` 分支，保证原 SSA 支持的无状态操作
  仍能走 block；
- 两个入口都增加 `output.len() > DEFAULT_BLOCK_SIZE` 的提前返回，避免
  超长输出触发栈数组切片 panic；
- 标量输入版最多支持 8 个输入槽，超出时明确返回 `false`。

`SsaKernel` 仍是“编译期编译、运行期只读 scratch”的显式路径，不受影响。

### 2. 分配结果

| 场景 | 修复前 | 修复后 |
| --- | --- | --- |
| `VoicePlan::process_block` 单次调用 | 19 次分配 | 0 |
| `Playback::render` 1000 个 block（8 音） | 未测 | 0 |
| `Playback::render` 1000 个 block（12 音） | 未测 | 0 |

修复前 1000 个 block 内约有 106 个静音回退 block。修复后 8/12 音都是
`playback_block_fallbacks = silent_block_fallbacks = 0`、
`piano_block_fallbacks = 0`，1000 个 block 的分配计数仍为 0。
静音保护本身没有被删除：`silent_fallback` 集成测试用合成静音声部验证
“音符仍在发声”“gate 已关闭但仍在尾窗内”都会显式回退，
超过真实尾音后则清零输出并正常推进。

### 3. 静音保护尾段整块回退

`Playback::block_output_needs_fallback` 在 block 峰值低于
`BLOCK_SILENCE_EPS`、但 `tracks_expect_audio` 认为还有音符时，整块回退到
逐采样，防止 block 内核静默丢音。

根因是 `tracks_expect_audio` 对没有轨道级 ADSR 的轨道使用固定 0.5 秒
尾窗。物理钢琴的尾音由声部图里的 `Damper` 决定：gate 关闭后电平每个
采样减 `release`（默认 0.00012），约 0.174 秒归零；音板余振到约
1.19 秒也低于静音阈值。但固定窗口持续到 1.5 秒，于是 1192–1500ms 的
每个 block 都触发一次 64 帧标量重算：每次 2 秒运行 232 个回退 block。

修复：

- `TrackPlan::note_tail_seconds` 优先从已编译声部推导尾音。只有
  `voice.output` 经 `Output`/`IntToFloat` 直通后就是 `Damper`、gate
  直接来自 `VoiceOp::Gate`、pedal 常量为零、damping 为正的常量时，
  才能证明制音器电平在 `1 / damping` 个采样后精确归零；尾音上界取该值；
- 推导结果仍以 `PianoPlan` 的 0.5 秒截断窗口封顶，与该计划逐采样路径
  的实际语义一致；
- 任何一项无法证明（动态 pedal、常开门控、无制音器声部、多级包络）
  都退回 0.5 秒保守窗口，宁可多回退一次也不吞掉尾音；
- `tracks_expect_audio` 改用推导出的尾音。分类为 `SilentTail` 的 block
  直接把混音清零后正常推进；`SuspectSilence` 仍回退逐采样。轨道 block
  失败、PianoPlan 内核失败和静音保护仍各自保留显式计数与回退，不会把
  失败误判为静音；
- 推导只读已编译计划的节点表，发生在 block 已静音时，不分配、不加锁、
  不写日志。

## Deadline 数据

本机、release、48kHz、64 帧预算 1333.3µs，3 轮共 4500 个 block。
修复前是历史两次运行与本次复现的区间；修复后是连续三次运行的区间：

| 音数 | 修复前总耗时 | 修复前最大 | 修复前 P99 | 修复后总耗时 | 修复后最大 | 修复后 P99 | 修复后 >1ms |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 8 | 1956–1980ms | 2985–3227µs | 2146–2846µs | 740–782ms | 461–573µs | 320–363µs | 0 |
| 12 | 2978–2998ms | 4548–4672µs | 3370–4246µs | 1118–1160ms | 710–789µs | 487–513µs | 0 |

修复前每次 2 秒运行有 232 个静音保护回退 block，3 轮共 696 个，
`>1ms` 的 693 个 block 全部落在回退段。修复后 4500 个 block 没有一次回退，
`block_max_us` 与整体 `max_us` 相同；8 音最大耗时有约 2.3× 余量，
12 音约 1.7× 余量，P99 分别约 3.7× 和 2.6× 余量。

真实 `Playback` 的 2 秒离线渲染（`playback_block_benchmark`）：

| 音数 | 修复前耗时 | 修复后耗时 | peak | 静音回退 | accumulations |
| --- | --- | --- | --- | --- | --- |
| 8 | 约 0.68s | 249–265ms | 0.223 | 0 | 9016 |
| 12 | 约 1.01s | 376–387ms | 0.291 | 0 | 13524 |

两条路径都保持 peak 非零、`piano_fallbacks = 0`，Repeat 原生累加器命中。

## 根因

`tracks_expect_audio` 用固定的 0.5 秒尾窗判断“音符是否还应发声”。物理钢琴
没有轨道级 ADSR，尾音由声部图里的 `Damper` 决定：gate 关闭后电平按
`release` 每采样下降，默认 0.00012 时约 0.174 秒归零；音板余振到约 1.19 秒
也低于 `BLOCK_SILENCE_EPS`。窗口却持续到 1.5 秒，于是 1192–1500ms 的每个
block 都被判为“应发声但输出静音”，整块回退逐采样。回退本身不分配，但它把
本可整块输出的静音尾段变成了最慢的 block。

## 已实施与后续

1. **已实施：让静音保护感知包络**。用已编译声部推导制音器尾音，见
   “静音保护尾段整块回退”。
2. **已实施：回退有明确上界**。物理钢琴的静音尾段回退从 232 个降为 0；
   无法证明尾音的声部仍保留 0.5 秒窗口。
3. **后续：扩大可证明范围**。可以为“输出即 ADSR”或制音器后无延迟/混响的
   声部补充线性包络推导；当前不为这些图收紧窗口。
4. 任何方案都必须保留 `physical_piano_block_matches_scalar_render` 一类
   一致性测试，并重新跑本 ADR 的 deadline 基准。

## 已知限制

- 尾音推导只覆盖“最终输出就是制音器”的声部；多级包络、动态 pedal、
  制音器之后还有延迟或混响的图仍使用 0.5 秒保守窗口；
- 推导以 `PianoPlan` 的 0.5 秒截断窗口为上限，与既有逐采样语义一致；
- seek/stop/loop 仍走逐采样或淡化路径，不在本次 block 审计范围；
- 插件处理在引擎 block 路径之外，插件自身耗时未计入；
- 任一轨道 `graph = None` 时整个播放计划禁用 block 路径；
- 标量输入版 `VoicePlan::process_block` 最多 8 个输入槽；
- 引擎里已有 `BLOCK_CALLS`、`TRACK_BLOCKS`、`REPEAT_*` 等 `Relaxed`
  原子计数器，均在 block 或迭代粒度；本次审计没有新增原子；
- 计时和分配计数只存在于测试 crate，生产路径没有审计开销。

## 复现

```sh
cargo test -p aurtrack-session --test block_realtime -- --nocapture
cargo test -p aurtrack-session --test silent_fallback
cargo test --release -p aurtrack-session --test block_realtime \
  block_deadline_report -- --ignored --nocapture
AURTRACK_BENCH_NOTES=12 cargo test -p aurtrack-session \
  playback_block_benchmark -- --ignored --nocapture
```

第一条验证 8/12 音零分配且尾段不再回退；第二条验证静音保护仍按真实尾音
回退；第三条输出上表 deadline 数据，release 下断言整条路径的 P99 和最大值
都不超过 1.333ms 且没有静音回退；第四条输出真实 `Playback` 基准
（默认 8 音，设置环境变量后跑 12 音）。
