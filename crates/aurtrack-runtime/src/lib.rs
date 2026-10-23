//! 职责：
//! - 把会话、实时引擎与音频输出组合成一个可执行运行时
//! - 在命令层统一文档命令与走带命令
//!
//! 禁止：
//! - 依赖 UI
//! - 在音频回调路径里加锁或分配

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::needless_pass_by_value,
    clippy::missing_const_for_fn
)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::sync::Arc;

use aurtrack_command::{CommandError, CommandRequest, CommandResponse};
use aurtrack_engine::{AdsrPlan, NotePlan, Playback, PlaybackPlan, TrackPlan, midi_to_hz};
use aurtrack_io::AudioHandle;
use aurtrack_session::{Project, Session, TICKS_PER_QUARTER};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 音频设备状态，供 UI 与 MCP 查询。
#[derive(Debug, Clone, Serialize)]
pub struct AudioInfo {
    pub available: bool,
    pub sample_rate: u32,
    pub device: Option<String>,
}

/// 文档、引擎与音频输出的组合根。
#[derive(Debug)]
pub struct Runtime {
    session: Session,
    playback: Arc<Playback>,
    audio: Option<AudioHandle>,
    sample_rate: u32,
    metronome: bool,
    plugin_bridge: Option<aurtrack_host::PluginBridgeControl>,
    plugin_config: Option<(std::path::PathBuf, String)>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            session: Session::default(),
            playback: Arc::new(Playback::new(48_000)),
            audio: None,
            sample_rate: 48_000,
            metronome: false,
            plugin_bridge: None,
            plugin_config: None,
        }
    }

    /// 惰性打开默认输出设备；音频流出错后会丢弃旧句柄并重新打开。
    pub fn ensure_audio(&mut self) -> Result<(), CommandError> {
        if self.audio.as_ref().is_some_and(AudioHandle::is_healthy) {
            return Ok(());
        }
        self.audio = None;
        self.plugin_bridge = None;
        let (bridge, port) = aurtrack_host::PluginBridgeControl::new();
        let handle = aurtrack_io::spawn_with_master(self.playback.clone(), Some(Box::new(port)))
            .map_err(|error| CommandError::new("audio_unavailable", error.to_string()))?;
        self.sample_rate = handle.sample_rate();
        self.audio = Some(handle);
        self.plugin_bridge = Some(bridge);
        if let Some((path, id)) = self.plugin_config.clone() {
            if let Some(bridge) = self.plugin_bridge.as_ref() {
                bridge
                    .load(path, id, self.sample_rate)
                    .map_err(|error| CommandError::new("plugin_unavailable", error))?;
            }
        }
        Ok(())
    }

    pub fn audio_info(&self) -> AudioInfo {
        AudioInfo {
            available: self.audio.is_some(),
            sample_rate: self.sample_rate,
            device: self
                .audio
                .as_ref()
                .map(|handle| handle.device_name().to_string()),
        }
    }

    /// 统一执行入口：走带命令由运行时处理，其余交给会话。
    pub fn execute(&mut self, request: &CommandRequest) -> Result<CommandResponse, CommandError> {
        match request.cmd.as_str() {
            "transport.play" => self.transport_play(),
            "transport.stop" => Ok(self.transport_stop()),
            "transport.seek" => self.transport_seek(&request.args),
            "transport.state" => Ok(self.transport_state()),
            "transport.gain" => self.transport_gain(&request.args),
            "transport.metronome" => self.transport_metronome(&request.args),
            "plugin.scan" => Self::plugin_scan(&request.args),
            "plugin.inspect" => Self::plugin_inspect(&request.args),
            "plugin.master" => self.plugin_master(&request.args),
            "plugin.master_off" => self.plugin_master_off(),
            "extension.list" => Self::extension_list(&request.args),
            "extension.call" => Self::extension_call(&request.args),
            _ => self.session.execute(request),
        }
    }

    fn transport_play(&mut self) -> Result<CommandResponse, CommandError> {
        if self.session.project().is_none() {
            return Err(CommandError::not_found("还没有工程，先执行 project.create"));
        }
        // 每次从停止状态开始播放都重新打开输出，避免系统切换默认设备后
        // 继续使用已经收不到回调的旧音频流。
        if !self.playback.is_playing() {
            self.audio = None;
            self.plugin_bridge = None;
        }
        self.ensure_audio()?;
        let project = self
            .session
            .project()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let plan = build_plan(project, self.sample_rate, self.metronome)?;
        self.playback.set_plan(plan);
        self.playback.play();
        Ok(CommandResponse::ok(json!({
            "playing": true,
            "sample_rate": self.sample_rate,
            "metronome": self.metronome,
        })))
    }

    fn transport_stop(&self) -> CommandResponse {
        self.playback.stop();
        CommandResponse::ok(json!({ "playing": false }))
    }

    fn transport_seek(&self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: SeekArgs = parse_args(args)?;
        if !args.seconds.is_finite() {
            return Err(CommandError::invalid_args("seconds 必须是有限数"));
        }
        self.playback.seek(args.seconds);
        Ok(CommandResponse::ok(json!({
            "position": self.playback.position_seconds()
        })))
    }

    fn transport_state(&self) -> CommandResponse {
        CommandResponse::ok(json!({
            "playing": self.playback.is_playing(),
            "position": self.playback.position_seconds(),
            "sample_rate": self.sample_rate,
            "metronome": self.metronome,
        }))
    }

    fn transport_gain(&self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: GainArgs = parse_args(args)?;
        if !args.gain.is_finite() || !(0.0..=4.0).contains(&args.gain) {
            return Err(CommandError::invalid_args("gain 必须在 0 到 4 之间"));
        }
        self.playback.set_master_gain(args.gain);
        Ok(CommandResponse::ok(json!({ "gain": args.gain })))
    }

    fn plugin_scan(args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ScanArgs {
            #[serde(default)]
            path: Option<String>,
        }
        let args: ScanArgs = parse_args(args)?;
        let directory = args
            .path
            .map(std::path::PathBuf::from)
            .or_else(aurtrack_host::default_plugin_directory)
            .ok_or_else(|| CommandError::invalid_args("找不到默认插件目录"))?;
        let report = aurtrack_host::scan_directory(&directory)
            .map_err(|error| CommandError::invalid_args(format!("扫描插件失败：{error}")))?;
        Ok(CommandResponse::ok(json!({
            "directory": directory.display().to_string(),
            "plugins": report.plugins,
            "failures": report.failures,
        })))
    }

    fn plugin_inspect(args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct InspectArgs {
            path: String,
            id: String,
        }
        let args: InspectArgs = parse_args(args)?;
        let capabilities =
            aurtrack_host::inspect_plugin(std::path::Path::new(&args.path), &args.id)
                .map_err(|error| CommandError::invalid_args(format!("读取插件失败：{error}")))?;
        Ok(CommandResponse::ok(json!({
            "path": args.path,
            "id": args.id,
            "parameters": capabilities.parameters,
            "has_state": capabilities.has_state,
        })))
    }

    fn plugin_master(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct MasterArgs {
            path: String,
            id: String,
        }
        let args: MasterArgs = parse_args(args)?;
        self.ensure_audio()?;
        let Some(bridge) = self.plugin_bridge.as_ref() else {
            return Err(CommandError::new(
                "audio_unavailable",
                "音频输出没有打开".to_string(),
            ));
        };
        let path = std::path::PathBuf::from(&args.path);
        if let Err(error) = bridge.load(path.clone(), args.id.clone(), self.sample_rate) {
            self.plugin_config = None;
            return Err(CommandError::new("plugin_unavailable", error));
        }
        self.plugin_config = Some((path, args.id.clone()));
        Ok(CommandResponse::ok(json!({
            "path": args.path,
            "id": args.id,
            "sample_rate": self.sample_rate,
            "loaded": true,
        })))
    }

    fn plugin_master_off(&mut self) -> Result<CommandResponse, CommandError> {
        if let Some(bridge) = self.plugin_bridge.as_ref() {
            bridge
                .unload()
                .map_err(|error| CommandError::new("plugin_unavailable", error))?;
        }
        self.plugin_config = None;
        Ok(CommandResponse::ok(json!({ "loaded": false })))
    }

    fn extension_list(args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ListArgs {
            #[serde(default)]
            path: Option<String>,
        }
        let args: ListArgs = parse_args(args)?;
        let directory = extension_directory(args.path)?;
        let report = aurtrack_extension::scan_extensions(&directory)
            .map_err(|error| CommandError::invalid_args(format!("扫描扩展失败：{error}")))?;
        Ok(CommandResponse::ok(json!({
            "directory": directory.display().to_string(),
            "extensions": report.extensions,
            "failures": report.failures,
        })))
    }

    fn extension_call(args: &Value) -> Result<CommandResponse, CommandError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct CallArgs {
            extension: String,
            command: String,
            #[serde(default)]
            args: Option<Value>,
            #[serde(default)]
            path: Option<String>,
        }
        let args: CallArgs = parse_args(args)?;
        let directory = extension_directory(args.path)?;
        let report = aurtrack_extension::scan_extensions(&directory)
            .map_err(|error| CommandError::invalid_args(format!("扫描扩展失败：{error}")))?;
        let extension = report
            .extensions
            .iter()
            .find(|extension| extension.id == args.extension)
            .ok_or_else(|| CommandError::not_found(format!("找不到扩展：{}", args.extension)))?;
        let result = aurtrack_extension::run_extension_command(
            extension,
            &args.command,
            args.args.as_ref().unwrap_or(&Value::Null),
        )
        .map_err(|error| CommandError::invalid_args(format!("扩展执行失败：{error}")))?;
        Ok(CommandResponse::ok(json!({
            "extension": args.extension,
            "command": args.command,
            "result": result,
        })))
    }

    fn transport_metronome(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: MetronomeArgs = parse_args(args)?;
        if self.session.project().is_none() {
            return Err(CommandError::not_found("还没有工程，先执行 project.create"));
        }
        self.metronome = args.enabled;
        if self.playback.is_playing() {
            return self.transport_play();
        }
        Ok(CommandResponse::ok(json!({ "metronome": self.metronome })))
    }
}

fn build_plan(
    project: &Project,
    sample_rate: u32,
    metronome: bool,
) -> Result<PlaybackPlan, CommandError> {
    let mut tracks: Vec<TrackPlan> = project
        .tracks
        .iter()
        .map(|track| {
            let (frequency, adsr_settings, graph) = track
                .playback_graph()
                .map_err(|error| CommandError::invalid_args(format!("节点图错误：{error}")))?;
            let adsr = adsr_settings.map(|settings| AdsrPlan {
                attack: settings.attack,
                decay: settings.decay,
                sustain: settings.sustain,
                release: settings.release,
            });
            let notes = track
                .clips
                .iter()
                .flat_map(|clip| {
                    clip.notes.iter().map(move |note| NotePlan {
                        start: project.ticks_to_seconds(clip.start_tick + note.start_tick),
                        duration: project.ticks_to_seconds(note.duration_tick),
                        frequency: midi_to_hz(note.pitch),
                        velocity: note.velocity,
                    })
                })
                .collect();
            Ok(TrackPlan {
                frequency,
                gain: 1.0,
                adsr,
                filters: Vec::new(),
                graph: Some(graph),
                notes,
            })
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    if metronome {
        let length_ticks = project.length_ticks();
        let bar_ticks = project.ticks_per_bar().max(1);
        let beat_seconds = project.ticks_to_seconds(TICKS_PER_QUARTER);
        let click_seconds = beat_seconds.clamp(0.008, 0.04);
        let mut notes = Vec::new();
        let mut tick = 0;
        while tick < length_ticks {
            let accent = tick % bar_ticks == 0;
            notes.push(NotePlan {
                start: project.ticks_to_seconds(tick),
                duration: click_seconds,
                frequency: if accent { 1760.0 } else { 1320.0 },
                velocity: if accent { 0.7 } else { 0.45 },
            });
            tick += TICKS_PER_QUARTER;
        }
        tracks.push(TrackPlan {
            frequency: None,
            gain: 0.8,
            adsr: None,
            filters: Vec::new(),
            graph: None,
            notes,
        });
    }
    Ok(PlaybackPlan {
        sample_rate,
        loop_start_seconds: project.loop_start_seconds(),
        loop_seconds: project.loop_end_seconds(),
        length_seconds: project.length_seconds(),
        tracks,
    })
}

fn extension_directory(path: Option<String>) -> Result<std::path::PathBuf, CommandError> {
    path.map(std::path::PathBuf::from)
        .or_else(aurtrack_extension::default_extension_directory)
        .ok_or_else(|| CommandError::invalid_args("找不到默认扩展目录"))
}

fn parse_args<T: DeserializeOwned>(args: &Value) -> Result<T, CommandError> {
    let normalized = if args.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        args.clone()
    };
    serde_json::from_value(normalized)
        .map_err(|error| CommandError::invalid_args(format!("参数不合法：{error}")))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeekArgs {
    seconds: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainArgs {
    gain: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetronomeArgs {
    enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurtrack_session::{Clip, Note, TICKS_PER_QUARTER, TimeSignature, Track};

    fn run(runtime: &mut Runtime, cmd: &str, args: Value) -> Result<CommandResponse, CommandError> {
        runtime.execute(&CommandRequest {
            cmd: cmd.to_string(),
            args,
        })
    }

    #[test]
    fn transport_state_starts_stopped() {
        let mut runtime = Runtime::new();
        let response = run(&mut runtime, "transport.state", json!({})).unwrap();
        assert_eq!(response.data["playing"], false);
        assert!(response.data["position"].as_f64().unwrap() < 0.001);
    }

    #[test]
    fn seek_sets_position_without_audio() {
        let mut runtime = Runtime::new();
        let response = run(&mut runtime, "transport.seek", json!({ "seconds": 2.5 })).unwrap();
        assert!((response.data["position"].as_f64().unwrap() - 2.5).abs() < 0.001);
    }

    #[test]
    fn metronome_toggles_and_is_reported() {
        let mut runtime = Runtime::new();
        run(&mut runtime, "project.create", json!({})).unwrap();
        let response = run(
            &mut runtime,
            "transport.metronome",
            json!({ "enabled": true }),
        )
        .unwrap();
        assert_eq!(response.data["metronome"], true);
        let state = run(&mut runtime, "transport.state", json!({})).unwrap();
        assert_eq!(state.data["metronome"], true);
    }

    #[test]
    fn gain_rejects_out_of_range() {
        let mut runtime = Runtime::new();
        let error = run(&mut runtime, "transport.gain", json!({ "gain": 9.0 })).unwrap_err();
        assert_eq!(error.code, "invalid_args");
    }

    #[test]
    fn build_plan_schedules_clip_notes() {
        let project = Project {
            name: "notes".to_string(),
            sample_rate: 48_000,
            tempo_bpm: 120.0,
            time_signature: TimeSignature {
                numerator: 4,
                denominator: 4,
            },
            length_bars: 2,
            loop_start_tick: 0,
            loop_end_tick: 8 * TICKS_PER_QUARTER,
            tracks: vec![Track {
                id: "track-1".to_string(),
                name: "lead".to_string(),
                graph: aurtrack_graph::Graph::empty_track(
                    aurtrack_graph::NodeKind::InstrumentSilence,
                )
                .unwrap(),
                clips: vec![Clip {
                    id: "clip-1".to_string(),
                    name: "verse".to_string(),
                    start_tick: TICKS_PER_QUARTER,
                    duration_tick: 2 * TICKS_PER_QUARTER,
                    notes: vec![Note {
                        id: "note-1".to_string(),
                        pitch: 69,
                        start_tick: TICKS_PER_QUARTER / 2,
                        duration_tick: TICKS_PER_QUARTER,
                        velocity: 0.8,
                    }],
                }],
            }],
        };

        let plan = build_plan(&project, 48_000, false).unwrap();
        assert_eq!(plan.tracks.len(), 1);
        let metronome_plan = build_plan(&project, 48_000, true).unwrap();
        assert_eq!(metronome_plan.tracks.len(), 2);
        assert_eq!(metronome_plan.tracks[1].notes.len(), 8);
        assert!((metronome_plan.tracks[1].notes[0].frequency - 1760.0).abs() < 1e-9);
        assert!((metronome_plan.tracks[1].notes[1].frequency - 1320.0).abs() < 1e-9);
        assert!((metronome_plan.tracks[1].notes[1].start - 0.5).abs() < 1e-9);
        assert!((plan.loop_start_seconds - 0.0).abs() < 1e-9);
        assert!((plan.loop_seconds - 4.0).abs() < 1e-9);
        assert!((plan.length_seconds - 4.0).abs() < 1e-9);
        assert_eq!(plan.tracks[0].notes.len(), 1);
        assert!((plan.tracks[0].notes[0].start - 0.75).abs() < 1e-9);
        assert!((plan.tracks[0].notes[0].frequency - 440.0).abs() < 0.01);
    }

    #[test]
    fn describe_lists_forty_eight_commands() {
        let mut runtime = Runtime::new();
        let response = run(&mut runtime, "describe", json!({})).unwrap();
        let commands = response.data["commands"].as_array().unwrap();
        assert_eq!(commands.len(), 48);
        assert!(
            commands
                .iter()
                .any(|command| command["id"] == "transport.play")
        );
    }
}
