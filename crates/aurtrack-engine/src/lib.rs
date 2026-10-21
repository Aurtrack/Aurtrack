//! 职责：
//! - 维护实时播放计划与走带状态
//! - 在音频回调里按块渲染，不分配、不加锁
//!
//! 禁止：
//! - 依赖会话、IO 或 UI
//! - 在 `render` 里分配、加锁、日志

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use arc_swap::ArcSwap;

/// 音符首尾的去咔嗒微包络长度。
pub const NOTE_FADE_SECONDS: f64 = 0.005;

const DECLICK_FADE_SECONDS: f64 = 0.003;
const NO_SEEK: u64 = u64::MAX;
const FADE_IDLE: u8 = 0;
const FADE_OUT: u8 = 1;
const FADE_IN: u8 = 2;

/// 一条轨道的播放参数。
#[derive(Debug, Clone)]
pub struct TrackPlan {
    /// `None` 表示静音轨道。
    pub frequency: Option<f64>,
    pub gain: f64,
    pub notes: Vec<NotePlan>,
}

/// 一个已排入时间线的音符。
#[derive(Debug, Clone)]
pub struct NotePlan {
    pub start: f64,
    pub duration: f64,
    pub frequency: f64,
    pub velocity: f64,
}

/// MIDI 音高转频率，A4 = 69 = 440 Hz。
pub fn midi_to_hz(pitch: u8) -> f64 {
    440.0 * 2_f64.powf((f64::from(pitch) - 69.0) / 12.0)
}

/// 音符内部相对时间到振幅包络，首尾各 5 毫秒线性淡入淡出。
pub fn note_envelope(local: f64, duration: f64) -> f64 {
    let attack = (local / NOTE_FADE_SECONDS).clamp(0.0, 1.0);
    let release = ((duration - local) / NOTE_FADE_SECONDS).clamp(0.0, 1.0);
    attack.min(release)
}

/// 一次播放使用的不可变计划。
#[derive(Debug, Clone)]
pub struct PlaybackPlan {
    pub sample_rate: u32,
    pub loop_seconds: f64,
    pub tracks: Vec<TrackPlan>,
}

impl PlaybackPlan {
    pub fn silent(sample_rate: u32, loop_seconds: f64) -> Self {
        Self {
            sample_rate,
            loop_seconds,
            tracks: Vec::new(),
        }
    }
}

/// 实时走带状态。音频回调只调用 `render`。
#[derive(Debug)]
pub struct Playback {
    plan: ArcSwap<PlaybackPlan>,
    playing: AtomicBool,
    stop_requested: AtomicBool,
    position: AtomicU64,
    master_gain: AtomicU64,
    transport_gain: AtomicU64,
    seek_target: AtomicU64,
    fade_target: AtomicU64,
    seek_stage: AtomicU8,
    fade_elapsed: AtomicU64,
}

impl Playback {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            plan: ArcSwap::from_pointee(PlaybackPlan::silent(sample_rate, 5.0)),
            playing: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            position: AtomicU64::new(0),
            master_gain: AtomicU64::new(1.0_f64.to_bits()),
            transport_gain: AtomicU64::new(0.0_f64.to_bits()),
            seek_target: AtomicU64::new(NO_SEEK),
            fade_target: AtomicU64::new(NO_SEEK),
            seek_stage: AtomicU8::new(FADE_IDLE),
            fade_elapsed: AtomicU64::new(0),
        }
    }

    pub fn set_plan(&self, plan: PlaybackPlan) {
        self.plan.store(Arc::new(plan));
    }

    pub fn play(&self) {
        let was_stopping = self.stop_requested.swap(false, Ordering::AcqRel);
        let was_playing = self.playing.swap(true, Ordering::AcqRel);
        if !was_playing || was_stopping {
            self.transport_gain
                .store(0.0_f64.to_bits(), Ordering::Release);
        }
    }

    pub fn stop(&self) {
        if self.playing.load(Ordering::Acquire) {
            self.stop_requested.store(true, Ordering::Release);
        }
        self.seek_target.store(NO_SEEK, Ordering::Release);
        self.fade_target.store(NO_SEEK, Ordering::Release);
        self.seek_stage.store(FADE_IDLE, Ordering::Release);
        self.fade_elapsed.store(0, Ordering::Release);
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire) && !self.stop_requested.load(Ordering::Acquire)
    }

    pub fn seek(&self, seconds: f64) {
        let plan = self.plan.load();
        let loop_seconds = plan.loop_seconds.max(0.001);
        let wrapped = seconds.rem_euclid(loop_seconds);
        let frames = (wrapped * f64::from(plan.sample_rate)).round() as u64;
        if !self.playing.load(Ordering::Acquire) {
            self.position.store(frames, Ordering::Release);
            self.seek_target.store(NO_SEEK, Ordering::Release);
            self.fade_target.store(NO_SEEK, Ordering::Release);
            self.seek_stage.store(FADE_IDLE, Ordering::Release);
            self.fade_elapsed.store(0, Ordering::Release);
            return;
        }
        self.seek_target.store(frames, Ordering::Release);
        if self.seek_stage.load(Ordering::Acquire) == FADE_IDLE {
            self.fade_elapsed.store(0, Ordering::Release);
            self.seek_stage.store(FADE_OUT, Ordering::Release);
        }
    }

    pub fn position_seconds(&self) -> f64 {
        let plan = self.plan.load();
        if plan.sample_rate == 0 {
            return 0.0;
        }
        let target = self.seek_target.load(Ordering::Acquire);
        if self.seek_stage.load(Ordering::Acquire) != FADE_IDLE && target != NO_SEEK {
            return target as f64 / f64::from(plan.sample_rate);
        }
        self.position.load(Ordering::Acquire) as f64 / f64::from(plan.sample_rate)
    }

    pub fn set_master_gain(&self, gain: f64) {
        if gain.is_finite() {
            self.master_gain
                .store(gain.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// 由音频回调调用。只做算术，不分配、不加锁。
    pub fn render(&self, out: &mut [f32]) {
        if !self.playing.load(Ordering::Acquire) {
            out.fill(0.0);
            return;
        }

        let plan = self.plan.load();
        let sample_rate = f64::from(plan.sample_rate).max(1.0);
        let loop_frames = (plan.loop_seconds * sample_rate).round().max(1.0) as u64;
        let fade_frames = (DECLICK_FADE_SECONDS * sample_rate).round().max(1.0) as u64;
        let loop_fade = if loop_frames > fade_frames * 2 {
            fade_frames
        } else {
            0
        };
        let loop_start = loop_fade;
        let master_gain = f64::from_bits(self.master_gain.load(Ordering::Relaxed));
        let stop_requested = self.stop_requested.load(Ordering::Acquire);
        let target_gain = if stop_requested { 0.0 } else { 1.0 };
        let gain_step = 1.0 / fade_frames as f64;
        let mut gain = f64::from_bits(self.transport_gain.load(Ordering::Relaxed));
        let mut position = self.position.load(Ordering::Relaxed);
        let mut stage = self.seek_stage.load(Ordering::Acquire);
        let mut target = if stage == FADE_IN {
            self.fade_target.load(Ordering::Acquire)
        } else {
            self.seek_target.load(Ordering::Acquire)
        };
        let mut elapsed = self.fade_elapsed.load(Ordering::Relaxed);

        if stop_requested && gain <= 0.0 {
            self.playing.store(false, Ordering::Release);
            self.stop_requested.store(false, Ordering::Release);
            out.fill(0.0);
            return;
        }

        if stage == FADE_IDLE && target != NO_SEEK {
            stage = FADE_OUT;
            elapsed = 0;
        } else if stage == FADE_OUT && target == NO_SEEK {
            stage = FADE_IDLE;
            elapsed = 0;
        }

        for (index, slot) in out.iter_mut().enumerate() {
            let seek_gain = if stage == FADE_OUT {
                Some(1.0 - (elapsed + 1) as f64 / fade_frames as f64)
            } else if stage == FADE_IN {
                Some((elapsed + 1) as f64 / fade_frames as f64)
            } else {
                None
            };
            let sample =
                Self::sample_with_loop(&plan, position, loop_frames, loop_fade, master_gain);
            *slot = (sample * seek_gain.unwrap_or(1.0) * gain) as f32;

            if seek_gain.is_some() {
                elapsed += 1;
                if elapsed >= fade_frames {
                    if stage == FADE_OUT {
                        target = self.seek_target.load(Ordering::Acquire);
                        if target == NO_SEEK {
                            stage = FADE_IDLE;
                            elapsed = 0;
                        } else {
                            position = target;
                            self.fade_target.store(target, Ordering::Release);
                            stage = FADE_IN;
                            elapsed = 0;
                        }
                    } else {
                        stage = FADE_IDLE;
                        elapsed = 0;
                        let latest = self.seek_target.load(Ordering::Acquire);
                        if latest != NO_SEEK && latest != target {
                            target = latest;
                            stage = FADE_OUT;
                        } else {
                            target = NO_SEEK;
                            self.seek_target.store(NO_SEEK, Ordering::Release);
                            self.fade_target.store(NO_SEEK, Ordering::Release);
                        }
                    }
                }
            }

            if stop_requested {
                gain = (gain - gain_step).max(0.0);
                if gain <= 0.0 {
                    self.playing.store(false, Ordering::Release);
                    self.stop_requested.store(false, Ordering::Release);
                    out[index + 1..].fill(0.0);
                    break;
                }
            } else if gain < target_gain {
                gain = (gain + gain_step).min(target_gain);
            }
            position = advance(position, loop_frames, loop_start);
        }

        self.position.store(position, Ordering::Release);
        self.transport_gain.store(gain.to_bits(), Ordering::Release);
        if stage != FADE_IDLE {
            self.seek_stage.store(stage, Ordering::Release);
            self.fade_elapsed.store(elapsed, Ordering::Release);
        } else if self.seek_stage.load(Ordering::Acquire) == FADE_IDLE {
            self.fade_elapsed.store(0, Ordering::Release);
        }
    }

    fn sample_with_loop(
        plan: &PlaybackPlan,
        position: u64,
        loop_frames: u64,
        loop_fade: u64,
        master_gain: f64,
    ) -> f64 {
        if loop_fade == 0 {
            return Self::sample_at(plan, position, master_gain);
        }
        let start = loop_frames.saturating_sub(loop_fade);
        if position < start {
            return Self::sample_at(plan, position, master_gain);
        }
        let offset = position - start;
        let blend = (offset + 1) as f64 / loop_fade as f64;
        let tail = Self::sample_at(plan, position, master_gain);
        let head = Self::sample_at(plan, offset, master_gain);
        tail * (1.0 - blend) + head * blend
    }

    fn sample_at(plan: &PlaybackPlan, position: u64, master_gain: f64) -> f64 {
        let sample_rate = f64::from(plan.sample_rate).max(1.0);
        let time = position as f64 / sample_rate;
        let mut sample = 0.0;
        for track in &plan.tracks {
            let mut voice = 0.0;
            if let Some(frequency) = track.frequency {
                voice += (std::f64::consts::TAU * frequency * time).sin() * 0.25;
            }
            for note in &track.notes {
                if time >= note.start && time < note.start + note.duration {
                    let local = time - note.start;
                    voice += (std::f64::consts::TAU * note.frequency * local).sin()
                        * note.velocity
                        * note_envelope(local, note.duration)
                        * 0.25;
                }
            }
            sample += voice * track.gain;
        }
        (sample * master_gain).clamp(-1.0, 1.0)
    }
}

fn advance(position: u64, loop_frames: u64, loop_start: u64) -> u64 {
    let next = position + 1;
    if next >= loop_frames {
        loop_start
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> PlaybackPlan {
        PlaybackPlan {
            sample_rate: 48_000,
            loop_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: Some(440.0),
                gain: 1.0,
                notes: Vec::new(),
            }],
        }
    }

    #[test]
    fn render_produces_signal_and_advances_position() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().any(|sample| sample.abs() > 0.0));
        assert!(playback.position_seconds() > 0.0);
    }

    #[test]
    fn stop_fades_then_outputs_silence() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.play();
        playback.stop();
        assert!(!playback.is_playing());
        let mut buffer = [1.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer[200..].iter().all(|sample| *sample == 0.0));
        let mut silent = [1.0_f32; 480];
        playback.render(&mut silent);
        assert!(silent.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn play_fades_in() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.seek(1.0);
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer[0].abs() < 1e-12);
        assert!(buffer[300..].iter().any(|sample| sample.abs() > 0.0));
    }

    #[test]
    fn loop_seam_crossfades() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_seconds: 0.1,
            tracks: vec![TrackPlan {
                frequency: Some(331.0),
                gain: 1.0,
                notes: Vec::new(),
            }],
        });
        playback.play();
        let mut buffer = vec![0.0_f32; 6_000];
        playback.render(&mut buffer);
        let max_delta = buffer
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_delta < 0.03, "loop step too large: {max_delta}");
    }

    #[test]
    fn master_gain_zero_mutes() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.set_master_gain(0.0);
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn seek_wraps_into_loop() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.seek(2.5);
        assert!((playback.position_seconds() - 2.5).abs() < 0.001);
        playback.seek(6.0);
        assert!((playback.position_seconds() - 1.0).abs() < 0.001);
    }

    #[test]
    fn note_plan_produces_signal() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                notes: vec![NotePlan {
                    start: 0.0,
                    duration: 1.0,
                    frequency: midi_to_hz(69),
                    velocity: 1.0,
                }],
            }],
        });
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().any(|sample| sample.abs() > 0.0));
    }

    #[test]
    fn note_envelope_has_soft_edges() {
        assert!(note_envelope(0.0, 1.0).abs() < 1e-12);
        assert!(note_envelope(0.002, 1.0) > 0.0);
        assert!(note_envelope(0.002, 1.0) < 0.5);
        assert!((note_envelope(0.5, 1.0) - 1.0).abs() < 1e-12);
        assert!(note_envelope(1.0, 1.0).abs() < 1e-12);
    }

    #[test]
    fn note_tail_is_declicked() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                notes: vec![NotePlan {
                    start: 0.0,
                    duration: 0.25,
                    frequency: midi_to_hz(72),
                    velocity: 1.0,
                }],
            }],
        });
        playback.play();
        let mut buffer = vec![0.0_f32; 12_000];
        playback.render(&mut buffer);
        assert!(buffer[0].abs() < 1e-9);
        assert!(buffer[11_999].abs() < 0.005);
    }

    #[test]
    fn seek_crossfades_instead_of_stepping() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.play();
        let mut before = [0.0_f32; 480];
        playback.render(&mut before);
        playback.seek(1.234);
        let mut buffer = [0.0_f32; 960];
        playback.render(&mut buffer);
        let mut max_delta = (buffer[0] - before[479]).abs();
        for pair in buffer.windows(2) {
            max_delta = max_delta.max((pair[1] - pair[0]).abs());
        }
        assert!(max_delta < 0.03, "seek step too large: {max_delta}");
        assert!(playback.position_seconds() > 1.23);
    }

    #[test]
    fn seek_during_fade_in_is_not_lost() {
        let playback = Playback::new(48_000);
        playback.set_plan(plan());
        playback.play();
        let mut warmup = [0.0_f32; 100];
        playback.render(&mut warmup);
        playback.seek(1.0);
        let mut first = [0.0_f32; 100];
        playback.render(&mut first);
        let mut second = [0.0_f32; 100];
        playback.render(&mut second);
        playback.seek(2.0);
        let mut third = [0.0_f32; 400];
        playback.render(&mut third);
        assert!((playback.position_seconds() - 2.0).abs() < 0.03);
    }

    #[test]
    fn output_is_clamped() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_seconds: 5.0,
            tracks: (0..8)
                .map(|_| TrackPlan {
                    frequency: Some(440.0),
                    gain: 1.0,
                    notes: Vec::new(),
                })
                .collect(),
        });
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().all(|sample| sample.abs() <= 1.0));
    }

    #[test]
    fn note_outside_window_is_silent() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                notes: vec![NotePlan {
                    start: 2.0,
                    duration: 1.0,
                    frequency: midi_to_hz(69),
                    velocity: 1.0,
                }],
            }],
        });
        playback.play();
        let mut buffer = [0.0_f32; 480];
        playback.render(&mut buffer);
        assert!(buffer.iter().all(|sample| *sample == 0.0));
    }
}
