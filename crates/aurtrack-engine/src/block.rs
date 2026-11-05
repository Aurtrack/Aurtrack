//! 职责：
//! - 定义 block 处理上下文和节点 kernel 边界
//! - 承载编译期分配的 block scratch
//!
//! 禁止：
//! - 在音频线程分配
//! - 在 kernel 内加锁或打日志

use crate::VoiceContext;
use crate::ssa::{BlockScratch, VoiceSsa};

/// 一个 block 的时间上下文。
#[derive(Debug, Clone, Copy)]
pub struct BlockContext {
    pub start_time: f64,
    pub frames: usize,
    pub sample_rate: f64,
}

impl BlockContext {
    pub fn new(start_time: f64, frames: usize, sample_rate: f64) -> Self {
        Self {
            start_time,
            frames,
            sample_rate,
        }
    }

    pub fn voice_context(&self) -> VoiceContext {
        VoiceContext {
            pitch: 0.0,
            velocity: 1.0,
            local_time: self.start_time,
            gate: true,
            seed: 0,
            note_index: 0,
            iteration: 0,
        }
    }
}

/// 节点 block 内核；输入是同一 block 内的外部值。
pub trait BlockKernel {
    fn process(&mut self, context: BlockContext, inputs: &[f64], output: &mut [f32]);
}

/// 无状态 SSA 程序的 block kernel。
#[derive(Debug)]
pub struct SsaKernel {
    program: VoiceSsa,
    scratch: BlockScratch,
}

impl SsaKernel {
    pub fn new(program: VoiceSsa) -> Self {
        let scratch = BlockScratch::new(program.instruction_count());
        Self { program, scratch }
    }
}

impl BlockKernel for SsaKernel {
    fn process(&mut self, context: BlockContext, inputs: &[f64], output: &mut [f32]) {
        let mut voice = context.voice_context();
        voice.local_time = context.start_time;
        self.program.process_block(
            voice,
            inputs,
            context.sample_rate,
            &mut self.scratch,
            output,
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{VoiceNodePlan, VoiceOp, VoicePlan};

    #[test]
    fn ssa_kernel_processes_a_block() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(VoiceOp::Time, Vec::new()),
                VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
                VoiceNodePlan::new(VoiceOp::Multiply, vec![0, 1]),
                VoiceNodePlan::new(VoiceOp::Output, vec![2]),
            ],
            3,
        );
        let program = VoiceSsa::compile(&plan).unwrap();
        let mut kernel = SsaKernel::new(program);
        let mut output = [0.0_f32; 64];
        kernel.process(BlockContext::new(0.1, 64, 48_000.0), &[], &mut output);
        assert!((f64::from(output[0]) - 0.2).abs() < 1e-6);
        assert!(output[1] > output[0]);
    }
}
