//! 职责：
//! - 验证逐采样路径不会让相隔状态容量的两个音符共享声部状态
//!
//! 禁止：
//! - 依赖引擎私有字段或测试专用分支
//! - 在音频线程做分配、加锁或日志

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use aurtrack_engine::{NotePlan, PianoPlan, VoiceNodePlan, VoiceOp, VoicePlan, VoiceState};

fn damper_piano(damping: f64, polyphony: usize) -> PianoPlan {
    let voice = VoicePlan::new(
        vec![
            VoiceNodePlan::new(VoiceOp::Constant(1.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Gate, Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(0.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(damping), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Damper, vec![0, 1, 2, 3]),
            VoiceNodePlan::new(VoiceOp::Output, vec![4]),
        ],
        5,
    );
    let mut piano = PianoPlan::new(0, 1.0, 0.5, 0.2);
    piano.voice = Some(voice);
    piano.set_polyphony(polyphony);
    piano.set_states(
        (0..piano.state_count())
            .map(|_| VoiceState::new(0))
            .collect(),
    );
    piano
}

/// 第 129 个音符开始时，第 1 个音符仍在制音器尾窗内；两者不能共享状态。
#[test]
fn scalar_note_129_does_not_reuse_note_1_damper_state() {
    let sample_rate = 48_000.0;
    let mut notes: Vec<NotePlan> = (0..129)
        .map(|_| NotePlan {
            start: 0.0,
            duration: 0.0,
            frequency: 220.0,
            velocity: 0.0,
        })
        .collect();
    notes[0] = NotePlan {
        start: 0.0,
        duration: 0.02,
        frequency: 220.0,
        velocity: 0.8,
    };
    notes[128] = NotePlan {
        start: 0.128,
        duration: 0.02,
        frequency: 440.0,
        velocity: 0.8,
    };
    let piano = damper_piano(0.00001, 128);
    let onset = (0.128 * sample_rate) as usize;
    let mut value = 0.0;
    for frame in 0..=onset {
        value = piano.sample(&notes, frame as f64 / sample_rate);
    }
    // 第 1 个音符在 0.128 秒时电平约 0.482（damping 被钳到 0.0001）；
    // 第 129 个音符必须从 0.01 起算，两条声部相加约 0.0885。
    assert!(
        (0.08..0.10).contains(&value),
        "第 129 个音符与第 1 个共享了 Damper 状态：{value}"
    );
}
