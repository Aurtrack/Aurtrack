# 0022 Block 播放路径实时安全审计

## 状态

已接受，审计完成；block 内核达标，静音保护尾段存在已知超支项。

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
- 1000 个 block 会覆盖 894–1125 的静音保护尾段，因此 block 内核与整块
  回退两条路径都在计数范围内。

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

1000 个 block 内包含 106 个静音回退 block，`piano_block_fallbacks = 0`，
`playback_block_fallbacks = silent_block_fallbacks`，说明回退只来自静音保护，
不是 block 内核失败。

## Deadline 数据

本机、release、48kHz、64 帧预算 1333.3µs，3 轮共 4500 个 block。
下表是两次完整运行的区间，不是挑最好的一次：

| 音数 | 6 秒音频总耗时 | 实时系数 | 最大 block | P99 | >1ms | >5ms |
| --- | --- | --- | --- | --- | --- | --- |
| 8 | 1956–1964ms | 0.326–0.327 | 3067–3227µs | 2146–2173µs | 693 | 0 |
| 12 | 2978–2997ms | 0.496–0.500 | 4557–4672µs | 3370–4246µs | 693 | 0 |

只看纯 block 内核（排除静音保护回退）：

| 音数 | 最大 block | P99 | 相对 1333.3µs 余量 |
| --- | --- | --- | --- |
| 8 | 444–487µs | 303–325µs | 2.7×–3.0× |
| 12 | 655–673µs | 489–508µs | 约 2.0× |

`>1ms` 的 693 个 block 恰好等于 3 轮 × 231 个静音保护 block：每次 2 秒
运行有 232 个回退 block，除首个外都超过 1ms；正常 block 没有一个超过
1ms。P99 的波动也全部来自回退段，而不是 block 内核。

结论必须分开写：

- **block 内核本身安全**：8/12 音的最大值和 P99 都在 1.333ms 预算内；
- **整条回调路径尚不安全**：每次运行有 232 个静音保护 block 落在
  1192–1500ms，整块回退逐采样，最大 3.1ms/4.6ms，超过 1.333ms。

## 根因

`Playback::block_output_needs_fallback` 在 block 接近静音、但
`tracks_expect_audio` 认为仍有音符时，整块回退到逐采样，防止 block 内核
静默丢音。

`tracks_expect_audio` 对没有轨道级 ADSR 的轨道使用固定 0.5 秒尾音窗口。
物理钢琴的声部包络在约 1.19 秒已经衰减到静音，但窗口持续到 1.5 秒，于是
1192–1500ms 的每个 block 都触发一次 64 帧逐采样重算。回退本身不分配，但
它把“本可整块跳过”的静音尾段变成了最慢的 block。

## 优化建议

1. **让静音保护感知包络**：优先使用声部/钢琴的活跃状态而不是固定 0.5 秒
   窗口；或在确认 block 与标量探测都静音后，跳过直到下一个音符边界。
2. **给回退设上限**：生产路径不允许无界回退。可以只在首块回退并记录
   诊断计数器，其余交给后续 block；或把回退拆成有界片段，跨回调完成。
3. **把静音保护降级为调试/一致性检查**：CI 用 `block` 与逐采样对比覆盖，
   正式回调只计数不回退，避免“保护机制”自己破坏 deadline。
4. 任何方案都必须保留 `physical_piano_block_matches_scalar_render` 一类
   一致性测试，并重新跑本 ADR 的 deadline 基准。

## 已知限制

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
cargo test --release -p aurtrack-session --test block_realtime \
  block_deadline_report -- --ignored --nocapture
```

第一条同时验证 8/12 音零分配并覆盖静音回退段；第二条输出上表数据，
release 下断言纯 block 内核不超过 1.333ms。
