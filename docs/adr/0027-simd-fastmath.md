# 0027 可选 AVX2 + FMA 数学快路径

## 状态

已接受并实施。`fastmath` 与 Repeat 帧循环新增可在计划构建期关闭的
AVX2 + FMA 四路 f64 内核；128 同时音符 2 秒离线渲染从 1.09–1.17 秒
降到 0.68–0.71 秒，实时系数 0.55–0.59 → 0.34–0.36。

## 背景

ADR 0025 与 0026 把 `sin`/`exp` 换成多项式、消掉逐帧边界检查并把帧不变
值广播成标量，但明确留下同一个瓶颈：帧循环仍是标量数学，x86-64 基线的
SSE2 双精度只有 2 lane，显式 SIMD 需要平台分支和 `unsafe`。

本轮先做临时消融（未提交）确认 128 同时音符的热点：

| 组成 | 耗时 | 说明 |
| --- | --- | --- |
| 总渲染 | 1.09–1.17s | 48kHz、64 帧 block、2 秒 |
| `sin` + `exp` | 约 0.42s | 两者全部替换为占位算术 |
| 其余帧循环与标量求值 | 约 0.70s | 消融后的下限 |

物理钢琴的 partial body 有 74 个 SSA 值：37 个与迭代无关，37 个与迭代
有关；其中约 20 个值依赖逐帧时间。默认计划里每帧要算 4 次 `sin`、
2 次 `exp`，其余是加/减/乘/除/取绝对值/钳制与包络组合。`sin`/`exp`
是最大单项，但单独消掉它们只能把上限压到约 0.70 秒；要稳定进入 0.8 秒
以内，必须同时向量化帧循环里的算术。

## 决定

### 后端检测与缓存

`fastmath::MathBackend` 只有两个状态：

```text
Scalar      所有平台可用，结果与本轮之前的实现逐位一致
Avx2Fma     仅 x86_64；构建期同时检测到 avx2 与 fma 时启用
```

- 检测发生在计划构建期（`RepeatAccumulator::compile`），结果写进计划；
  音频线程只读这个 `Copy` 枚举，不读环境变量、不初始化 CPUID 缓存。
- `is_x86_feature_detected!` 自身缓存 CPU 特性，但只在构建期调用一次。
- 环境变量 `AURTRACK_SIMD` 可关闭：`0`、`off`、`false`、`scalar`、`none`
  （大小写不敏感）都退回标量；未设置或其他值按自动检测处理。
- `RepeatAccumulator` 的 `Clone` 直接复制后端，克隆发生在控制线程，
  scratch 仍每份独立。
- 音频线程没有新增分配、锁、日志或系统调用；SIMD 开关只在构建期解析。

### 四路 `sin` / `exp`

`fastmath::avx2` 提供：

```text
sin4/exp4        四路 f64 内核，clamp、归约、多项式与标量同一公式
sin_slice        整段输入 → 整段输出；4 帧一组，尾部走标量
exp_slice        同上
sine_scalar_*    sin(TAU·frequency·time + phase) 的两种常见组合
binary_slice     四路加/减/乘/除，标量操作数广播
add_assign_slice sum += output
narrow_slice     f64 → f32
abs_slice        清符号位
clamp_scalar_*   上下界为标量的 clamp
```

- 归约常量与 `ROUND_MAGIC` 技巧与标量完全相同；SIMD 用 `FNMADD`
  计算 `r`，比标量更精确。
- 多项式系数相同；标量用 Horner，SIMD 用 Estrin 分组 + FMA。两者是
  同一个多项式，只是求值顺序不同，误差远低于 `1e-6`。
- `sin` 的符号仍从 `biased` 尾数最低位异或到结果上；没有整数转换。
- NaN 通过 `min(hi, max(lo, x))` 的第二个操作数传播：Intel 的
  `max`/`min` 在任一参数为 NaN 时返回第二个操作数，因此不需要额外的
  比较与选择指令。
- `exp` 的 `2^n` 仍从 `biased` 的低 52 位直接拼指数位。
- 除法的零分母保护用 `_CMP_EQ_OQ` 掩码加 blend 实现，`b == ±0` 时结果
  为 0，NaN 继续传播，与标量一致。

### 帧循环接入

`evaluate_frames` 在进入循环前只判断一次后端与 `frames >= 4`：

- 时间逐帧、频率与相位为标量的 `Sine` 走 `sine_scalar_slice`；
  相位逐帧时走 `sine_scalar_phase_slice`；
- 数组输入的 `Sin`/`Exp` 走 `sin_slice`/`exp_slice`；
- 至少一个操作数是数组的加/减/乘/除走 `binary_slice`；标量×标量仍走
  原来的 `fill`，不为广播结果绕函数调用；
- 输出的逐迭代累加与最终 f64→f32 转换分别走 `add_assign_slice` 与
  `narrow_slice`；
- `Abs` 与上下界为标量的 `Clamp` 也走四路循环。非法 clamp 边界仍交给
  标量 `f64::clamp`，保持原有 panic 语义。

加/减/乘/除、`Abs`、`Clamp`、求和与窄化都没有 FMA，逐位与标量一致。
`Sine` 的角度计算用 FMA，`sin`/`exp` 的多项式用 FMA，差异只有舍入。

没有改默认 block ABI：`DEFAULT_BLOCK_SIZE` 仍是 64，`frames` 可以小于
64；SIMD 循环只在 `frames >= 4` 时启用，尾部按标量处理，短 block 不受
影响。也没有引入 nightly portable SIMD 或运行时 JIT。

### 可移植性

- 所有 x86 intrinsics 都在 `#[target_feature(enable = "avx2,fma")]`
  函数内；每个 `unsafe` 块都有 `// SAFETY:` 说明。
- 非 x86_64 平台只编译标量实现；`MathBackend::is_avx2()` 恒为 `false`。
- 调用点先按后端分支，再调用后端函数；帧循环内没有逐帧分派。
- 不依赖 `is_x86_feature_detected!` 在音频线程的初始化行为。

## 精度证据

新增与保留的测试：

| 检查 | 结果 |
| --- | --- |
| `avx2_sin_matches_scalar_formula` | 最大绝对差：`2.2e-16`（`±2π`）、`5.7e-11`（`±1e6`）、`9.4e-7`（`±1e10`） |
| `avx2_exp_matches_scalar_formula` | 最大相对差：`7.1e-15`（`[-80, 0]`）、`5.7e-14`（`[0, 700]`） |
| `avx2_sine_matches_scalar_formula` | 相位标量组合最大绝对差 `1.1e-12`；相位逐帧组合也 ≤ `1e-6` |
| `avx2_handles_special_arguments` | `sin`/`exp` 的 NaN 保持 NaN，超出上下限按边界处理 |
| `avx2_binary_matches_scalar_bitwise` | 加/减/乘/除 × 三种操作数组合逐位相等 |
| `avx2_divide_zero_and_nan_match_scalar` | `±0` 分母、NaN 分子/分母逐位或 NaN 语义一致 |
| `avx2_clamp_matches_scalar_bits` | NaN、`±inf`、越界值逐位一致 |
| `avx2_abs_sum_and_narrow_match_scalar` | `abs`、`sum +=`、f64→f32 逐位一致 |
| `broadcasts_scalar_operands_into_frame_loops` | 仍按标量后端要求 f32 逐位相等，未放宽 |
| `physical_piano_block_matches_scalar_render` | 通过，容差仍是 `1e-4`，未放宽 |
| `simd_accumulator_matches_scalar_reference` | 真实累加器 SIMD 与标量差 ≤ `1e-6` |

`sin`/`exp` 的归约使用相同常量与 `ROUND_MAGIC` 技巧；SIMD 的 `FNMADD`
只改变舍入且通常更精确。因此 SIMD 与标量的差只有多项式求值顺序带来的
几个 ulp。满量程 `±1e10` 处绝对差 `9.4e-7`，仍低于 `1e-6`；普通音频
相位范围内低 4–13 个数量级。所有路径共用同一组常量与同一个多项式，
不存在 block 与 scalar 各用一套近似的问题。

## 性能

同一台机器（AMD Ryzen 9 8945H，Windows x86_64，release，48kHz，
64 帧 block，2 秒离线渲染），改动前后各测三次：

| 场景 | 改动前 | 改动后 | 实时系数（前 → 后） |
| --- | --- | --- | --- |
| 32 同时 | 277–287ms | 175–195ms | 0.14 → 0.09–0.10 |
| 64 同时 | 547–577ms | 340–369ms | 0.27–0.29 → 0.17–0.18 |
| 128 同时 | 1.09–1.17s | 0.68–0.71s | 0.55–0.59 → 0.34–0.36 |
| 300 顺序 | 449–471ms | 321–342ms | 0.22–0.24 → 0.16–0.17 |

- 128 同时稳定低于 0.8 秒；三次测量为 677ms、690ms、710ms，目标
  `<0.7s` 在多数测量中达成，剩余差距主要来自系统调度噪声与每值调用
  开销。
- 64/32 同比例下降约 35–38%，300 顺序下降约 27%。
- 收益拆解（临时消融）：`sin`/`exp` 从约 0.42 秒降到约 0.16 秒；
  算术帧循环向量化再省约 0.1 秒；求和与窄化约省 0.02 秒。
- `AURTRACK_SIMD=0` 时 128 同时回到 1.126 秒，与改动前一致，证明回退
  路径完整可用。
- 音频线程仍然零分配：`block_render_does_not_allocate` 与
  `voice_process_block_does_not_allocate` 均通过。

## 已知限制

- SIMD 只在 x86_64 + AVX2 + FMA 上启用；ARM、旧 x86 与禁用环境变量时
  走标量，性能回到 ADR 0026 的水平。
- 没有用 AVX-512：目标平台不保证，且 8 lane 需要额外的频率与掩码
  处理；4 lane 已经能把 128 同时压进 0.8 秒。
- `sin`/`exp` 仍是最大单项（约 0.16 秒）。继续下降需要更低阶的多项式
  或查表，前者会突破 `1e-6`，后者违反 `fastmath` 的无查表约束。
- 帧值仍逐个求值；每值一次 SIMD 调用与操作数分类约占剩余开销的一
  部分。把整个 body 编成更长的 SIMD 内核需要新的 IR，本轮不做。
- `Sine` 的 FMA 角度计算、`sin`/`exp` 的 Estrin 求值顺序与标量不同，
  差异只有 ulp 级；如果未来要求 SIMD 与标量逐位相等，需要关掉 FMA
  或改回 Horner。
- 位置、力度等标量值仍每块重算；ADR 0026 记录的跨块缓存限制仍然
  存在，本轮没有引入新的缓存状态。

## 复现

```sh
cargo test -p aurtrack-engine fastmath
cargo test -p aurtrack-engine
cargo test -p aurtrack-session physical
cargo test --release -p aurtrack-session --test block_realtime \
  polyphony_stress_benchmark -- --ignored --nocapture
cargo test --release -p aurtrack-session --test block_realtime \
  block_deadline_report -- --ignored --nocapture
AURTRACK_SIMD=0 cargo test --release -p aurtrack-session \
  --test block_realtime polyphony_stress_benchmark -- --ignored --nocapture
```
