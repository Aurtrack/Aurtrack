//! 职责：
//! - 为音频热路径提供足够精确的多项式 `sin` 与 `exp`，替代 libm 调用
//! - 在 x86_64 上提供可关闭的 AVX2 + FMA 四路内核，标量实现始终可用
//!
//! 禁止：
//! - 查表、分配、加锁或依赖任何初始化状态
//! - 让 block 与 scalar 路径使用不同的归约或多项式公式
//! - 在音频线程做 CPUID 检测或读环境变量

// PERF: 物理钢琴每个 partial 每帧要算 3 次 `sin` 和 1 次 `exp`，libm 调用
// 占离线渲染一半以上的时间。这里的实现没有函数调用、没有分支，可以被内联；
// 默认路径已由 AVX2 四路内核接管（见 ADR 0027），标量实现继续作为回退。

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
/// `ROUND_BIAS` 的 `i64` 形式，供 AVX2 的 64 位整数指令直接使用。
#[cfg(target_arch = "x86_64")]
const ROUND_BIAS_I64: i64 = 1 << 51;

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

/// 音频热路径的数学后端；在计划构建期检测并缓存，音频线程只读取结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MathBackend {
    /// 标量回退；所有平台可用，结果与既有实现逐位一致。
    #[default]
    Scalar,
    /// AVX2 + FMA 四路 f64 内核；只在 x86_64 且运行期检测通过时可用。
    #[cfg(target_arch = "x86_64")]
    Avx2Fma,
}

impl MathBackend {
    /// 控制线程调用；音频线程不得调用，因为这里会读环境变量并初始化
    /// `is_x86_feature_detected!` 的缓存。
    pub(crate) fn detect() -> Self {
        if simd_disabled(std::env::var_os("AURTRACK_SIMD").as_deref()) {
            return Self::Scalar;
        }
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2")
                && std::arch::is_x86_feature_detected!("fma")
            {
                return Self::Avx2Fma;
            }
        }
        Self::Scalar
    }

    /// 是否选择 AVX2 内核；非 x86_64 平台恒为 `false`。
    #[inline]
    #[must_use]
    pub(crate) fn is_avx2(self) -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            matches!(self, Self::Avx2Fma)
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    }
}

/// `AURTRACK_SIMD` 的关闭值；未设置或其他值都按自动检测处理。
fn simd_disabled(value: Option<&std::ffi::OsStr>) -> bool {
    let Some(value) = value.and_then(std::ffi::OsStr::to_str) else {
        return false;
    };
    ["0", "off", "false", "scalar", "none"]
        .iter()
        .any(|disabled| value.eq_ignore_ascii_case(disabled))
}

/// 帧循环里可以逐位向量化的二元算子。
#[derive(Debug, Clone, Copy)]
pub(crate) enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
}

impl BinaryOp {
    #[inline]
    fn apply(self, left: f64, right: f64) -> f64 {
        match self {
            Self::Add => left + right,
            Self::Subtract => left - right,
            Self::Multiply => left * right,
            Self::Divide => {
                if right == 0.0 {
                    0.0
                } else {
                    left / right
                }
            },
        }
    }
}

/// 二元算子的一个操作数：标量广播或逐帧数组。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Operand<'a> {
    Scalar(f64),
    Array(&'a [f64]),
}

#[cfg(target_arch = "x86_64")]
pub(crate) mod avx2 {
    use std::arch::x86_64::{
        __m256d, _CMP_EQ_OQ, _mm_storeu_ps, _mm256_add_epi64, _mm256_add_pd, _mm256_and_si256,
        _mm256_andnot_pd, _mm256_blendv_pd, _mm256_castpd_si256, _mm256_castsi256_pd,
        _mm256_cmp_pd, _mm256_cvtpd_ps, _mm256_div_pd, _mm256_fmadd_pd, _mm256_fnmadd_pd,
        _mm256_loadu_pd, _mm256_max_pd, _mm256_min_pd, _mm256_mul_pd, _mm256_set1_epi64x,
        _mm256_set1_pd, _mm256_setzero_pd, _mm256_slli_epi64, _mm256_storeu_pd, _mm256_sub_epi64,
        _mm256_sub_pd, _mm256_xor_pd,
    };

    use super::{
        BinaryOp, EXP_COEFFICIENTS, EXP_MAX_ARG, INV_LN2, INV_PI, LN2_HI, LN2_LO, Operand, PI_HI,
        PI_LO, ROUND_BIAS_I64, ROUND_MAGIC, SIN_COEFFICIENTS, SIN_MAX_ARG, exp, sin,
    };

    /// 生成“数组×数组”“标量×数组”“数组×标量”三份四路循环。
    ///
    /// `$vector` 是逐位一致的 AVX2 算术指令，`$scalar` 是尾部与回退用的
    /// 同序标量表达式。
    macro_rules! define_binary_slice {
        ($array_array:ident, $scalar_left:ident, $scalar_right:ident, $vector:ident, $scalar:expr) => {
            #[target_feature(enable = "avx2")]
            fn $array_array(left: &[f64], right: &[f64], output: &mut [f64]) {
                let len = left.len().min(right.len()).min(output.len());
                let mut index = 0;
                while index + 4 <= len {
                    // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu`
                    // 允许任意对齐，三个切片由调用方保证互不重叠。
                    unsafe {
                        let a = _mm256_loadu_pd(left.as_ptr().add(index));
                        let b = _mm256_loadu_pd(right.as_ptr().add(index));
                        _mm256_storeu_pd(output.as_mut_ptr().add(index), $vector(a, b));
                    }
                    index += 4;
                }
                for frame in index..len {
                    output[frame] = ($scalar)(left[frame], right[frame]);
                }
            }

            #[target_feature(enable = "avx2")]
            fn $scalar_left(left: f64, right: &[f64], output: &mut [f64]) {
                let len = right.len().min(output.len());
                let left_scalar = left;
                let left = _mm256_set1_pd(left);
                let mut index = 0;
                while index + 4 <= len {
                    // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu`
                    // 允许任意对齐，两个切片由调用方保证互不重叠。
                    unsafe {
                        let b = _mm256_loadu_pd(right.as_ptr().add(index));
                        _mm256_storeu_pd(output.as_mut_ptr().add(index), $vector(left, b));
                    }
                    index += 4;
                }
                for frame in index..len {
                    output[frame] = ($scalar)(left_scalar, right[frame]);
                }
            }

            #[target_feature(enable = "avx2")]
            fn $scalar_right(left: &[f64], right: f64, output: &mut [f64]) {
                let len = left.len().min(output.len());
                let right_scalar = right;
                let right = _mm256_set1_pd(right);
                let mut index = 0;
                while index + 4 <= len {
                    // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu`
                    // 允许任意对齐，两个切片由调用方保证互不重叠。
                    unsafe {
                        let a = _mm256_loadu_pd(left.as_ptr().add(index));
                        _mm256_storeu_pd(output.as_mut_ptr().add(index), $vector(a, right));
                    }
                    index += 4;
                }
                for frame in index..len {
                    output[frame] = ($scalar)(left[frame], right_scalar);
                }
            }
        };
    }

    define_binary_slice!(
        add_array_array,
        add_scalar_left,
        add_scalar_right,
        _mm256_add_pd,
        |left: f64, right: f64| left + right
    );
    define_binary_slice!(
        subtract_array_array,
        subtract_scalar_left,
        subtract_scalar_right,
        _mm256_sub_pd,
        |left: f64, right: f64| left - right
    );
    define_binary_slice!(
        multiply_array_array,
        multiply_scalar_left,
        multiply_scalar_right,
        _mm256_mul_pd,
        |left: f64, right: f64| left * right
    );

    /// 除法的零分母保护与标量语义一致：`b == 0` 时结果为 0，NaN 继续传播。
    #[target_feature(enable = "avx2")]
    fn divide_guard(value: __m256d, divisor: __m256d) -> __m256d {
        let zero = _mm256_setzero_pd();
        let zero_divisor = _mm256_cmp_pd::<_CMP_EQ_OQ>(divisor, zero);
        _mm256_blendv_pd(value, zero, zero_divisor)
    }

    macro_rules! define_divide_slice {
        ($array_array:ident, $scalar_left:ident, $scalar_right:ident) => {
            #[target_feature(enable = "avx2")]
            fn $array_array(left: &[f64], right: &[f64], output: &mut [f64]) {
                let len = left.len().min(right.len()).min(output.len());
                let mut index = 0;
                while index + 4 <= len {
                    // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu`
                    // 允许任意对齐，三个切片由调用方保证互不重叠。
                    unsafe {
                        let a = _mm256_loadu_pd(left.as_ptr().add(index));
                        let b = _mm256_loadu_pd(right.as_ptr().add(index));
                        let value = divide_guard(_mm256_div_pd(a, b), b);
                        _mm256_storeu_pd(output.as_mut_ptr().add(index), value);
                    }
                    index += 4;
                }
                for frame in index..len {
                    output[frame] = if right[frame] == 0.0 {
                        0.0
                    } else {
                        left[frame] / right[frame]
                    };
                }
            }

            #[target_feature(enable = "avx2")]
            fn $scalar_left(left: f64, right: &[f64], output: &mut [f64]) {
                let len = right.len().min(output.len());
                let left_scalar = left;
                let left = _mm256_set1_pd(left);
                let mut index = 0;
                while index + 4 <= len {
                    // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu`
                    // 允许任意对齐，两个切片由调用方保证互不重叠。
                    unsafe {
                        let b = _mm256_loadu_pd(right.as_ptr().add(index));
                        let value = divide_guard(_mm256_div_pd(left, b), b);
                        _mm256_storeu_pd(output.as_mut_ptr().add(index), value);
                    }
                    index += 4;
                }
                for frame in index..len {
                    output[frame] = if right[frame] == 0.0 {
                        0.0
                    } else {
                        left_scalar / right[frame]
                    };
                }
            }

            #[target_feature(enable = "avx2")]
            fn $scalar_right(left: &[f64], right: f64, output: &mut [f64]) {
                let len = left.len().min(output.len());
                let right_scalar = right;
                let right = _mm256_set1_pd(right);
                let mut index = 0;
                while index + 4 <= len {
                    // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu`
                    // 允许任意对齐，两个切片由调用方保证互不重叠。
                    unsafe {
                        let a = _mm256_loadu_pd(left.as_ptr().add(index));
                        let value = divide_guard(_mm256_div_pd(a, right), right);
                        _mm256_storeu_pd(output.as_mut_ptr().add(index), value);
                    }
                    index += 4;
                }
                for frame in index..len {
                    output[frame] = if right_scalar == 0.0 {
                        0.0
                    } else {
                        left[frame] / right_scalar
                    };
                }
            }
        };
    }

    define_divide_slice!(divide_array_array, divide_scalar_left, divide_scalar_right);

    /// 四路二元算术；标量操作数广播，尾部走同一公式的标量实现。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2`，且 `output` 与数组操作数不重叠。
    #[target_feature(enable = "avx2")]
    pub(crate) unsafe fn binary_slice(
        op: BinaryOp,
        left: Operand<'_>,
        right: Operand<'_>,
        output: &mut [f64],
    ) {
        match (op, left, right) {
            (BinaryOp::Add, Operand::Array(left), Operand::Array(right)) => {
                add_array_array(left, right, output);
            },
            (BinaryOp::Add, Operand::Scalar(left), Operand::Array(right)) => {
                add_scalar_left(left, right, output);
            },
            (BinaryOp::Add, Operand::Array(left), Operand::Scalar(right)) => {
                add_scalar_right(left, right, output);
            },
            (BinaryOp::Subtract, Operand::Array(left), Operand::Array(right)) => {
                subtract_array_array(left, right, output);
            },
            (BinaryOp::Subtract, Operand::Scalar(left), Operand::Array(right)) => {
                subtract_scalar_left(left, right, output);
            },
            (BinaryOp::Subtract, Operand::Array(left), Operand::Scalar(right)) => {
                subtract_scalar_right(left, right, output);
            },
            (BinaryOp::Multiply, Operand::Array(left), Operand::Array(right)) => {
                multiply_array_array(left, right, output);
            },
            (BinaryOp::Multiply, Operand::Scalar(left), Operand::Array(right)) => {
                multiply_scalar_left(left, right, output);
            },
            (BinaryOp::Multiply, Operand::Array(left), Operand::Scalar(right)) => {
                multiply_scalar_right(left, right, output);
            },
            (BinaryOp::Divide, Operand::Array(left), Operand::Array(right)) => {
                divide_array_array(left, right, output);
            },
            (BinaryOp::Divide, Operand::Scalar(left), Operand::Array(right)) => {
                divide_scalar_left(left, right, output);
            },
            (BinaryOp::Divide, Operand::Array(left), Operand::Scalar(right)) => {
                divide_scalar_right(left, right, output);
            },
            (
                BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide,
                Operand::Scalar(left),
                Operand::Scalar(right),
            ) => output.fill(op.apply(left, right)),
        }
    }

    /// `sum += addend`，逐位一致的 f64 加法。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2`，且两个切片不重叠。
    #[target_feature(enable = "avx2")]
    pub(crate) unsafe fn add_assign_slice(sum: &mut [f64], addend: &[f64]) {
        let len = sum.len().min(addend.len());
        let mut index = 0;
        while index + 4 <= len {
            // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu` 允许任意
            // 对齐，两个切片由调用方保证不重叠。
            unsafe {
                let current = _mm256_loadu_pd(sum.as_ptr().add(index));
                let addend = _mm256_loadu_pd(addend.as_ptr().add(index));
                _mm256_storeu_pd(sum.as_mut_ptr().add(index), _mm256_add_pd(current, addend));
            }
            index += 4;
        }
        for frame in index..len {
            sum[frame] += addend[frame];
        }
    }

    /// `output = input as f32`，与标量转换逐位一致。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2`，且两个切片不重叠。
    #[target_feature(enable = "avx2")]
    pub(crate) unsafe fn narrow_slice(input: &[f64], output: &mut [f32]) {
        let len = input.len().min(output.len());
        let mut index = 0;
        while index + 4 <= len {
            // SAFETY: 下标满足 `index + 4 <= len`；`loadu` 与 `storeu_ps`
            // 允许任意对齐，两个切片由调用方保证不重叠。
            unsafe {
                let values = _mm256_loadu_pd(input.as_ptr().add(index));
                let narrowed = _mm256_cvtpd_ps(values);
                _mm_storeu_ps(output.as_mut_ptr().add(index), narrowed);
            }
            index += 4;
        }
        for frame in index..len {
            output[frame] = input[frame] as f32;
        }
    }

    /// 逐段 `abs`；只清符号位，与 `f64::abs` 逐位一致。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2`，且两个切片不重叠。
    #[target_feature(enable = "avx2")]
    pub(crate) unsafe fn abs_slice(input: &[f64], output: &mut [f64]) {
        let len = input.len().min(output.len());
        let sign = _mm256_set1_pd(-0.0);
        let mut index = 0;
        while index + 4 <= len {
            // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu` 允许任意
            // 对齐，两个切片由调用方保证不重叠。
            unsafe {
                let values = _mm256_loadu_pd(input.as_ptr().add(index));
                _mm256_storeu_pd(
                    output.as_mut_ptr().add(index),
                    _mm256_andnot_pd(sign, values),
                );
            }
            index += 4;
        }
        for frame in index..len {
            output[frame] = input[frame].abs();
        }
    }

    /// 逐段 `clamp`，上下界都是标量；NaN 输入保持 NaN。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2`，且两个切片不重叠。
    #[target_feature(enable = "avx2")]
    pub(crate) unsafe fn clamp_scalar_slice(input: &[f64], min: f64, max: f64, output: &mut [f64]) {
        let len = input.len().min(output.len());
        let min_scalar = min;
        let max_scalar = max;
        let min = _mm256_set1_pd(min);
        let max = _mm256_set1_pd(max);
        let mut index = 0;
        while index + 4 <= len {
            // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu` 允许任意
            // 对齐，两个切片由调用方保证不重叠。
            unsafe {
                let values = _mm256_loadu_pd(input.as_ptr().add(index));
                let clamped = _mm256_min_pd(max, _mm256_max_pd(min, values));
                _mm256_storeu_pd(output.as_mut_ptr().add(index), clamped);
            }
            index += 4;
        }
        for frame in index..len {
            output[frame] = input[frame].clamp(min_scalar, max_scalar);
        }
    }

    /// 四路 `sin`；归约常量与标量相同，FNMADD 让 SIMD 归约更精确。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2` 与 `fma`。
    #[target_feature(enable = "avx2,fma")]
    #[inline]
    fn sin4(x: __m256d) -> __m256d {
        // NOTE: `min(hi, max(lo, x))` 的第二个操作数是 `x`：x 为 NaN 时
        // Intel 的 `max`/`min` 会原样返回 NaN，不需要额外的比较与选择。
        let clamped = _mm256_min_pd(
            _mm256_set1_pd(SIN_MAX_ARG),
            _mm256_max_pd(_mm256_set1_pd(-SIN_MAX_ARG), x),
        );
        let biased = _mm256_add_pd(
            _mm256_mul_pd(clamped, _mm256_set1_pd(INV_PI)),
            _mm256_set1_pd(ROUND_MAGIC),
        );
        let n = _mm256_sub_pd(biased, _mm256_set1_pd(ROUND_MAGIC));
        let r = _mm256_fnmadd_pd(
            n,
            _mm256_set1_pd(PI_LO),
            _mm256_fnmadd_pd(n, _mm256_set1_pd(PI_HI), clamped),
        );
        let u = _mm256_mul_pd(r, r);
        let u2 = _mm256_mul_pd(u, u);
        // NOTE: Estrin 分组与标量 Horner 是同一个多项式；FMA 只改变
        // 舍入，误差仍远低于 `1e-6`。
        let p01 = _mm256_fmadd_pd(
            _mm256_set1_pd(SIN_COEFFICIENTS[1]),
            u,
            _mm256_set1_pd(SIN_COEFFICIENTS[0]),
        );
        let p23 = _mm256_fmadd_pd(
            _mm256_set1_pd(SIN_COEFFICIENTS[3]),
            u,
            _mm256_set1_pd(SIN_COEFFICIENTS[2]),
        );
        let p45 = _mm256_fmadd_pd(
            _mm256_set1_pd(SIN_COEFFICIENTS[5]),
            u,
            _mm256_set1_pd(SIN_COEFFICIENTS[4]),
        );
        let p2345 = _mm256_fmadd_pd(p45, u2, p23);
        let polynomial = _mm256_fmadd_pd(p2345, u2, p01);
        let value = _mm256_mul_pd(r, polynomial);
        let sign = _mm256_slli_epi64(
            _mm256_and_si256(_mm256_castpd_si256(biased), _mm256_set1_epi64x(1)),
            63,
        );
        _mm256_xor_pd(value, _mm256_castsi256_pd(sign))
    }

    /// 四路 `exp`；归约常量与标量相同，FNMADD 让 SIMD 归约更精确。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2` 与 `fma`。
    #[target_feature(enable = "avx2,fma")]
    #[inline]
    fn exp4(x: __m256d) -> __m256d {
        // NOTE: 与 `sin4` 相同，把 `x` 放在 `max` 的第二个参数上以传播 NaN。
        let clamped = _mm256_min_pd(
            _mm256_set1_pd(EXP_MAX_ARG),
            _mm256_max_pd(_mm256_set1_pd(-EXP_MAX_ARG), x),
        );
        let biased = _mm256_add_pd(
            _mm256_mul_pd(clamped, _mm256_set1_pd(INV_LN2)),
            _mm256_set1_pd(ROUND_MAGIC),
        );
        let n = _mm256_sub_pd(biased, _mm256_set1_pd(ROUND_MAGIC));
        let r = _mm256_fnmadd_pd(
            n,
            _mm256_set1_pd(LN2_LO),
            _mm256_fnmadd_pd(n, _mm256_set1_pd(LN2_HI), clamped),
        );
        let r2 = _mm256_mul_pd(r, r);
        // NOTE: 与标量 Horner 同一多项式，Estrin 分组缩短依赖链。
        let p01 = _mm256_fmadd_pd(
            _mm256_set1_pd(EXP_COEFFICIENTS[1]),
            r,
            _mm256_set1_pd(EXP_COEFFICIENTS[0]),
        );
        let p23 = _mm256_fmadd_pd(
            _mm256_set1_pd(EXP_COEFFICIENTS[3]),
            r,
            _mm256_set1_pd(EXP_COEFFICIENTS[2]),
        );
        let p45 = _mm256_fmadd_pd(
            _mm256_set1_pd(EXP_COEFFICIENTS[5]),
            r,
            _mm256_set1_pd(EXP_COEFFICIENTS[4]),
        );
        let p456 = _mm256_fmadd_pd(_mm256_set1_pd(EXP_COEFFICIENTS[6]), r2, p45);
        let p23456 = _mm256_fmadd_pd(p456, r2, p23);
        let polynomial = _mm256_fmadd_pd(p23456, r2, p01);
        let mantissa = _mm256_and_si256(
            _mm256_castpd_si256(biased),
            _mm256_set1_epi64x((1_i64 << 52) - 1),
        );
        let exponent = _mm256_add_epi64(
            _mm256_sub_epi64(mantissa, _mm256_set1_epi64x(ROUND_BIAS_I64)),
            _mm256_set1_epi64x(1023),
        );
        let scale = _mm256_castsi256_pd(_mm256_slli_epi64(exponent, 52));
        _mm256_mul_pd(polynomial, scale)
    }

    /// 对整段输入做四路 `sin`；长度不是 4 的倍数时尾部走标量。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2` 与 `fma`。
    #[target_feature(enable = "avx2,fma")]
    pub(crate) unsafe fn sin_slice(input: &[f64], output: &mut [f64]) {
        let len = input.len().min(output.len());
        let mut index = 0;
        while index + 4 <= len {
            // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu` 允许任意
            // 对齐，`input` 与 `output` 是调用方保证不重叠的独立切片。
            unsafe {
                let x = _mm256_loadu_pd(input.as_ptr().add(index));
                let y = sin4(x);
                _mm256_storeu_pd(output.as_mut_ptr().add(index), y);
            }
            index += 4;
        }
        for (source, slot) in input[index..len].iter().zip(&mut output[index..len]) {
            *slot = sin(*source);
        }
    }

    /// 对整段输入做四路 `exp`；长度不是 4 的倍数时尾部走标量。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2` 与 `fma`。
    #[target_feature(enable = "avx2,fma")]
    pub(crate) unsafe fn exp_slice(input: &[f64], output: &mut [f64]) {
        let len = input.len().min(output.len());
        let mut index = 0;
        while index + 4 <= len {
            // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu` 允许任意
            // 对齐，`input` 与 `output` 是调用方保证不重叠的独立切片。
            unsafe {
                let x = _mm256_loadu_pd(input.as_ptr().add(index));
                let y = exp4(x);
                _mm256_storeu_pd(output.as_mut_ptr().add(index), y);
            }
            index += 4;
        }
        for (source, slot) in input[index..len].iter().zip(&mut output[index..len]) {
            *slot = exp(*source);
        }
    }

    /// `sin(TAU · frequency · time + phase)`，频率与相位是标量。
    ///
    /// `frequency` 由调用方预先乘好 `TAU`；角度用 FMA 计算，差异在 ulp 级。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2` 与 `fma`。
    #[target_feature(enable = "avx2,fma")]
    pub(crate) unsafe fn sine_scalar_slice(
        time: &[f64],
        frequency: f64,
        phase: f64,
        output: &mut [f64],
    ) {
        let len = time.len().min(output.len());
        let frequency_vector = _mm256_set1_pd(frequency);
        let phase_vector = _mm256_set1_pd(phase);
        let mut index = 0;
        while index + 4 <= len {
            // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu` 允许任意
            // 对齐，`time` 与 `output` 是调用方保证不重叠的独立切片。
            unsafe {
                let time = _mm256_loadu_pd(time.as_ptr().add(index));
                let angle = _mm256_fmadd_pd(frequency_vector, time, phase_vector);
                let value = sin4(angle);
                _mm256_storeu_pd(output.as_mut_ptr().add(index), value);
            }
            index += 4;
        }
        for (source, slot) in time[index..len].iter().zip(&mut output[index..len]) {
            *slot = sin(frequency * *source + phase);
        }
    }

    /// `sin(TAU · frequency · time + phase)`，频率是标量、相位逐帧。
    ///
    /// # Safety
    /// 调用方必须确认 CPU 支持 `avx2` 与 `fma`。
    #[target_feature(enable = "avx2,fma")]
    pub(crate) unsafe fn sine_scalar_phase_slice(
        time: &[f64],
        frequency: f64,
        phase: &[f64],
        output: &mut [f64],
    ) {
        let len = time.len().min(phase.len()).min(output.len());
        let frequency_vector = _mm256_set1_pd(frequency);
        let mut index = 0;
        while index + 4 <= len {
            // SAFETY: 下标满足 `index + 4 <= len`；`loadu`/`storeu` 允许任意
            // 对齐，`time`、`phase` 与 `output` 是调用方保证不重叠的独立切片。
            unsafe {
                let time = _mm256_loadu_pd(time.as_ptr().add(index));
                let phase = _mm256_loadu_pd(phase.as_ptr().add(index));
                let angle = _mm256_fmadd_pd(frequency_vector, time, phase);
                let value = sin4(angle);
                _mm256_storeu_pd(output.as_mut_ptr().add(index), value);
            }
            index += 4;
        }
        for frame in index..len {
            output[frame] = sin(frequency * time[frame] + phase[frame]);
        }
    }
}

/// 标量二元帧循环；与 AVX2 内核逐位一致，也是非 x86_64 的唯一实现。
fn scalar_binary(op: BinaryOp, left: Operand<'_>, right: Operand<'_>, output: &mut [f64]) {
    match (left, right) {
        (Operand::Scalar(left), Operand::Scalar(right)) => output.fill(op.apply(left, right)),
        (Operand::Scalar(left), Operand::Array(right)) => {
            for (frame, slot) in output.iter_mut().enumerate() {
                *slot = op.apply(left, right.get(frame).copied().unwrap_or(0.0));
            }
        },
        (Operand::Array(left), Operand::Scalar(right)) => {
            for (frame, slot) in output.iter_mut().enumerate() {
                *slot = op.apply(left.get(frame).copied().unwrap_or(0.0), right);
            }
        },
        (Operand::Array(left), Operand::Array(right)) => {
            for (frame, slot) in output.iter_mut().enumerate() {
                *slot = op.apply(
                    left.get(frame).copied().unwrap_or(0.0),
                    right.get(frame).copied().unwrap_or(0.0),
                );
            }
        },
    }
}

/// 逐段 `sin`；选择 AVX2 或标量回退，分支在循环外。
#[inline]
pub(crate) fn sin_slice(math: MathBackend, input: &[f64], output: &mut [f64]) {
    #[cfg(target_arch = "x86_64")]
    {
        if math.is_avx2() {
            // SAFETY: `Avx2Fma` 只可能由 `MathBackend::detect` 在计划构建期
            // 检测 avx2+fma 后构造；`input` 与 `output` 由调用方保证不重叠。
            unsafe { avx2::sin_slice(input, output) };
            return;
        }
    }
    let len = input.len().min(output.len());
    for (source, slot) in input[..len].iter().zip(&mut output[..len]) {
        *slot = sin(*source);
    }
}

/// 逐段 `exp`；选择 AVX2 或标量回退，分支在循环外。
#[inline]
pub(crate) fn exp_slice(math: MathBackend, input: &[f64], output: &mut [f64]) {
    #[cfg(target_arch = "x86_64")]
    {
        if math.is_avx2() {
            // SAFETY: `Avx2Fma` 只可能由 `MathBackend::detect` 在计划构建期
            // 检测 avx2+fma 后构造；`input` 与 `output` 由调用方保证不重叠。
            unsafe { avx2::exp_slice(input, output) };
            return;
        }
    }
    let len = input.len().min(output.len());
    for (source, slot) in input[..len].iter().zip(&mut output[..len]) {
        *slot = exp(*source);
    }
}

/// 逐帧二元算术；选择 AVX2 或标量回退，分支在循环外。
#[inline]
pub(crate) fn binary_slice(
    math: MathBackend,
    op: BinaryOp,
    left: Operand<'_>,
    right: Operand<'_>,
    output: &mut [f64],
) {
    #[cfg(target_arch = "x86_64")]
    {
        if math.is_avx2() {
            // SAFETY: `Avx2Fma` 只可能由 `MathBackend::detect` 在计划构建期
            // 检测 avx2+fma 后构造；`output` 与数组操作数由调用方保证不重叠。
            unsafe { avx2::binary_slice(op, left, right, output) };
            return;
        }
    }
    scalar_binary(op, left, right, output);
}

/// `sum += addend`；选择 AVX2 或标量回退，分支在循环外。
#[inline]
pub(crate) fn add_assign_slice(math: MathBackend, sum: &mut [f64], addend: &[f64]) {
    #[cfg(target_arch = "x86_64")]
    {
        if math.is_avx2() {
            // SAFETY: `Avx2Fma` 只可能由 `MathBackend::detect` 在计划构建期
            // 检测 avx2+fma 后构造；两个切片由调用方保证不重叠。
            unsafe { avx2::add_assign_slice(sum, addend) };
            return;
        }
    }
    let len = sum.len().min(addend.len());
    for frame in 0..len {
        sum[frame] += addend[frame];
    }
}

/// `output = input as f32`；选择 AVX2 或标量回退，分支在循环外。
#[inline]
pub(crate) fn narrow_slice(math: MathBackend, input: &[f64], output: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    {
        if math.is_avx2() {
            // SAFETY: `Avx2Fma` 只可能由 `MathBackend::detect` 在计划构建期
            // 检测 avx2+fma 后构造；两个切片由调用方保证不重叠。
            unsafe { avx2::narrow_slice(input, output) };
            return;
        }
    }
    let len = input.len().min(output.len());
    for frame in 0..len {
        output[frame] = input[frame] as f32;
    }
}

/// 逐段 `abs`；选择 AVX2 或标量回退，分支在循环外。
#[inline]
pub(crate) fn abs_slice(math: MathBackend, input: &[f64], output: &mut [f64]) {
    #[cfg(target_arch = "x86_64")]
    {
        if math.is_avx2() {
            // SAFETY: `Avx2Fma` 只可能由 `MathBackend::detect` 在计划构建期
            // 检测 avx2+fma 后构造；两个切片由调用方保证不重叠。
            unsafe { avx2::abs_slice(input, output) };
            return;
        }
    }
    let len = input.len().min(output.len());
    for frame in 0..len {
        output[frame] = input[frame].abs();
    }
}

/// 逐段 `clamp`；选择 AVX2 或标量回退，分支在循环外。
#[inline]
pub(crate) fn clamp_scalar_slice(
    math: MathBackend,
    input: &[f64],
    min: f64,
    max: f64,
    output: &mut [f64],
) {
    #[cfg(target_arch = "x86_64")]
    {
        if math.is_avx2() {
            // SAFETY: `Avx2Fma` 只可能由 `MathBackend::detect` 在计划构建期
            // 检测 avx2+fma 后构造；两个切片由调用方保证不重叠。
            unsafe { avx2::clamp_scalar_slice(input, min, max, output) };
            return;
        }
    }
    let len = input.len().min(output.len());
    for frame in 0..len {
        output[frame] = input[frame].clamp(min, max);
    }
}

/// 逐段 `sin(TAU · frequency · time + phase)`，频率与相位是标量。
#[inline]
pub(crate) fn sine_scalar_slice(
    math: MathBackend,
    time: &[f64],
    frequency: f64,
    phase: f64,
    output: &mut [f64],
) {
    #[cfg(target_arch = "x86_64")]
    {
        if math.is_avx2() {
            // SAFETY: `Avx2Fma` 只可能由 `MathBackend::detect` 在计划构建期
            // 检测 avx2+fma 后构造；`time` 与 `output` 由调用方保证不重叠。
            unsafe { avx2::sine_scalar_slice(time, frequency, phase, output) };
            return;
        }
    }
    let len = time.len().min(output.len());
    for (source, slot) in time[..len].iter().zip(&mut output[..len]) {
        *slot = sin(frequency * *source + phase);
    }
}

/// 逐段 `sin(TAU · frequency · time + phase)`，频率是标量、相位逐帧。
#[inline]
pub(crate) fn sine_scalar_phase_slice(
    math: MathBackend,
    time: &[f64],
    frequency: f64,
    phase: &[f64],
    output: &mut [f64],
) {
    #[cfg(target_arch = "x86_64")]
    {
        if math.is_avx2() {
            // SAFETY: `Avx2Fma` 只可能由 `MathBackend::detect` 在计划构建期
            // 检测 avx2+fma 后构造；三个切片由调用方保证不重叠。
            unsafe { avx2::sine_scalar_phase_slice(time, frequency, phase, output) };
            return;
        }
    }
    let len = time.len().min(phase.len()).min(output.len());
    for (frame, (source, slot)) in time[..len].iter().zip(output[..len].iter_mut()).enumerate() {
        *slot = sin(frequency * *source + phase[frame]);
    }
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

    #[test]
    fn simd_can_be_disabled_by_environment_value() {
        for value in ["0", "off", "OFF", "false", "False", "scalar", "none"] {
            assert!(
                simd_disabled(Some(std::ffi::OsStr::new(value))),
                "{value} 必须关闭 SIMD"
            );
        }
        for value in ["1", "on", "auto", "avx2", ""] {
            assert!(
                !simd_disabled(Some(std::ffi::OsStr::new(value))),
                "{value} 必须按自动检测处理"
            );
        }
        assert!(!simd_disabled(None));
    }

    #[cfg(target_arch = "x86_64")]
    fn avx2_available() -> bool {
        std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma")
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn detect_selects_avx2_when_available() {
        let disabled = simd_disabled(std::env::var_os("AURTRACK_SIMD").as_deref());
        assert_eq!(
            MathBackend::detect().is_avx2(),
            avx2_available() && !disabled,
            "后端必须与 CPU 特性及环境变量一致"
        );
    }

    #[cfg(target_arch = "x86_64")]
    fn samples(min: f64, max: f64, count: usize) -> Vec<f64> {
        (0..count)
            .map(|index| min + (max - min) * index as f64 / (count - 1) as f64)
            .collect()
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_sin_matches_scalar_formula() {
        if !avx2_available() {
            return;
        }
        for (min, max) in [
            (-std::f64::consts::TAU, std::f64::consts::TAU),
            (-1.0e6, 1.0e6),
            (-SIN_MAX_ARG, SIN_MAX_ARG),
        ] {
            let input = samples(min, max, 10_003);
            let mut output = vec![0.0_f64; input.len()];
            // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；
            // input/output 不重叠。
            unsafe { avx2::sin_slice(&input, &mut output) };
            let mut worst = 0.0_f64;
            for (input, fast) in input.iter().zip(&output) {
                worst = worst.max((fast - sin(*input)).abs());
                assert!(
                    (fast - sin(*input)).abs() <= 1.0e-6,
                    "x={input} simd={fast} scalar={}",
                    sin(*input)
                );
            }
            assert!(
                worst <= 1.0e-6,
                "[{min}, {max}] SIMD 与标量最大绝对差 {worst}"
            );
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_exp_matches_scalar_formula() {
        if !avx2_available() {
            return;
        }
        for (min, max) in [(-80.0, 0.0), (0.0, 700.0)] {
            let input = samples(min, max, 10_003);
            let mut output = vec![0.0_f64; input.len()];
            // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；
            // input/output 不重叠。
            unsafe { avx2::exp_slice(&input, &mut output) };
            let mut worst = 0.0_f64;
            for (input, fast) in input.iter().zip(&output) {
                let reference = exp(*input);
                let relative = (fast / reference - 1.0).abs();
                worst = worst.max(relative);
                assert!(
                    relative <= 1.0e-6,
                    "x={input} simd={fast} scalar={reference} 相对误差={relative}"
                );
            }
            assert!(
                worst <= 1.0e-6,
                "[{min}, {max}] SIMD 与标量最大相对差 {worst}"
            );
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_handles_special_arguments() {
        if !avx2_available() {
            return;
        }
        let input = [
            0.0,
            -0.0,
            12.5,
            -12.5,
            1.0e10,
            -1.0e10,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.5,
        ];
        let mut output = vec![0.0_f64; input.len()];
        // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；input/output 不重叠。
        unsafe { avx2::sin_slice(&input, &mut output) };
        for (index, x) in input.iter().enumerate() {
            if x.is_nan() {
                assert!(output[index].is_nan(), "sin({x}) 必须保持 NaN");
            } else {
                assert!(
                    (output[index] - sin(*x)).abs() <= 1.0e-6,
                    "sin({x}) simd={} scalar={}",
                    output[index],
                    sin(*x)
                );
            }
        }
        // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；input/output 不重叠。
        unsafe { avx2::exp_slice(&input, &mut output) };
        for (index, x) in input.iter().enumerate() {
            if x.is_nan() {
                assert!(output[index].is_nan(), "exp({x}) 必须保持 NaN");
            } else {
                let reference = exp(*x);
                assert!(
                    (output[index] / reference - 1.0).abs() <= 1.0e-6,
                    "exp({x}) 相对误差过大"
                );
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_sine_matches_scalar_formula() {
        if !avx2_available() {
            return;
        }
        let time = samples(0.0, 2.0, 1_001);
        let frequency = std::f64::consts::TAU * 440.0;
        let phase = 0.3;
        let mut output = vec![0.0_f64; time.len()];
        // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；各切片不重叠。
        unsafe { avx2::sine_scalar_slice(&time, frequency, phase, &mut output) };
        for (input, fast) in time.iter().zip(&output) {
            assert!(
                (fast - sin(frequency * input + phase)).abs() <= 1.0e-6,
                "t={input} simd={fast}"
            );
        }
        let phase: Vec<f64> = time.iter().map(|input| 0.1 + 0.5 * input).collect();
        // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；各切片不重叠。
        unsafe { avx2::sine_scalar_phase_slice(&time, frequency, &phase, &mut output) };
        for ((input, phase), fast) in time.iter().zip(&phase).zip(&output) {
            assert!(
                (fast - sin(frequency * input + phase)).abs() <= 1.0e-6,
                "t={input} phase={phase} simd={fast}"
            );
        }
    }

    #[cfg(target_arch = "x86_64")]
    fn scalar_binary(op: BinaryOp, left: f64, right: f64) -> f64 {
        match op {
            BinaryOp::Add => left + right,
            BinaryOp::Subtract => left - right,
            BinaryOp::Multiply => left * right,
            BinaryOp::Divide => {
                if right == 0.0 {
                    0.0
                } else {
                    left / right
                }
            },
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_binary_matches_scalar_bitwise() {
        if !avx2_available() {
            return;
        }
        let left = samples(-10.0, 10.0, 67);
        let right = samples(0.5, 5.0, 67);
        let operations = [
            BinaryOp::Add,
            BinaryOp::Subtract,
            BinaryOp::Multiply,
            BinaryOp::Divide,
        ];
        for op in operations {
            for kind in 0..3 {
                let mut output = vec![0.0_f64; left.len()];
                let (left_operand, right_operand) = match kind {
                    0 => (Operand::Array(&left), Operand::Array(&right)),
                    1 => (Operand::Scalar(2.5), Operand::Array(&right)),
                    _ => (Operand::Array(&left), Operand::Scalar(3.5)),
                };
                // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；output 与
                // 数组操作数不重叠。
                unsafe { avx2::binary_slice(op, left_operand, right_operand, &mut output) };
                for (frame, value) in output.iter().enumerate() {
                    let expected = match kind {
                        0 => scalar_binary(op, left[frame], right[frame]),
                        1 => scalar_binary(op, 2.5, right[frame]),
                        _ => scalar_binary(op, left[frame], 3.5),
                    };
                    assert_eq!(
                        value.to_bits(),
                        expected.to_bits(),
                        "kind={kind} frame={frame} simd={value} scalar={expected}"
                    );
                }
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_divide_zero_and_nan_match_scalar() {
        if !avx2_available() {
            return;
        }
        let numerator = [1.0, -1.0, 0.0, f64::NAN, 5.0, f64::INFINITY];
        let denominator = [0.0, -0.0, 0.0, 1.0, f64::NAN, 2.0];
        let mut output = [0.0_f64; 6];
        // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；output 与输入不重叠。
        unsafe {
            avx2::binary_slice(
                BinaryOp::Divide,
                Operand::Array(&numerator),
                Operand::Array(&denominator),
                &mut output,
            );
        }
        for frame in 0..numerator.len() {
            let expected = scalar_binary(BinaryOp::Divide, numerator[frame], denominator[frame]);
            if expected.is_nan() {
                assert!(output[frame].is_nan(), "frame={frame}");
            } else {
                assert_eq!(output[frame].to_bits(), expected.to_bits(), "frame={frame}");
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_clamp_matches_scalar_bits() {
        if !avx2_available() {
            return;
        }
        let input = [
            -2.0,
            -1.0,
            0.0,
            0.5,
            1.0,
            2.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        let mut output = [0.0_f64; 9];
        // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；output 与输入不重叠。
        unsafe { avx2::clamp_scalar_slice(&input, -1.0, 1.0, &mut output) };
        for (frame, value) in input.iter().enumerate() {
            let expected = value.clamp(-1.0, 1.0);
            if expected.is_nan() {
                assert!(output[frame].is_nan(), "frame={frame}");
            } else {
                assert_eq!(output[frame].to_bits(), expected.to_bits(), "frame={frame}");
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_abs_sum_and_narrow_match_scalar() {
        if !avx2_available() {
            return;
        }
        let input = [-0.0, -1.5, 0.0, 2.25, -3.0, 1.0e-30, -1.0e30];
        let mut absolute = [0.0_f64; 7];
        // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；两个切片不重叠。
        unsafe { avx2::abs_slice(&input, &mut absolute) };
        for (value, absolute) in input.iter().zip(&absolute) {
            assert_eq!(absolute.to_bits(), value.abs().to_bits());
        }
        let mut sum = [0.25_f64; 7];
        // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；两个切片不重叠。
        unsafe { avx2::add_assign_slice(&mut sum, &absolute) };
        for (frame, value) in sum.iter().enumerate() {
            assert_eq!(value.to_bits(), (0.25 + absolute[frame]).to_bits());
        }
        let narrowed: Vec<f64> = input.iter().map(|value| value * 1.0e-3).collect();
        let mut output = [0.0_f32; 7];
        // SAFETY: `avx2_available` 已确认 AVX2 与 FMA；两个切片不重叠。
        unsafe { avx2::narrow_slice(&narrowed, &mut output) };
        for (value, output) in narrowed.iter().zip(&output) {
            assert_eq!(output.to_bits(), (*value as f32).to_bits());
        }
    }
}
