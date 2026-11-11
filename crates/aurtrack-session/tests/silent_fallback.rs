//! 职责：
//! - 验证 block 静音保护按真实包络尾音收敛
//! - 验证无法推导尾音的声部仍保留保守回退
//!
//! 禁止：
//! - 用生产专用的测试分支伪造静音或回退
//! - 在音频线程做分配、加锁或日志

use aurtrack_engine::{
    GraphNodePlan, GraphOp, GraphPlan, GraphStateCell, NotePlan, PianoPlan, Playback, PlaybackPlan,
    TrackPlan, VoiceNodePlan, VoiceOp, VoicePlan, VoiceState,
};

/// 合成一个 block 内核恒为静音的制音器声部：用来验证静音保护仍会触发。
fn silent_damper_voice() -> VoicePlan {
    VoicePlan::new(
        vec![
            VoiceNodePlan::new(VoiceOp::Constant(0.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Gate, Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(0.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(0.0001), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Damper, vec![0, 1, 2, 3]),
        ],
        4,
    )
}

/// 没有可证明包络的静音声部：必须保留 0.5 秒保守尾窗。
fn silent_plain_voice() -> VoicePlan {
    VoicePlan::new(
        vec![VoiceNodePlan::new(VoiceOp::Constant(0.0), Vec::new())],
        0,
    )
}

fn silent_playback(voice: VoicePlan, residual: f64) -> Playback {
    let mut piano = PianoPlan::new(0, 1.0, 0.5, 0.2);
    piano.voice = Some(voice);
    piano.set_states(
        (0..piano.state_count())
            .map(|_| VoiceState::new(0))
            .collect(),
    );
    let mut nodes = vec![GraphNodePlan {
        op: GraphOp::Voice { index: 0 },
        inputs: Vec::new(),
        controls: Vec::new(),
    }];
    let output = if residual == 0.0 {
        0
    } else {
        // NOTE: 用图级常量模拟制音器之后的音板余振：它低于静音阈值，
        // 但 block 内核仍会把它算进混音。
        nodes.push(GraphNodePlan {
            op: GraphOp::Constant(residual),
            inputs: Vec::new(),
            controls: Vec::new(),
        });
        nodes.push(GraphNodePlan {
            op: GraphOp::Add,
            inputs: Vec::new(),
            controls: vec![0, 1],
        });
        2
    };
    let graph = GraphPlan {
        nodes,
        output,
        plugin: None,
        pianos: vec![piano],
        fallback_sine: None,
        state: GraphStateCell::new(),
    };
    let plan = PlaybackPlan {
        sample_rate: 48_000,
        loop_start_seconds: 0.0,
        loop_seconds: 0.0,
        length_seconds: 2.0,
        tracks: vec![TrackPlan {
            frequency: None,
            gain: 1.0,
            adsr: None,
            filters: Vec::new(),
            graph: Some(graph),
            notes: vec![NotePlan {
                start: 0.0,
                duration: 1.0,
                frequency: 440.0,
                velocity: 1.0,
            }],
            plugin: None,
            piano: None,
        }],
    };
    let playback = Playback::new(48_000);
    playback.set_plan(plan);
    playback
}

/// 跳到目标位置后渲染一个 block，返回回退计数和推进后的位置。
fn render_silent_at(voice: VoicePlan, seconds: f64) -> (u64, u64, u64, f64) {
    let playback = silent_playback(voice, 0.0);
    playback.seek(seconds);
    playback.play();
    aurtrack_engine::reset_block_stats();
    let mut block = [0.0_f32; aurtrack_engine::DEFAULT_BLOCK_SIZE];
    playback.render(&mut block);
    assert!(
        block.iter().all(|value| *value == 0.0),
        "合成声部必须保持静音"
    );
    let stats = aurtrack_engine::block_stats();
    (
        stats.playback_block_fallbacks,
        stats.silent_block_fallbacks,
        stats.piano_block_fallbacks,
        playback.position_seconds(),
    )
}

/// 回退计数是进程级原子量，所有用例放在同一个测试里避免并行干扰。
#[test]
fn silent_block_fallback_tracks_actual_tail() {
    // 音符 0–1 秒；制音器每采样衰减 0.0001，尾音上界约 0.2083 秒。
    let (playback, silent, piano, _) = render_silent_at(silent_damper_voice(), 0.5);
    assert_eq!(
        (playback, silent, piano),
        (1, 1, 0),
        "音符仍在发声时必须回退"
    );

    let (playback, silent, _, _) = render_silent_at(silent_damper_voice(), 1.05);
    assert_eq!(
        (playback, silent),
        (1, 1),
        "gate 关闭但仍在制音器尾窗内时必须回退"
    );

    let (playback, silent, _, position) = render_silent_at(silent_damper_voice(), 1.25);
    assert_eq!((playback, silent), (0, 0), "超过真实尾音后不应再整块回退");
    assert!(
        (position - (1.25 + 64.0 / 48_000.0)).abs() < 1.0e-9,
        "跳过的静音 block 仍必须正常推进"
    );

    let (playback, silent, _, _) = render_silent_at(silent_plain_voice(), 1.3);
    assert_eq!(
        (playback, silent),
        (1, 1),
        "无法证明归零的声部必须保留保守尾窗"
    );

    let (playback, silent, _, _) = render_silent_at(silent_plain_voice(), 1.6);
    assert_eq!((playback, silent), (0, 0), "保守尾窗结束后必须停止回退");

    // 制音器之后仍有低于阈值的余振：必须直接输出 0，不再回退。
    let playback = silent_playback(silent_damper_voice(), 1.0e-7);
    playback.seek(1.25);
    playback.play();
    aurtrack_engine::reset_block_stats();
    let mut block = [0.0_f32; aurtrack_engine::DEFAULT_BLOCK_SIZE];
    for _ in 0..8 {
        playback.render(&mut block);
    }
    let stats = aurtrack_engine::block_stats();
    assert_eq!(stats.playback_block_fallbacks, 0);
    assert_eq!(stats.silent_block_fallbacks, 0);
    assert!(
        block.iter().all(|value| *value == 0.0),
        "静音尾段必须直接输出 0"
    );
}
