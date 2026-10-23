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
    pub adsr: Option<AdsrPlan>,
    pub filters: Vec<FilterPlan>,
    pub graph: Option<GraphPlan>,
    pub notes: Vec<NotePlan>,
    pub plugin: Option<PluginPlan>,
    pub piano: Option<PianoPlan>,
}

/// 单个音符的 Voice 上下文。
#[derive(Debug, Clone, Copy)]
pub struct VoiceContext {
    pub pitch: f64,
    pub velocity: f64,
    pub local_time: f64,
    pub gate: bool,
    pub seed: u64,
}

/// Voice 子图编译结果。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct VoicePlan {
    pub nodes: Vec<VoiceNodePlan>,
    pub output: usize,
}

impl VoicePlan {
    pub fn new(nodes: Vec<VoiceNodePlan>, output: usize) -> Self {
        Self { nodes, output }
    }
}

/// Voice 子图中的一个节点。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct VoiceNodePlan {
    pub op: VoiceOp,
    pub inputs: Vec<usize>,
}

impl VoiceNodePlan {
    pub fn new(op: VoiceOp, inputs: Vec<usize>) -> Self {
        Self { op, inputs }
    }
}

/// Voice 子图节点操作。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum VoiceOp {
    Pitch,
    Velocity,
    Gate,
    OscPartials {
        partials: usize,
        inharmonicity: f64,
        brightness: f64,
        decay: f64,
    },
    EnvPercussive {
        attack: f64,
        decay: f64,
    },
    NoiseBurst {
        level: f64,
        duration: f64,
    },
    Mix,
    Output,
}

impl VoicePlan {
    pub fn sample(&self, context: VoiceContext) -> f64 {
        if self.nodes.len() > 64 || self.output >= self.nodes.len() {
            return 0.0;
        }
        let mut values = [0.0_f64; 64];
        for (index, node) in self.nodes.iter().enumerate() {
            values[index] = match node.op {
                VoiceOp::Pitch => context.pitch,
                VoiceOp::Velocity => context.velocity,
                VoiceOp::Gate => f64::from(u8::from(context.gate)),
                VoiceOp::OscPartials {
                    partials,
                    inharmonicity,
                    brightness,
                    decay,
                } => {
                    let pitch = node
                        .inputs
                        .first()
                        .map_or(context.pitch, |input| values[*input]);
                    let velocity = node
                        .inputs
                        .get(1)
                        .map_or(context.velocity, |input| values[*input]);
                    osc_partials(
                        pitch,
                        velocity,
                        context.local_time,
                        partials,
                        inharmonicity,
                        brightness,
                        decay,
                    )
                },
                VoiceOp::EnvPercussive { attack, decay } => {
                    let input = node.inputs.first().map_or(0.0, |input| values[*input]);
                    let attack_gain = (context.local_time / attack.max(0.0001)).min(1.0);
                    input * attack_gain * (-context.local_time / decay.max(0.01)).exp()
                },
                VoiceOp::NoiseBurst { level, duration } => {
                    if context.local_time > duration || !context.gate {
                        0.0
                    } else {
                        let velocity = node.inputs.first().map_or(1.0, |input| values[*input]);
                        let noise = ((context.seed.wrapping_mul(6_364_136_223_846_793_005) >> 40)
                            as f64
                            / 16_777_216.0)
                            * 2.0
                            - 1.0;
                        noise * level * velocity * (-context.local_time / duration.max(0.001)).exp()
                    }
                },
                VoiceOp::Mix => node.inputs.iter().map(|input| values[*input]).sum(),
                VoiceOp::Output => node.inputs.first().map_or(0.0, |input| values[*input]),
            };
        }
        values[self.output]
    }
}

fn osc_partials(
    pitch: f64,
    velocity: f64,
    time: f64,
    partials: usize,
    inharmonicity: f64,
    brightness: f64,
    decay: f64,
) -> f64 {
    let mut output = 0.0;
    let velocity = velocity.clamp(0.0, 1.0).powf(brightness + 0.5);
    for partial in 1..=partials {
        let frequency =
            pitch * partial as f64 * (1.0 + inharmonicity * (partial * partial) as f64).sqrt();
        if frequency >= 20_000.0 {
            break;
        }
        let envelope = (-time * (0.8 + partial as f64 * 0.35) / decay.max(0.05)).exp();
        output +=
            (std::f64::consts::TAU * frequency * time).sin() * envelope * velocity / partial as f64;
    }
    output * 0.18
}

/// 加法钢琴音色。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PianoPlan {
    pub partials: usize,
    pub decay: f64,
    pub brightness: f64,
    pub hammer: f64,
    pub voice: Option<VoicePlan>,
}

impl PianoPlan {
    pub fn new(partials: usize, decay: f64, brightness: f64, hammer: f64) -> Self {
        Self {
            partials,
            decay,
            brightness,
            hammer,
            voice: None,
        }
    }

    pub fn sample(&self, notes: &[NotePlan], time: f64) -> f64 {
        let mut output = 0.0;
        for note in notes {
            let local = time - note.start;
            if local < 0.0 || local > note.duration + 0.5 {
                continue;
            }
            if let Some(voice) = &self.voice {
                output += voice.sample(VoiceContext {
                    pitch: note.frequency,
                    velocity: note.velocity,
                    local_time: local,
                    gate: local <= note.duration,
                    seed: note.start.to_bits() ^ note.frequency.to_bits() ^ note.velocity.to_bits(),
                });
                continue;
            }
            let attack = (local / 0.004).min(1.0);
            let brightness = note.velocity.clamp(0.0, 1.0).powf(self.brightness + 0.5);
            for partial in 1..=self.partials {
                let frequency = note.frequency * partial as f64;
                if frequency >= 20_000.0 {
                    break;
                }
                let envelope =
                    (-local * (0.8 + partial as f64 * 0.35) / self.decay.max(0.05)).exp();
                let amplitude = brightness / partial as f64;
                output += (std::f64::consts::TAU * frequency * local).sin()
                    * envelope
                    * amplitude
                    * attack;
            }
            if local < 0.012 {
                let seed =
                    note.start.to_bits() ^ note.frequency.to_bits() ^ note.velocity.to_bits();
                let noise = ((seed.wrapping_mul(6_364_136_223_846_793_005) >> 40) as f64
                    / 16_777_216.0)
                    * 2.0
                    - 1.0;
                output += noise * (-local * 180.0).exp() * self.hammer * note.velocity;
            }
        }
        output * 0.18
    }
}

/// 轨道插件引用；实际处理器由宿主线程持有。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPlan {
    pub path: String,
    pub id: String,
    pub state: Option<String>,
}

impl PluginPlan {
    pub fn new(path: String, id: String, state: Option<String>) -> Self {
        Self { path, id, state }
    }
}

/// 宿主音频处理器的内部契约。
///
/// 插件格式由 `aurtrack-host` 适配，引擎只认这个块处理接口。实现必须保证
/// `process` 不在音频线程分配、加锁、日志或做系统调用。
pub trait BlockProcessor: Send {
    fn prepare(&mut self, sample_rate: u32, max_block_frames: usize) -> Result<(), String>;
    fn reset(&mut self);
    fn process(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) -> Result<(), String>;
}

/// 低通滤波器参数。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterSettings {
    pub cutoff: f64,
    pub resonance: f64,
}

impl FilterSettings {
    pub fn new(cutoff: f64, resonance: f64) -> Self {
        Self { cutoff, resonance }
    }
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self::new(1200.0, 0.2)
    }
}

/// 离线渲染使用的滤波器状态。
#[derive(Debug, Clone, Copy, Default)]
pub struct FilterState {
    z1: f64,
    z2: f64,
}

impl FilterState {
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

impl FilterSettings {
    /// 单采样 TPT 低通；共振 0 时接近一阶，接近 1 时更尖。
    pub fn process(&self, sample_rate: u32, state: &mut FilterState, input: f64) -> f64 {
        let nyquist_limit = f64::from(sample_rate) * 0.45;
        let cutoff = self.cutoff.clamp(20.0, nyquist_limit.max(20.0));
        let resonance = self.resonance.clamp(0.0, 0.95);
        let g = (std::f64::consts::PI * cutoff / f64::from(sample_rate)).tan();
        let k = (2.0 - 2.0 * resonance).max(0.05);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = input - state.z2;
        let v1 = a1 * state.z1 + a2 * v3;
        let v2 = state.z2 + a2 * state.z1 + a3 * v3;
        state.z1 = 2.0 * v1 - state.z1;
        state.z2 = 2.0 * v2 - state.z2;
        v2
    }
}

#[derive(Debug, Default)]
struct FilterStateAtomics {
    z1: AtomicU64,
    z2: AtomicU64,
}

/// 实时计划里的一个有状态滤波器。
#[derive(Debug, Clone)]
pub struct FilterPlan {
    pub settings: FilterSettings,
    state: Arc<FilterStateAtomics>,
}

impl FilterPlan {
    pub fn new(settings: FilterSettings) -> Self {
        Self {
            settings,
            state: Arc::new(FilterStateAtomics::default()),
        }
    }

    pub fn process(&self, sample_rate: u32, input: f64) -> f64 {
        let mut state = FilterState {
            z1: f64::from_bits(self.state.z1.load(Ordering::Relaxed)),
            z2: f64::from_bits(self.state.z2.load(Ordering::Relaxed)),
        };
        let output = self.settings.process(sample_rate, &mut state, input);
        self.state.z1.store(state.z1.to_bits(), Ordering::Relaxed);
        self.state.z2.store(state.z2.to_bits(), Ordering::Relaxed);
        output
    }

    pub fn reset(&self) {
        self.state.z1.store(0.0_f64.to_bits(), Ordering::Relaxed);
        self.state.z2.store(0.0_f64.to_bits(), Ordering::Relaxed);
    }
}

/// DAG 计划最多允许的节点数。
pub const MAX_GRAPH_NODES: usize = 64;

/// DAG 中的一个节点。
#[derive(Debug, Clone)]
pub struct GraphNodePlan {
    pub op: GraphOp,
    pub inputs: Vec<usize>,
}

/// DAG 节点操作。
#[derive(Debug, Clone)]
pub enum GraphOp {
    Voice,
    Filter(FilterPlan),
    Gain(f64),
    Mix,
    Plugin,
    Output,
}

/// 已排序的 DAG 计划；索引 0 固定是声部。
#[derive(Debug, Clone)]
pub struct GraphPlan {
    pub nodes: Vec<GraphNodePlan>,
    pub output: usize,
    pub plugin: Option<PluginPlan>,
    pub piano: Option<PianoPlan>,
}

impl GraphPlan {
    pub fn sample(&self, voice: f64, sample_rate: u32) -> f64 {
        if self.nodes.len() > MAX_GRAPH_NODES || self.output >= self.nodes.len() {
            return 0.0;
        }
        let mut values = [0.0_f64; MAX_GRAPH_NODES];
        for (index, node) in self.nodes.iter().enumerate() {
            let value = match &node.op {
                GraphOp::Voice => voice,
                GraphOp::Filter(filter) => node
                    .inputs
                    .first()
                    .map_or(0.0, |input| filter.process(sample_rate, values[*input])),
                GraphOp::Gain(gain) => {
                    node.inputs.first().map_or(0.0, |input| values[*input]) * gain
                },
                GraphOp::Mix => node.inputs.iter().map(|input| values[*input]).sum(),
                GraphOp::Plugin => node.inputs.first().map_or(0.0, |input| values[*input]),
                GraphOp::Output => node.inputs.first().map_or(0.0, |input| values[*input]),
            };
            values[index] = value;
        }
        values[self.output]
    }
}

/// 每个音符可用的 ADSR 包络参数，时间单位秒。
#[derive(Debug, Clone, Copy)]
pub struct AdsrPlan {
    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
}

impl Default for AdsrPlan {
    fn default() -> Self {
        Self {
            attack: DEFAULT_ATTACK,
            decay: DEFAULT_DECAY,
            sustain: DEFAULT_SUSTAIN,
            release: DEFAULT_RELEASE,
        }
    }
}

pub const DEFAULT_ATTACK: f64 = 0.01;
pub const DEFAULT_DECAY: f64 = 0.12;
pub const DEFAULT_SUSTAIN: f64 = 0.75;
pub const DEFAULT_RELEASE: f64 = 0.18;
const MIN_ENVELOPE_SECONDS: f64 = 0.001;

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

/// ADSR：gate 从 0 开始，`duration` 是按键时长，release 从 gate 结束处开始。
pub fn adsr_envelope(gate: f64, duration: f64, adsr: AdsrPlan) -> f64 {
    if !gate.is_finite() || gate < 0.0 {
        return 0.0;
    }
    let attack = adsr.attack.max(MIN_ENVELOPE_SECONDS);
    let decay = adsr.decay.max(0.0);
    let sustain = adsr.sustain.clamp(0.0, 1.0);
    let release = adsr.release.max(MIN_ENVELOPE_SECONDS);
    let peak_at_gate_off = if duration < attack {
        duration / attack
    } else if duration < attack + decay {
        1.0 - (1.0 - sustain) * ((duration - attack) / decay)
    } else {
        sustain
    };
    if gate < duration {
        if gate < attack {
            gate / attack
        } else if gate < attack + decay {
            1.0 - (1.0 - sustain) * ((gate - attack) / decay)
        } else {
            sustain
        }
    } else if gate < duration + release {
        peak_at_gate_off * (1.0 - (gate - duration) / release)
    } else {
        0.0
    }
}

/// 一次播放使用的不可变计划。
#[derive(Debug, Clone)]
pub struct PlaybackPlan {
    pub sample_rate: u32,
    pub loop_start_seconds: f64,
    pub loop_seconds: f64,
    pub length_seconds: f64,
    pub tracks: Vec<TrackPlan>,
}

impl PlaybackPlan {
    pub fn silent(sample_rate: u32, loop_seconds: f64) -> Self {
        Self {
            sample_rate,
            loop_start_seconds: 0.0,
            loop_seconds,
            length_seconds: loop_seconds,
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
            self.reset_filters();
        }
    }

    fn reset_filters(&self) {
        let plan = self.plan.load();
        for track in &plan.tracks {
            for filter in &track.filters {
                filter.reset();
            }
            if let Some(graph) = &track.graph {
                for node in &graph.nodes {
                    if let GraphOp::Filter(filter) = &node.op {
                        filter.reset();
                    }
                }
            }
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
        let length_seconds = plan.length_seconds.max(plan.loop_seconds).max(0.001);
        let wrapped = seconds.rem_euclid(length_seconds);
        let frames = (wrapped * f64::from(plan.sample_rate)).round() as u64;
        self.reset_filters();
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
        let loop_start_frames = (plan.loop_start_seconds * sample_rate).round() as u64;
        let loop_end_frames = (plan.loop_seconds * sample_rate)
            .round()
            .max(loop_start_frames.saturating_add(1) as f64) as u64;
        let fade_frames = (DECLICK_FADE_SECONDS * sample_rate).round().max(1.0) as u64;
        let loop_fade = if loop_end_frames.saturating_sub(loop_start_frames) > fade_frames * 2 {
            fade_frames
        } else {
            0
        };
        let loop_advance_start = loop_start_frames + loop_fade;
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
            let sample = Self::sample_with_loop(
                &plan,
                position,
                loop_start_frames,
                loop_end_frames,
                loop_fade,
                master_gain,
            );
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
            position = advance(position, loop_end_frames, loop_advance_start);
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

    /// 分轨块渲染：输出按轨道优先排列，`stems[track * frames + frame]`。
    ///
    /// 音频回调在每轨插件处理后再混音。轨道数超过计划时，多出的槽保持静音。
    pub fn render_stems(&self, stems: &mut [f32], frames: usize, track_count: usize) {
        let needed = frames.saturating_mul(track_count);
        if needed == 0 || stems.len() < needed {
            return;
        }
        if !self.playing.load(Ordering::Acquire) {
            stems[..needed].fill(0.0);
            return;
        }
        let plan = self.plan.load();
        let sample_rate = f64::from(plan.sample_rate).max(1.0);
        let loop_start_frames = (plan.loop_start_seconds * sample_rate).round() as u64;
        let loop_end_frames = (plan.loop_seconds * sample_rate)
            .round()
            .max(loop_start_frames.saturating_add(1) as f64) as u64;
        let fade_frames = (DECLICK_FADE_SECONDS * sample_rate).round().max(1.0) as u64;
        let loop_fade = if loop_end_frames.saturating_sub(loop_start_frames) > fade_frames * 2 {
            fade_frames
        } else {
            0
        };
        let loop_advance_start = loop_start_frames + loop_fade;
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
            stems[..needed].fill(0.0);
            return;
        }
        if stage == FADE_IDLE && target != NO_SEEK {
            stage = FADE_OUT;
            elapsed = 0;
        } else if stage == FADE_OUT && target == NO_SEEK {
            stage = FADE_IDLE;
            elapsed = 0;
        }
        for frame in 0..frames {
            let seek_gain = if stage == FADE_OUT {
                Some(1.0 - (elapsed + 1) as f64 / fade_frames as f64)
            } else if stage == FADE_IN {
                Some((elapsed + 1) as f64 / fade_frames as f64)
            } else {
                None
            };
            let frame_gain = seek_gain.unwrap_or(1.0) * gain * master_gain;
            for track_index in 0..track_count {
                let sample = plan.tracks.get(track_index).map_or(0.0, |track| {
                    Self::sample_track_with_loop(
                        &plan,
                        track,
                        position,
                        loop_start_frames,
                        loop_end_frames,
                        loop_fade,
                    )
                });
                stems[track_index * frames + frame] = (sample * frame_gain) as f32;
            }
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
                    for track_index in 0..track_count {
                        stems[track_index * frames + frame + 1..(track_index + 1) * frames]
                            .fill(0.0);
                    }
                    break;
                }
            } else if gain < target_gain {
                gain = (gain + gain_step).min(target_gain);
            }
            position = advance(position, loop_end_frames, loop_advance_start);
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
        loop_start: u64,
        loop_end: u64,
        loop_fade: u64,
        master_gain: f64,
    ) -> f64 {
        if loop_fade == 0 {
            return Self::sample_at(plan, position, master_gain);
        }
        let start = loop_end.saturating_sub(loop_fade);
        if position < start || position >= loop_end {
            return Self::sample_at(plan, position, master_gain);
        }
        let offset = position - start;
        let blend = (offset + 1) as f64 / loop_fade as f64;
        let tail = Self::sample_at(plan, position, master_gain);
        let head = Self::sample_at(plan, loop_start + offset, master_gain);
        tail * (1.0 - blend) + head * blend
    }

    fn sample_at(plan: &PlaybackPlan, position: u64, master_gain: f64) -> f64 {
        let sample = plan
            .tracks
            .iter()
            .map(|track| Self::sample_track(plan, track, position))
            .sum::<f64>();
        (sample * master_gain).clamp(-1.0, 1.0)
    }

    fn sample_track(plan: &PlaybackPlan, track: &TrackPlan, position: u64) -> f64 {
        let sample_rate = f64::from(plan.sample_rate).max(1.0);
        let time = position as f64 / sample_rate;
        let mut voice = if let Some(piano) = &track.piano {
            piano.sample(&track.notes, time)
        } else {
            0.0
        };
        if track.piano.is_none() {
            if let Some(frequency) = track.frequency {
                voice += (std::f64::consts::TAU * frequency * time).sin() * 0.25;
            }
            let release = track
                .adsr
                .map_or(0.0, |adsr| adsr.release.max(MIN_ENVELOPE_SECONDS));
            for note in &track.notes {
                if time >= note.start && time < note.start + note.duration + release {
                    let local = time - note.start;
                    let envelope = track.adsr.map_or_else(
                        || note_envelope(local, note.duration),
                        |adsr| adsr_envelope(local, note.duration, adsr),
                    );
                    voice += (std::f64::consts::TAU * note.frequency * local).sin()
                        * note.velocity
                        * envelope
                        * 0.25;
                }
            }
        }
        if let Some(graph) = &track.graph {
            return graph.sample(voice, plan.sample_rate);
        }
        for filter in &track.filters {
            voice = filter.process(plan.sample_rate, voice);
        }
        voice * track.gain
    }

    fn sample_track_with_loop(
        plan: &PlaybackPlan,
        track: &TrackPlan,
        position: u64,
        loop_start: u64,
        loop_end: u64,
        loop_fade: u64,
    ) -> f64 {
        if loop_fade == 0 {
            return Self::sample_track(plan, track, position);
        }
        let start = loop_end.saturating_sub(loop_fade);
        if position < start || position >= loop_end {
            return Self::sample_track(plan, track, position);
        }
        let offset = position - start;
        let blend = (offset + 1) as f64 / loop_fade as f64;
        let tail = Self::sample_track(plan, track, position);
        let head = Self::sample_track(plan, track, loop_start + offset);
        tail * (1.0 - blend) + head * blend
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
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: Some(440.0),
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: Vec::new(),
            }],
        }
    }

    #[test]
    fn voice_plan_renders_partials_and_noise() {
        let plan = VoicePlan::new(
            vec![
                VoiceNodePlan::new(
                    VoiceOp::OscPartials {
                        partials: 8,
                        inharmonicity: 0.0004,
                        brightness: 0.5,
                        decay: 1.4,
                    },
                    vec![],
                ),
                VoiceNodePlan::new(
                    VoiceOp::EnvPercussive {
                        attack: 0.004,
                        decay: 1.8,
                    },
                    vec![0],
                ),
                VoiceNodePlan::new(
                    VoiceOp::NoiseBurst {
                        level: 0.2,
                        duration: 0.008,
                    },
                    vec![],
                ),
                VoiceNodePlan::new(VoiceOp::Mix, vec![1, 2]),
                VoiceNodePlan::new(VoiceOp::Output, vec![3]),
            ],
            4,
        );
        let sample = plan.sample(VoiceContext {
            pitch: 261.63,
            velocity: 0.8,
            local_time: 0.002,
            gate: true,
            seed: 7,
        });
        assert!(sample.abs() > 0.0001);
    }

    #[test]
    fn piano_voice_produces_harmonics_and_decays() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 2.0,
            length_seconds: 2.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                notes: vec![NotePlan {
                    start: 0.0,
                    duration: 1.0,
                    frequency: midi_to_hz(60),
                    velocity: 0.8,
                }],
                plugin: None,
                piano: Some(PianoPlan::new(8, 1.0, 0.5, 0.2)),
            }],
        });
        playback.play();
        let mut early = vec![0.0_f32; 4800];
        playback.render(&mut early);
        let mut late = vec![0.0_f32; 4800];
        playback.render(&mut late);
        assert!(early.iter().any(|sample| sample.abs() > 0.001));
        let early_energy: f32 = early.iter().map(|sample| sample.abs()).sum();
        let late_energy: f32 = late.iter().map(|sample| sample.abs()).sum();
        assert!(late_energy < early_energy);
    }

    #[test]
    fn render_stems_keeps_tracks_separate() {
        let mut plan = plan();
        plan.tracks.push(TrackPlan {
            frequency: Some(220.0),
            gain: 0.5,
            adsr: None,
            filters: Vec::new(),
            graph: None,
            plugin: None,
            piano: None,
            notes: Vec::new(),
        });
        let playback = Playback::new(48_000);
        playback.set_plan(plan);
        playback.play();
        let frames = 256;
        let mut stems = vec![0.0_f32; frames * 2];
        playback.render_stems(&mut stems, frames, 2);
        assert!(stems[..frames].iter().any(|sample| *sample != 0.0));
        assert!(stems[frames..].iter().any(|sample| *sample != 0.0));
        assert!((stems[10] - stems[frames + 10]).abs() > 1.0e-6);
        assert!(playback.position_seconds() > 0.0);
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
            loop_start_seconds: 0.0,
            loop_seconds: 0.1,
            length_seconds: 0.1,
            tracks: vec![TrackPlan {
                frequency: Some(331.0),
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
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
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
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
    fn adsr_has_sustain_and_release_tail() {
        let adsr = AdsrPlan {
            attack: 0.01,
            decay: 0.01,
            sustain: 0.5,
            release: 0.1,
        };
        assert!(adsr_envelope(0.0, 0.1, adsr).abs() < 1e-12);
        assert!((adsr_envelope(0.005, 0.1, adsr) - 0.5).abs() < 1e-9);
        assert!((adsr_envelope(0.02, 0.1, adsr) - 0.5).abs() < 1e-9);
        assert!((adsr_envelope(0.15, 0.1, adsr) - 0.25).abs() < 1e-9);
        assert!(adsr_envelope(0.21, 0.1, adsr).abs() < 1e-12);
    }

    #[test]
    fn lowpass_attenuates_high_frequency() {
        let settings = FilterSettings {
            cutoff: 500.0,
            resonance: 0.2,
        };
        let mut low_state = FilterState::default();
        let mut high_state = FilterState::default();
        let mut low_energy = 0.0;
        let mut high_energy = 0.0;
        for index in 0..48_000 {
            let time = index as f64 / 48_000.0;
            let low = settings.process(
                48_000,
                &mut low_state,
                (std::f64::consts::TAU * 100.0 * time).sin(),
            );
            let high = settings.process(
                48_000,
                &mut high_state,
                (std::f64::consts::TAU * 8_000.0 * time).sin(),
            );
            if index > 4_800 {
                low_energy += low * low;
                high_energy += high * high;
            }
        }
        assert!(low_energy > high_energy * 4.0);
    }

    #[test]
    fn adsr_release_tail_is_rendered() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 1.0,
            length_seconds: 1.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: Some(AdsrPlan {
                    attack: 0.005,
                    decay: 0.005,
                    sustain: 0.8,
                    release: 0.2,
                }),
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: vec![NotePlan {
                    start: 0.0,
                    duration: 0.05,
                    frequency: 440.0,
                    velocity: 1.0,
                }],
            }],
        });
        playback.play();
        let mut buffer = vec![0.0_f32; 9_600];
        playback.render(&mut buffer);
        assert!(buffer[3_000].abs() > 0.0);
    }

    #[test]
    fn note_tail_is_declicked() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
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
    fn custom_loop_range_crossfades() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.1,
            loop_seconds: 0.2,
            length_seconds: 0.3,
            tracks: vec![TrackPlan {
                frequency: Some(331.0),
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
                notes: Vec::new(),
            }],
        });
        playback.play();
        let mut buffer = vec![0.0_f32; 12_000];
        playback.render(&mut buffer);
        let max_delta = buffer
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_delta < 0.03, "custom loop step too large: {max_delta}");
    }

    #[test]
    fn output_is_clamped() {
        let playback = Playback::new(48_000);
        playback.set_plan(PlaybackPlan {
            sample_rate: 48_000,
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: (0..8)
                .map(|_| TrackPlan {
                    frequency: Some(440.0),
                    gain: 1.0,
                    adsr: None,
                    filters: Vec::new(),
                    graph: None,
                    plugin: None,
                    piano: None,
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
            loop_start_seconds: 0.0,
            loop_seconds: 5.0,
            length_seconds: 5.0,
            tracks: vec![TrackPlan {
                frequency: None,
                gain: 1.0,
                adsr: None,
                filters: Vec::new(),
                graph: None,
                plugin: None,
                piano: None,
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
