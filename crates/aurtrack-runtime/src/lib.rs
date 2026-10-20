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
use aurtrack_engine::{Playback, PlaybackPlan, TrackPlan};
use aurtrack_io::AudioHandle;
use aurtrack_session::{Project, Session, Source};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const LOOP_SECONDS: f64 = 5.0;

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
        }
    }

    /// 惰性打开默认输出设备；重复调用是幂等的。
    pub fn ensure_audio(&mut self) -> Result<(), CommandError> {
        if self.audio.is_some() {
            return Ok(());
        }
        let handle = aurtrack_io::spawn(self.playback.clone())
            .map_err(|error| CommandError::new("audio_unavailable", error.to_string()))?;
        self.sample_rate = handle.sample_rate();
        self.audio = Some(handle);
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
            _ => self.session.execute(request),
        }
    }

    fn transport_play(&mut self) -> Result<CommandResponse, CommandError> {
        if self.session.project().is_none() {
            return Err(CommandError::not_found("还没有工程，先执行 project.create"));
        }
        self.ensure_audio()?;
        let project = self
            .session
            .project()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let plan = build_plan(project, self.sample_rate);
        self.playback.set_plan(plan);
        self.playback.play();
        Ok(CommandResponse::ok(json!({
            "playing": true,
            "sample_rate": self.sample_rate
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
            "sample_rate": self.sample_rate
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
}

fn build_plan(project: &Project, sample_rate: u32) -> PlaybackPlan {
    let tracks = project
        .tracks
        .iter()
        .map(|track| {
            let gain = track
                .devices
                .iter()
                .filter(|device| device.kind == "gain")
                .filter_map(|device| device.params.get("gain").copied())
                .product::<f64>();
            let frequency = match &track.source {
                Source::Sine { frequency } => Some(*frequency),
                Source::Silence => None,
            };
            TrackPlan { frequency, gain }
        })
        .collect();
    PlaybackPlan {
        sample_rate,
        loop_seconds: LOOP_SECONDS,
        tracks,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn gain_rejects_out_of_range() {
        let mut runtime = Runtime::new();
        let error = run(&mut runtime, "transport.gain", json!({ "gain": 9.0 })).unwrap_err();
        assert_eq!(error.code, "invalid_args");
    }

    #[test]
    fn describe_lists_sixteen_commands() {
        let mut runtime = Runtime::new();
        let response = run(&mut runtime, "describe", json!({})).unwrap();
        let commands = response.data["commands"].as_array().unwrap();
        assert_eq!(commands.len(), 16);
        assert!(
            commands
                .iter()
                .any(|command| command["id"] == "transport.play")
        );
    }
}
