//! 职责：
//! - 验证 `Playback::render` 的 block 热路径不分配
//! - 用生产同源的物理钢琴计划覆盖 8/12 音、32/64/128 同时音符
//! - 覆盖 300 个顺序音符的声部分配与离线压力基准
//!
//! 禁止：
//! - 把分配计数或计时逻辑放进生产 crate
//! - 让测试 allocator 影响其他测试二进制

#![allow(
    clippy::unwrap_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use aurtrack_command::CommandRequest;
use aurtrack_engine::{
    NotePlan, Playback, PlaybackPlan, TrackPlan, VoiceContext, VoiceNodePlan, VoiceOp, VoicePlan,
    VoiceState,
};
use aurtrack_session::Session;
use serde_json::json;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

struct CountingAllocator;

// SAFETY: 所有分支都委托给 `System`，计数器是线程局部的 `Cell<u64>`，
// 不分配、不阻塞，也不会在 allocator 内重入。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        // SAFETY: 调用方按 `GlobalAlloc` 契约传入合法 layout。
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        // SAFETY: 调用方按 `GlobalAlloc` 契约传入合法 layout。
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        // SAFETY: 调用方按 `GlobalAlloc` 契约传入合法指针与 layout。
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: 调用方按 `GlobalAlloc` 契约传入合法指针与 layout。
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn reset_allocations() {
    ALLOCATIONS.with(|count| count.set(0));
}

fn allocation_count() -> u64 {
    ALLOCATIONS.with(Cell::get)
}

/// 用命令总线搭一个与 `playback_block_benchmark` 同源的物理钢琴工程。
fn build_playback(note_count: usize) -> (Playback, u32) {
    let notes: Vec<(u16, f64, f64)> = (0..note_count)
        // NOTE: 同时音符不能随 index 递增 pitch；循环一个八度才不会越过
        // MIDI 127 上限，128 音压力测试才有意义。
        .map(|index| ((60 + index % 12) as u16, 0.0, 1.0))
        .collect();
    build_playback_with(&notes, note_count.max(8), 2.0)
}

/// 按给定的 `(pitch, start, duration)` 搭工程；`polyphony` 走命令注册表。
fn build_playback_with(
    notes: &[(u16, f64, f64)],
    polyphony: usize,
    clip_seconds: f64,
) -> (Playback, u32) {
    let mut session = Session::default();
    for request in [
        json!({ "cmd": "project.create", "args": { "name": "block-realtime" } }),
        json!({ "cmd": "track.create", "args": { "name": "piano", "source": "physical" } }),
        json!({
            "cmd": "track.polyphony",
            "args": { "track_id": "track-1", "polyphony": polyphony },
        }),
        json!({
            "cmd": "clip.create",
            "args": { "track_id": "track-1", "start": 0, "duration": clip_seconds },
        }),
    ] {
        let request: CommandRequest = serde_json::from_value(request).unwrap();
        session.execute(&request).unwrap();
    }
    for (pitch, start, duration) in notes {
        session
            .execute(&CommandRequest {
                cmd: "note.add".to_string(),
                args: json!({
                    "path": "/tracks/track-1/clips/clip-1",
                    "pitch": pitch,
                    "start": start,
                    "duration": duration,
                    "velocity": 0.8,
                }),
            })
            .unwrap();
    }
    build_playback_from_session(&session, clip_seconds)
}

fn build_playback_from_session(session: &Session, length_seconds: f64) -> (Playback, u32) {
    let project = session.project().unwrap().clone();
    let track = &project.tracks[0];
    let (frequency, adsr, graph) = track
        .playback_graph_with_buffers(
            &project.node_groups,
            &project.audio_groups,
            &project.buffers,
        )
        .unwrap();
    let notes: Vec<NotePlan> = track
        .clips
        .iter()
        .flat_map(|clip| {
            clip.notes.iter().map(|note| NotePlan {
                start: project.ticks_to_seconds(clip.start_tick + note.start_tick),
                duration: project.ticks_to_seconds(note.duration_tick),
                frequency: aurtrack_engine::midi_to_hz(note.pitch),
                velocity: note.velocity,
            })
        })
        .collect();
    let plan = PlaybackPlan {
        sample_rate: project.sample_rate,
        loop_start_seconds: 0.0,
        loop_seconds: 0.0,
        length_seconds,
        tracks: vec![TrackPlan {
            frequency,
            gain: 1.0,
            adsr: adsr.map(|settings| aurtrack_engine::AdsrPlan {
                attack: settings.attack,
                decay: settings.decay,
                sustain: settings.sustain,
                release: settings.release,
            }),
            filters: Vec::new(),
            graph: Some(graph),
            notes,
            plugin: None,
            piano: None,
        }],
    };
    let playback = Playback::new(project.sample_rate);
    playback.set_plan(plan);
    (playback, project.sample_rate)
}

fn piano_voice_stats(playback: &Playback) -> aurtrack_engine::VoiceStats {
    playback
        .plan()
        .tracks
        .first()
        .and_then(|track| track.graph.as_ref())
        .and_then(|graph| graph.pianos.first())
        .map(aurtrack_engine::PianoPlan::voice_stats)
        .unwrap_or_default()
}

#[test]
fn voice_process_block_does_not_allocate() {
    let plan = VoicePlan::new(
        vec![
            VoiceNodePlan::new(VoiceOp::Time, Vec::new()),
            VoiceNodePlan::new(VoiceOp::Constant(2.0), Vec::new()),
            VoiceNodePlan::new(VoiceOp::Multiply, vec![0, 1]),
            VoiceNodePlan::new(VoiceOp::Output, vec![2]),
        ],
        3,
    );
    let mut state = VoiceState::new(16);
    let context = VoiceContext {
        pitch: 440.0,
        velocity: 1.0,
        local_time: 0.1,
        gate: true,
        seed: 0,
        note_index: 0,
        iteration: 0,
    };
    let mut block = [0.0_f32; aurtrack_engine::DEFAULT_BLOCK_SIZE];
    reset_allocations();
    assert!(plan.process_block(&mut state, context, &[], 48_000.0, &mut block));
    let allocations = allocation_count();
    println!("voice_process_block allocations={allocations}");
    assert_eq!(allocations, 0, "VoicePlan::process_block 发生分配");
}

#[test]
fn block_render_does_not_allocate() {
    for note_count in [8, 12] {
        let (playback, _sample_rate) = build_playback(note_count);
        aurtrack_engine::reset_block_stats();
        playback.play();
        reset_allocations();
        let mut block = [0.0_f32; aurtrack_engine::DEFAULT_BLOCK_SIZE];
        // NOTE: 1000 个 block 约 1.33 秒，覆盖制音器尾音和整块静音段；
        // 修复后尾段应直接走 block 内核，不再触发逐采样回退。
        for _ in 0..1_000 {
            playback.render(&mut block);
        }
        let allocations = allocation_count();
        let stats = aurtrack_engine::block_stats();
        println!(
            "notes={note_count} allocations={allocations} track_blocks={} fallbacks={} silent={} piano={}",
            stats.track_blocks,
            stats.playback_block_fallbacks,
            stats.silent_block_fallbacks,
            stats.piano_block_fallbacks
        );
        assert_eq!(allocations, 0, "block 热路径发生分配");
        assert!(stats.track_blocks > 0, "block 路径没有被使用");
        assert_eq!(
            stats.piano_block_fallbacks, 0,
            "PianoPlan block 内核不应回退"
        );
        assert_eq!(
            stats.playback_block_fallbacks, 0,
            "尾音已静音，不应再整块回退"
        );
        assert_eq!(stats.silent_block_fallbacks, 0);
        assert!(
            playback.position_seconds() > 1.2,
            "渲染窗口必须覆盖制音器尾音段"
        );
    }
}

#[test]
#[ignore = "复音压力基准，需 release 运行"]
fn polyphony_stress_benchmark() {
    let budget = 2.0;
    for note_count in [32, 64, 128] {
        let notes: Vec<(u16, f64, f64)> = (0..note_count)
            .map(|index| ((60 + index % 12) as u16, 0.0, 1.0))
            .collect();
        let (playback, sample_rate) = build_playback_with(&notes, note_count, 2.0);
        aurtrack_engine::reset_block_stats();
        playback.play();
        let mut output = vec![0.0_f32; sample_rate as usize * 2];
        let started = Instant::now();
        playback.render(&mut output);
        let elapsed = started.elapsed();
        let peak = output
            .iter()
            .fold(0.0_f32, |peak, value| peak.max(value.abs()));
        let stats = aurtrack_engine::block_stats();
        let voices = piano_voice_stats(&playback);
        println!(
            "simultaneous={note_count} elapsed={elapsed:?} ratio={:.3} peak={peak} \
             voices={voices:?} piano_fallbacks={}",
            elapsed.as_secs_f64() / budget,
            stats.piano_block_fallbacks
        );
        assert!(peak > 1.0e-4, "压力基准不应静音，peak={peak}");
        assert_eq!(
            stats.piano_block_fallbacks, 0,
            "复音压力不允许 PianoPlan 整块回退"
        );
        assert_eq!(
            voices.voice_starts, note_count as u64,
            "每个同时音符都必须进入声部分配"
        );
        assert_eq!(voices.voice_drops, 0);
        assert_eq!(voices.voice_steals, 0, "复音上限足够时不允许抢占");
        if cfg!(not(debug_assertions)) && note_count == 64 {
            assert!(
                elapsed.as_secs_f64() <= budget,
                "64 同时音符 2 秒离线渲染 {elapsed:?} 超过实时 {budget}s"
            );
        }
    }

    // 300 个顺序音符：每个都必须被 block 处理，不能只处理前 128 个。
    let notes: Vec<(u16, f64, f64)> = (0..300_u16)
        .map(|index| (48 + index % 12, f64::from(index) * 0.002, 0.01))
        .collect();
    let (playback, sample_rate) = build_playback_with(&notes, 128, 0.8);
    aurtrack_engine::reset_block_stats();
    playback.play();
    let mut output = vec![0.0_f32; sample_rate as usize * 2];
    let started = Instant::now();
    playback.render(&mut output);
    let elapsed = started.elapsed();
    let peak = output
        .iter()
        .fold(0.0_f32, |peak, value| peak.max(value.abs()));
    let stats = aurtrack_engine::block_stats();
    let voices = piano_voice_stats(&playback);
    println!(
        "sequential=300 elapsed={elapsed:?} peak={peak} voices={voices:?} \
         piano_fallbacks={}",
        stats.piano_block_fallbacks
    );
    assert!(peak > 1.0e-4, "300 顺序音符不应静音");
    assert_eq!(stats.piano_block_fallbacks, 0);
    assert_eq!(voices.voice_starts, 300, "300 个顺序音符必须全部经过分配");
    assert_eq!(voices.voice_drops, 0, "顺序音符不允许丢音");
}

#[test]
#[ignore = "离线 deadline 基准，需 release 运行"]
fn block_deadline_report() {
    const ROUNDS: usize = 3;
    for note_count in [8, 12] {
        // NOTE: 先单独构建一次取得采样率；测量窗口内不包含计划构建。
        let (_, sample_rate) = build_playback(note_count);
        let block_size = aurtrack_engine::DEFAULT_BLOCK_SIZE;
        let blocks_per_round = (f64::from(sample_rate) * 2.0 / block_size as f64) as usize;
        let mut timings = Vec::with_capacity(blocks_per_round * ROUNDS);
        let mut fallback_flags = Vec::with_capacity(blocks_per_round * ROUNDS);
        let mut total = 0.0_f64;
        let mut fallback_count = 0_usize;
        let mut fallback_first_ms = None;
        let mut fallback_last_ms = None;
        let mut track_blocks = 0_u64;
        let mut block = [0.0_f32; aurtrack_engine::DEFAULT_BLOCK_SIZE];
        for _ in 0..ROUNDS {
            let (playback, _) = build_playback(note_count);
            aurtrack_engine::reset_block_stats();
            playback.play();
            for index in 0..blocks_per_round {
                let before = aurtrack_engine::block_stats().playback_block_fallbacks;
                let started = Instant::now();
                playback.render(&mut block);
                let elapsed = started.elapsed().as_secs_f64() * 1_000_000.0;
                let after = aurtrack_engine::block_stats().playback_block_fallbacks;
                let fallback = after > before;
                if fallback {
                    fallback_count += 1;
                    let at_ms = index as f64 * block_size as f64 / f64::from(sample_rate) * 1_000.0;
                    fallback_first_ms.get_or_insert(at_ms);
                    fallback_last_ms = Some(at_ms);
                }
                timings.push(elapsed);
                fallback_flags.push(fallback);
                total += elapsed;
            }
            track_blocks += aurtrack_engine::block_stats().track_blocks;
        }
        let mut sorted = timings.clone();
        sorted.sort_by(f64::total_cmp);
        let max = sorted.last().copied().unwrap_or(0.0);
        let p99 = sorted[(sorted.len() * 99) / 100];
        let over_1ms = sorted.iter().filter(|value| **value > 1_000.0).count();
        let over_5ms = sorted.iter().filter(|value| **value > 5_000.0).count();
        let non_fallback: Vec<f64> = timings
            .iter()
            .zip(&fallback_flags)
            .filter(|(_, fallback)| !**fallback)
            .map(|(value, _)| *value)
            .collect();
        let mut non_fallback_sorted = non_fallback;
        non_fallback_sorted.sort_by(f64::total_cmp);
        let non_fallback_max = non_fallback_sorted.last().copied().unwrap_or(0.0);
        let non_fallback_p99 = non_fallback_sorted
            .get((non_fallback_sorted.len() * 99) / 100)
            .copied()
            .unwrap_or(0.0);
        let budget_us = 64.0 / f64::from(sample_rate) * 1_000_000.0;
        let over_budget = sorted.iter().filter(|value| **value > budget_us).count();
        println!(
            "notes={note_count} rounds={ROUNDS} blocks={} total_ms={:.1} budget_us={budget_us:.1} \
             max_us={max:.1} p99_us={p99:.1} over_1ms={over_1ms} over_5ms={over_5ms} \
             block_max_us={non_fallback_max:.1} block_p99_us={non_fallback_p99:.1} \
             fallback_blocks={fallback_count} fallback_first_ms={fallback_first_ms:?} \
             fallback_last_ms={fallback_last_ms:?} track_blocks={track_blocks}",
            timings.len(),
            total / 1_000.0,
        );
        if cfg!(not(debug_assertions)) {
            // NOTE: 制音器尾音已被静音保护识别，整条回调路径必须守住 1.333ms；
            // 这里不再给已知超支开例外。
            assert!(
                fallback_count == 0,
                "物理钢琴尾段仍有 {fallback_count} 个整块回退 block"
            );
            assert!(
                p99 <= budget_us,
                "整体 P99 {p99:.1}us 超过预算 {budget_us:.1}us"
            );
            // NOTE: 离线测试会受系统调度抖动影响，允许极少数孤立尖峰；
            // 持续超预算或超过 5ms 仍必须失败。
            assert!(
                over_budget <= 2,
                "整体有 {over_budget} 个 block 超过预算 {budget_us:.1}us，最大 {max:.1}us"
            );
            assert!(
                over_5ms == 0,
                "整体有 {over_5ms} 个 block 超过 5ms，最大 {max:.1}us"
            );
        }
    }
}
