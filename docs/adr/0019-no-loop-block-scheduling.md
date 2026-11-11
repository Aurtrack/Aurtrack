# 0019 无循环播放与 block 调度

## 状态

已接受。

## 背景

`Playback::render_block` 的 block 分支有两处互相掩护的缺陷：

1. 分支条件要求 `loop_end_frames == 0`，但该值最小为 1，分支不可达；
2. 无循环时 `advance` 在位置 0 就回绕到 0，逐采样路径一直读第 0 帧。

因此基准测到的是“第 0 帧重复 96000 次”，不是真正播放。block 内核与逐采样
路径还有状态偏差：`RepeatResult` 用 `carried` 填 body 槽位、`Damper` 初值
为 `NaN`、`Time`/`Hammer` 固定按 48kHz 推进、`PianoPlan` 只有 8 个音符状态。

## 决定

### 播放时序

- `loop_seconds <= 0` 视为无循环：block 一次推进 `frames`，逐采样每帧
  `position += 1`，不再调用 `advance`。
- 有循环保留现有 `advance`、loop fade 和 seek fade；block 只在无循环且
  无 seek/stop 时启用。
- block 先在栈上混入 `mixed[DEFAULT_BLOCK_SIZE]`。任一轨 `process_block`
  返回 false，或混音接近静音但存在活跃源时，整块丢弃并回退逐采样，不保留
  零块或部分数据。
- 活跃源判定覆盖音符释放尾、`fallback_sine` 和无 graph 的固定频率轨。
- `BlockStats` 增加 `playback_block_fallbacks`、`silent_block_fallbacks`、
  `piano_block_fallbacks`，基准据此证明 block 真实命中，而不是被回退掩盖。

### block 内核对齐

- `VoiceBlockInputs` 显式携带采样率，`Time` 和 `Hammer` 按帧推进。
- `RepeatResult` 的 `carried` 按输出槽数截断，body 输入槽按
  `Repeat.inputs[1 + slot]` 填满。
- `VoiceState::damper_level` 从 0 起始，与逐采样 `Damper` 的节点状态一致；
  block 起音增益也按帧推进，避免 `play()` 后状态晚 144 帧。
- `PianoPlan` 的 block 状态容量对齐 `VOICE_STATE_SLOTS`（128），音符数超过
  容量时整块回退逐采样。

## 后果

- 8 音基准 473ms，12 音 692ms（release，2 秒离线）；峰值 0.218/0.257，
  `track_blocks = 1500`，`piano_block_fallbacks = 0`。
- 尾音跌破 1e-6 时静音保护触发，两种规模均约 230 个 block，属预期回退。
- 新增 `physical_piano_block_matches_scalar_render`，校验锤击瞬态结束后
  block 与逐采样的误差小于 1e-4。

## 已知限制

- `PianoPlan::sample_block` 仍要求所有音符在 block 起点已开始；混合起始的
  block 在 `PianoPlan` 内回退逐采样，此时专用 `VoiceState` 不推进，恢复
  block 时包络可能有轻微跳变。
- seek/stop 期间走逐采样，block 状态不重置；与逐采样 `Damper` 的
  `node.state` 一样，seek 不重算包络。
