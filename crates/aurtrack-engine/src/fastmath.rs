//! 职责：
//! - 为音频热路径提供足够精确的多项式 `sin` 与 `exp`，替代 libm 调用
//!
//! 禁止：
//! - 查表、分配、加锁或依赖任何初始化状态
//! - 让 block 与 scalar 路径使用不同实现

// PERF: 物理钢琴每个 partial 每帧要算 3 次 `sin` 和 1 次 `exp`，libm 调用
// 占离线渲染一半以上的时间。这里的实现没有函数调用、没有分支，可以被内联；
// 帧循环本身仍是标量（切片的运行期下标挡住了向量化，见 ADR 0025）。

/// π 的 f64 主值与余项；范围缩减分两步相减，把乘积舍入误差压到目标精度内。
const PI_HI: f64 = std::f64::consts::PI;
/// 真实 π 减去 `PI_HI` 的余项。
const PI_LO: f64 = 1.224_646_799_147_353_2e-16;
/// `1/π`，把参数折算成半周期个数。
const INV_PI: f64 = std::f64::consts::FRAC_1_PI;
/// `1.5 * 2^52`：加上再减去它可把 `|x| < 2^51` 的数舍入到整数。
///
/// x86-64 基线没有 SSE4.1 的 `roundsd`，`f64::round` 会退化成 libm 调用；
/// 这个技巧既避免调用，也让奇偶校验只依赖 `biased` 的位模式。
const ROUND_MAGIC: f64 = 6_755_399_441_055_744.0;
/// `ROUND_MAGIC` 的尾数里 `2^51` 那一位；减去它才能得到真正的指数。
const ROUND_BIAS: u64 = 1 << 51;

/// 相位绝对值上限；超过时按上限处理。
///
/// 折算后 `x / π` 必须小于 `2^51`，且 `x - nπ` 的抵消误差要小于目标精度。
/// `1e10` 弧度相当于 20 kHz 连续播放约 22 小时，正常音乐信号不会触及；此时
/// 绝对误差仍约为 `1e-6`，见 `fastmath` 的测试。
const SIN_MAX_ARG: f64 = 1.0e10;

/// `exp` 的参数上限；超出后结果在 `f32` 中必然上溢或下溢。
const EXP_MAX_ARG: f64 = 700.0;
const INV_LN2: f64 = std::f64::consts::LOG2_E;
const LN2_HI: f64 = std::f64::consts::LN_2;
const LN2_LO: f64 = 2.319_046_813_846_299_6e-17;

/// `sin(r) / r` 在 `u = r²` 上的切比雪夫插值（6 阶，`r ∈ [-π/2, π/2]`）。
///
/// 最大绝对误差约 `1.7e-11`，远低于 `f32` 音频缓冲的量化噪声。
const SIN_COEFFICIENTS: [f64; 6] = [
    0.999_999_999_982_919_2,
    -0.166_666_666_168_152_57,
    0.008_333_330_974_193_892,
    -0.000_198_408_611_774_389_5,
    2.752_526_971_211_564_7e-6,
    -2.388_921_595_752_523e-8,
];

/// `exp(r)` 在 `r ∈ [-ln2/2, ln2/2]` 上的切比雪夫插值（6 阶），最大相对误差约 `2.5e-9`。
const EXP_COEFFICIENTS: [f64; 7] = [
    1.000_000_000_000_000_2,
    1.000_000_037_716_216_2,
    0.500_000_004_711_766,
    0.166_664_155_146_418_66,
    0.041_666_352_896_984_796,
    0.008_375_126_399_158_68,
    0.001_394_110_842_928_724_5,
];

/// `sin(x)` 的多项式实现。
///
/// `sin(x) = (-1)^n sin(r)`，`n = round(x / π)`、`r = x - nπ ∈ [-π/2, π/2]`。
/// 奇偶来自舍入后的位模式，不需要整数转换，因而整条链路无分支、可向量化。
#[inline]
#[must_use]
pub(crate) fn sin(x: f64) -> f64 {
    // NOTE: `clamp` 对 NaN 原样返回，NaN 会一直传播到输出。
    let x = x.clamp(-SIN_MAX_ARG, SIN_MAX_ARG);
    let biased = x * INV_PI + ROUND_MAGIC;
    let n = biased - ROUND_MAGIC;
    let r = (x - n * PI_HI) - n * PI_LO;
    let u = r * r;
    let mut polynomial = SIN_COEFFICIENTS[5];
    polynomial = polynomial * u + SIN_COEFFICIENTS[4];
    polynomial = polynomial * u + SIN_COEFFICIENTS[3];
    polynomial = polynomial * u + SIN_COEFFICIENTS[2];
    polynomial = polynomial * u + SIN_COEFFICIENTS[1];
    polynomial = polynomial * u + SIN_COEFFICIENTS[0];
    let value = r * polynomial;
    // 舍入后的 `biased` 尾数低比特就是 `n` 的奇偶；半周期为奇数时取反。
    let sign = (biased.to_bits() & 1) << 63;
    f64::from_bits(value.to_bits() ^ sign)
}

/// `exp(x)` 的多项式实现。
///
/// `exp(x) = 2^n exp(r)`，`n = round(x / ln2)`、`r ∈ [-ln2/2, ln2/2]`；
/// `2^n` 直接拼指数位，没有 2 的幂调用，也能向量化。
#[inline]
#[must_use]
pub(crate) fn exp(x: f64) -> f64 {
    let x = x.clamp(-EXP_MAX_ARG, EXP_MAX_ARG);
    let biased = x * INV_LN2 + ROUND_MAGIC;
    let n = biased - ROUND_MAGIC;
    let r = (x - n * LN2_HI) - n * LN2_LO;
    let mut polynomial = EXP_COEFFICIENTS[6];
    polynomial = polynomial * r + EXP_COEFFICIENTS[5];
    polynomial = polynomial * r + EXP_COEFFICIENTS[4];
    polynomial = polynomial * r + EXP_COEFFICIENTS[3];
    polynomial = polynomial * r + EXP_COEFFICIENTS[2];
    polynomial = polynomial * r + EXP_COEFFICIENTS[1];
    polynomial = polynomial * r + EXP_COEFFICIENTS[0];
    // `biased` 的低 52 位是 `2^51 + n`，减去偏置再加上 f64 的指数偏移。
    // NOTE: `n` 为负时低 52 位小于 `2^51`，这里必须用回绕减法，结果才是无符号的
    // `n + 1023`。
    let mantissa = biased.to_bits() & ((1 << 52) - 1);
    let exponent = mantissa.wrapping_sub(ROUND_BIAS).wrapping_add(1023);
    polynomial * f64::from_bits(exponent << 52)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// 采样一段区间，返回最大绝对误差与最大相对误差。
    fn sin_error(min: f64, max: f64, steps: usize) -> (f64, f64) {
        let mut absolute = 0.0_f64;
        let mut relative = 0.0_f64;
        for index in 0..=steps {
            let x = min + (max - min) * index as f64 / steps as f64;
            let fast = sin(x);
            let exact = x.sin();
            absolute = absolute.max((fast - exact).abs());
            if exact.abs() > 0.25 {
                relative = relative.max((fast / exact - 1.0).abs());
            }
        }
        (absolute, relative)
    }

    #[test]
    fn sin_matches_libm_over_audio_range() {
        for (min, max) in [
            (-std::f64::consts::TAU, std::f64::consts::TAU),
            (-1_000.0, 1_000.0),
            (-1.0e6, 1.0e6),
        ] {
            let (absolute, relative) = sin_error(min, max, 200_000);
            assert!(absolute < 1.0e-9, "[{min}, {max}] 绝对误差 {absolute}");
            assert!(relative < 1.0e-9, "[{min}, {max}] 相对误差 {relative}");
        }
        // 上限附近允许更大误差，但仍需明显低于 1e-5。
        let (absolute, _) = sin_error(-SIN_MAX_ARG, SIN_MAX_ARG, 200_000);
        assert!(absolute < 1.0e-5, "满量程绝对误差 {absolute}");
    }

    #[test]
    fn sin_handles_special_arguments() {
        for x in [0.0, -0.0, 12.5, -12.5, 1.0e10, -1.0e10] {
            let difference = (sin(x) - x.sin()).abs();
            assert!(difference < 1.0e-5, "x={x} 误差 {difference}");
        }
        assert!(sin(f64::NAN).is_nan());
        // NOTE: 超过上限的相位按上限处理，结果有界且可复现。
        assert_eq!(sin(1.0e300).to_bits(), sin(SIN_MAX_ARG).to_bits());
    }

    #[test]
    fn exp_matches_libm_over_audio_range() {
        for (min, max) in [(-80.0, 0.0), (-20.0, 20.0), (0.0, 700.0)] {
            let mut relative = 0.0_f64;
            for index in 0..=200_000 {
                let x = min + (max - min) * index as f64 / 200_000.0;
                let fast = exp(x);
                let exact = x.exp();
                relative = relative.max((fast / exact - 1.0).abs());
            }
            assert!(relative < 1.0e-7, "[{min}, {max}] 相对误差 {relative}");
        }
    }

    #[test]
    fn exp_handles_special_arguments() {
        assert!((exp(0.0) - 1.0).abs() < 1.0e-9);
        assert!((exp(-0.0) - 1.0).abs() < 1.0e-9);
        assert!(exp(f64::NAN).is_nan());
        // NOTE: 超出上界的参数按上界处理，结果在 `f32` 中同样是 0 或无穷。
        assert_eq!(exp(-1_000.0).to_bits(), exp(-EXP_MAX_ARG).to_bits());
        assert_eq!(exp(1_000.0).to_bits(), exp(EXP_MAX_ARG).to_bits());
    }
}
