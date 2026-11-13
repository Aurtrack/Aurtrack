# 0025 多项式三角函数与帧循环去边界检查

## 状态

已接受并实施。`RepeatAccumulator` 的帧循环改用多项式 `sin`/`exp` 并消除
逐帧边界检查；128 同时音符 2 秒离线渲染从 3.63 秒降到 1.50–1.88 秒。

## 背景

ADR 0020 的 Repeat 累加器把物理钢琴的 partial 循环降成 SSA：每个 partial
每帧要算 3 次 `sin`（主弦、微失谐弦、低八度弦）和 1 次 `exp`（包络），
另有约 19 个逐帧算术值。ADR 0020 的已知限制写着“帧循环仍是 f64 标量数学，
sin/exp 是主要成本；没有 SIMD 或近似三角函数”。

本机 release、48kHz、64 帧 block、2 秒离线渲染的实测：

| 场景 | 耗时 | 实时系数 |
| --- | --- | --- |
| 32 同时 | 1.04s | 0.52 |
| 64 同时 | 1.97s | 0.98 |
| 128 同时 | 3.63s | 1.82 |
| 300 顺序 | 1.46s | 0.73 |

拆解 128 同时的耗时（临时探针，未提交）：每个 block 有 61,568 次 SSA 值
求值，其中 402,944 次 `sin` 与 `exp` 调用。把 `sin`/`exp` 换成占位算术后
耗时降到 2.13 秒，说明超越函数约占 1.5 秒；再移除帧循环的算术只剩
0.60 秒，其中大半是每次值求值的分派与常量 `fill`。

## 决定

### 多项式 `sin`

`crates/aurtrack-engine/src/fastmath.rs`：

```text
n      = round(x / π)              // 舍入用 1.5·2^52 的浮点技巧，无 libm 调用
r      = (x - n·PI_HI) - n·PI_LO   // 两步相减，PI_HI = f64(π)，PI_LO 为余项
sin(x) = (-1)^n · r · P(r²)        // P 为 6 阶切比雪夫插值
```

- `P` 的系数由 `sin(r)/r` 在 `u = r² ∈ [0, (π/2)²]` 上的切比雪夫插值得到，
  在 `[-π/2, π/2]` 上最大绝对误差约 `1.7e-11`。
- 奇偶 `(-1)^n` 直接取舍入结果 `biased = n + 1.5·2^52` 的尾数最低位再异或
  符号位：`n` 是整数，`2^51` 是偶数，最低位就是 `n` 的奇偶。**不能**用
  `n as i64`，整数转换会引入 libm 调用（x86-64 基线没有 `roundsd`）。
- 相位先 `clamp` 到 `±1e10`。粗归约的抵消误差约为 `ulp(x)`，`1e10` 处约
  `1e-6`，仍低于 1e-5 目标；`1e10` 弧度相当于 20 kHz 连续播放 22 小时。

### 多项式 `exp`

```text
n       = round(x / ln2)
r       = (x - n·LN2_HI) - n·LN2_LO      // r ∈ [-ln2/2, ln2/2]
exp(x)  = P(r) · 2^n                     // 2^n 直接拼指数位
```

- `P` 是 `exp(r)` 的 6 阶切比雪夫插值，最大相对误差约 `2.5e-9`。
- `2^n` 的指数从 `biased` 的低 52 位取出（`2^51 + n`），减偏置加 1023 后
  左移 52 位；负数用回绕减法，debug 构建也不会溢出。
- 参数先 `clamp` 到 `[-700, 700]`：更小的值在 `f32` 中已是 0，更大的值
  已是无穷。

### 全路径共用

近似必须同时用于 block 与逐采样，否则两条路径会漂移。`sin`/`exp` 的替换
覆盖：

- `RepeatAccumulator` 的 `evaluate_frames` 与 `evaluate`（`repeat.rs`）；
- `VoiceSsa` 的 block 与标量内核（`ssa.rs`）；
- `VoicePlan` 的 `eval_top_block`（block）与 `sample_values_traced`（标量）：
  `Sin`/`Exp`/`Sine`/`Hammer`/`Decay`/`Envelope`/`Partial`；
- 融合表达式 `VoiceExpr`/`CompiledVoice` 与 JIT 辅助函数；
- `partial_sample`、`osc_partials`、`sample_partials`；
- 常量折叠 `eval_constant`、图级 `Sine`、`Delay` 调制。

Biquad、Resonator、HighpassPlan 等只有单一路径的滤波器仍用 libm：它们
不参与 block/scalar 对照，替换收益也小。

### 帧循环去边界检查

`evaluate_frames` 原来按 `done[value.args[i]][frame]` 取值、在帧循环内部
对每个算子重新 `match`，而且 `frame < frames` 里 `frames` 是运行期参数，
编译器无法证明不越界，于是每帧都带一次边界检查。现在：

- 进入循环前把目标与操作数裁成 `frames` 长的等长切片；
- 二元、一元、`Sine`、`Clamp`/`MapRange` 按算子拆成独立循环，`match`
  留在循环外；
- 循环体只剩纯 f64 运算，`iter_mut().zip(...)` 不需要逐帧边界检查。

逐帧算术因此从约 1.53 秒降到约 0.78 秒，平均约 1 cycle/帧，已经接近标量
吞吐上限（每帧一次读、一次运算、一次写）。

**LLVM 没有把这些循环向量化。** 实测 `accumulate` 的汇编里只有
`movsd`/`mulsd`/`addsd`，没有 `mulpd`/`addpd`；把帧循环拆成独立函数、
或改成函数内 `split_at_mut` 都不改变结论。原因是切片的下标是运行期值，
LLVM 无法证明共享操作数与可变目标不重叠（Rust 只对 `&mut` 发 `noalias`），
而运行期别名检查在这种嵌套循环里没有被触发。显式 SIMD 需要平台分支和
`unsafe`，本轮不做。

没有新增每次 block 的初始化：切片裁剪和算子拆分都只在值求值时发生，
没有查表、没有预热、没有分配。

## 精度证据

`fastmath` 的单元测试与既有 block/scalar 对照：

| 检查 | 结果 |
| --- | --- |
| `sin` 在 `[-2π, 2π]`、`±1e3`、`±1e6` 上 | 最大绝对/相对误差 < `1e-9` |
| `sin` 在 `±1e10`（上限） | 最大绝对误差 < `1e-5` |
| `exp` 在 `[-80, 0]`、`±20`、`[0, 700]` | 最大相对误差 < `1e-7` |
| 特殊值 | `sin(NaN)`/`exp(NaN)` 仍为 NaN；超过上下限按边界处理且可复现 |
| `physical_piano_block_matches_scalar_render` | 通过（误差 < `1e-4`） |
| 混合起始、seek/stop、零分配 | 全部通过 |

误差比 `f32` 音频缓冲的量化噪声（约 `6e-8` 相对）低一到两个数量级，听感
上与 libm 版本无法区分；所有路径共用同一实现，因此 block 与 scalar 的差异
仍只来自既有的 f32/f64 中间表示。

## 性能

同一台机器、release、48kHz、64 帧 block、2 秒离线渲染，改动前后各测三次：

| 场景 | 改动前 | 改动后 | 实时系数（前 → 后） |
| --- | --- | --- | --- |
| 32 同时 | 1.00–1.04s | 0.40–0.43s | 0.50 → 0.20–0.21 |
| 64 同时 | 1.88–1.97s | 0.78–0.84s | 0.94–0.98 → 0.39–0.42 |
| 128 同时 | 3.63–3.65s | 1.50–1.88s | 1.82 → 0.75–0.94 |
| 300 顺序 | 1.45–1.46s | 0.60–0.97s | 0.72–0.73 → 0.30–0.49 |

- 128 同时首次进入 2 秒预算内（目标 <2.0s）；多数测量在 1.8 秒内，最差
  一次 1.88 秒。64 同时的实时系数从 0.98 降到 0.39–0.42，不再贴着 deadline。
- `block_deadline_report`（8/12 音，每轮回调独立计时）：p99 194–571µs，
  预算 1333µs；偶尔出现 1–4ms 的孤立尖峰（系统调度噪声），整块回退为 0，
  多次重跑都能满足 `over_5ms == 0` 与 `over_budget <= 2`。
- 收益来源：超越函数从约 1.5 秒降到约 0.26 秒；去掉逐帧边界检查后逐帧
  算术从约 1.53 秒降到约 0.78 秒；`fill` 与分派开销约 0.6 秒保持不变。
- 测量对系统状态敏感：同一提交连续测量会得到 1.50–1.88 秒的区间；
  ADR 0024 的表同样受此影响，对比时应保持相近的空闲条件。

## 已知限制

- 相位绝对值超过 `1e10` 时按 `1e10` 处理，不再调用 libm 做完整周期归约；
  这是为了在帧循环里保持无分支。正常音乐信号远达不到该量级，行为仍有界、
  可复现，并有单元测试锁定。
- `exp` 的参数被钳在 `[-700, 700]`；更小的值在 `f32` 输出中本来就是 0，
  更大的值本来就是无穷。
- 近似精度按音频需求设计，不是通用数学库：满量程相位（`1e10`）附近的
  绝对误差约 `1e-6`，不适合当作高精度计算使用。
- 帧循环没有向量化（原因见上）。x86-64 基线的 SSE2 双精度只有 2 lane，
  即使向量化上限也是约两倍；真要更快需要显式 SIMD 或按值广播。
- 累加器仍有约 0.6 秒的每值分派与 `fill` 开销（128 同时），是下一步优化
  方向；本轮没有引入按值广播或相位递推，避免状态化改动破坏 seek/混合
  起始语义。
- 纯标量路径（`PianoPlan` 没有 `voice` 时）仍然逐采样调用，不受益于帧
  循环的切片化；该路径只用于旧计划与兼容 API。

## 复现

```sh
cargo test -p aurtrack-engine fastmath
cargo test -p aurtrack-engine
cargo test -p aurtrack-session physical
cargo test --release -p aurtrack-session --test block_realtime \
  polyphony_stress_benchmark -- --ignored --nocapture
cargo test --release -p aurtrack-session --test block_realtime \
  block_deadline_report -- --ignored --nocapture
```
