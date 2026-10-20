//! 职责：
//! - 维护内存中的最小工程模型
//! - 执行命令、维护撤销快照
//! - 渲染 WAV 片段并返回分析结果
//!
//! 禁止：
//! - 依赖 UI 或音频设备
//! - 在渲染路径里调用外部进程

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value
)]

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use aurtrack_command::{CommandError, CommandRequest, CommandResponse, registry};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 最小工程模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub sample_rate: u32,
    pub tracks: Vec<Track>,
}

/// 一条轨道。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub source: Source,
    pub devices: Vec<Device>,
}

/// 轨道信号源。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Silence,
    Sine { frequency: f64 },
}

/// 轨道上的设备。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub kind: String,
    pub params: BTreeMap<String, f64>,
}

const PROJECT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    format_version: u32,
    project: Project,
}

#[derive(Debug, Clone)]
struct Snapshot {
    project: Option<Project>,
    command: String,
}

/// 命令执行会话。
#[derive(Debug, Default)]
pub struct Session {
    project: Option<Project>,
    history: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    next_track: u64,
    next_device: u64,
    next_undo: u64,
}

impl Session {
    /// 只读访问当前工程，供渲染与实时播放编译计划。
    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    /// 执行一条命令。
    pub fn execute(&mut self, request: &CommandRequest) -> Result<CommandResponse, CommandError> {
        match request.cmd.as_str() {
            "project.create" => self.cmd_project_create(&request.args),
            "project.state" => Ok(self.cmd_project_state()),
            "project.save" => self.cmd_project_save(&request.args),
            "project.open" => self.cmd_project_open(&request.args),
            "track.create" => self.cmd_track_create(&request.args),
            "track.remove" => self.cmd_track_remove(&request.args),
            "device.add" => self.cmd_device_add(&request.args),
            "device.remove" => self.cmd_device_remove(&request.args),
            "param.set" => self.cmd_param_set(&request.args),
            "render.segment" => self.cmd_render_segment(&request.args),
            "describe" => Ok(self.cmd_describe()),
            "undo" => self.cmd_undo(&request.args),
            "redo" => self.cmd_redo(&request.args),
            other => Err(CommandError::unknown_command(other)),
        }
    }

    fn snapshot(&mut self, command: &str) -> String {
        self.redo_stack.clear();
        self.next_undo += 1;
        let number = self.next_undo;
        self.history.push(Snapshot {
            project: self.project.clone(),
            command: command.to_string(),
        });
        format!("undo-{number}")
    }

    fn cmd_project_create(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ProjectCreateArgs = parse_args(args)?;
        let name = args.name.unwrap_or_else(|| "Untitled".to_string());
        if name.trim().is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }
        let sample_rate = args.sample_rate.unwrap_or(48_000);
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(CommandError::invalid_args(
                "sample_rate 必须在 8000 到 192000 之间",
            ));
        }
        let token = self.snapshot("project.create");
        self.project = Some(Project {
            name,
            sample_rate,
            tracks: Vec::new(),
        });
        Ok(CommandResponse::ok(json!({ "project": self.project_summary() })).with_undo(token))
    }

    fn cmd_track_create(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: TrackCreateArgs = parse_args(args)?;
        if args.name.trim().is_empty() {
            return Err(CommandError::invalid_args("name 不能为空"));
        }
        let sample_rate = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?
            .sample_rate;
        let source = match args.source.as_deref().unwrap_or("silence") {
            "silence" => Source::Silence,
            "sine" => {
                let frequency = args.frequency.unwrap_or(440.0);
                if !frequency.is_finite() || !(20.0..=20_000.0).contains(&frequency) {
                    return Err(CommandError::invalid_args(
                        "frequency 必须在 20 到 20000 之间",
                    ));
                }
                if frequency >= f64::from(sample_rate) / 2.0 {
                    return Err(CommandError::invalid_args("frequency 必须小于采样率的一半"));
                }
                Source::Sine { frequency }
            },
            other => {
                return Err(CommandError::invalid_args(format!(
                    "不支持的 source：{other}"
                )));
            },
        };
        let token = self.snapshot("track.create");
        self.next_track += 1;
        let number = self.next_track;
        let track_id = format!("track-{number}");
        let path = format!("/tracks/{track_id}");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        project.tracks.push(Track {
            id: track_id.clone(),
            name: args.name,
            source,
            devices: Vec::new(),
        });
        Ok(CommandResponse::ok(json!({ "track_id": track_id, "path": path })).with_undo(token))
    }

    fn cmd_device_add(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: DeviceAddArgs = parse_args(args)?;
        if args.kind != "gain" {
            return Err(CommandError::invalid_args(format!(
                "不支持的 kind：{}",
                args.kind
            )));
        }
        let params = args.params.unwrap_or_default();
        for (name, value) in &params {
            if name != "gain" {
                return Err(CommandError::invalid_args(format!("不支持的参数：{name}")));
            }
            validate_gain(*value)?;
        }
        let exists = self
            .project
            .as_ref()
            .is_some_and(|project| project.tracks.iter().any(|track| track.id == args.track_id));
        if !exists {
            return Err(CommandError::not_found(format!(
                "找不到轨道：{}",
                args.track_id
            )));
        }
        let token = self.snapshot("device.add");
        self.next_device += 1;
        let number = self.next_device;
        let device_id = format!("device-{number}");
        let path = format!("/tracks/{}/devices/{device_id}", args.track_id);
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == args.track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到轨道：{}", args.track_id)))?;
        let mut params = params;
        params.entry("gain".to_string()).or_insert(1.0);
        track.devices.push(Device {
            id: device_id.clone(),
            kind: args.kind,
            params,
        });
        Ok(CommandResponse::ok(json!({ "device_id": device_id, "path": path })).with_undo(token))
    }

    fn cmd_param_set(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ParamSetArgs = parse_args(args)?;
        validate_gain(args.value)?;
        let path = args.path.clone();
        let (track_id, device_id, name) = parse_param_path(&path)?;
        if name != "gain" {
            return Err(CommandError::invalid_args(format!("不支持的参数：{name}")));
        }
        let exists = self.project.as_ref().is_some_and(|project| {
            project.tracks.iter().any(|track| {
                track.id == track_id
                    && track
                        .devices
                        .iter()
                        .any(|device| device.id == device_id && device.kind == "gain")
            })
        });
        if !exists {
            return Err(CommandError::not_found(format!("找不到参数：{path}")));
        }
        let token = self.snapshot("param.set");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let device = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .and_then(|track| {
                track
                    .devices
                    .iter_mut()
                    .find(|device| device.id == device_id)
            })
            .ok_or_else(|| CommandError::not_found(format!("找不到参数：{path}")))?;
        device.params.insert("gain".to_string(), args.value);
        Ok(CommandResponse::ok(json!({ "path": path, "value": args.value })).with_undo(token))
    }

    fn cmd_render_segment(&self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: RenderArgs = parse_args(args)?;
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let start_seconds = args.start_seconds.unwrap_or(0.0);
        if !start_seconds.is_finite() || start_seconds < 0.0 {
            return Err(CommandError::invalid_args(
                "start_seconds 必须是大于等于 0 的有限数",
            ));
        }
        let duration_seconds = args.duration_seconds.unwrap_or(1.0);
        if !duration_seconds.is_finite() || !(0.01..=600.0).contains(&duration_seconds) {
            return Err(CommandError::invalid_args(
                "duration_seconds 必须在 0.01 到 600 之间",
            ));
        }
        let path = args
            .path
            .unwrap_or_else(|| "aurtrack-render.wav".to_string());
        if path.trim().is_empty() {
            return Err(CommandError::invalid_args("path 不能为空"));
        }
        let data = render_project(project, start_seconds, duration_seconds, &path)?;
        Ok(CommandResponse::ok(data))
    }

    fn cmd_describe(&self) -> CommandResponse {
        let commands: Vec<Value> = registry()
            .iter()
            .map(|spec| {
                json!({
                    "id": spec.id,
                    "summary": spec.summary,
                    "undo": spec.undo,
                    "input_schema": (spec.input_schema)(),
                })
            })
            .collect();
        CommandResponse::ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "commands": commands,
            "project": self.project_summary(),
        }))
    }

    fn cmd_undo(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: UndoArgs = parse_args(args)?;
        let steps = args.steps.unwrap_or(1);
        if !(1..=100).contains(&steps) {
            return Err(CommandError::invalid_args("steps 必须在 1 到 100 之间"));
        }
        if self.history.is_empty() {
            return Err(CommandError::nothing_to_undo());
        }
        let mut undone = Vec::new();
        for _ in 0..steps {
            let Some(snapshot) = self.history.pop() else {
                break;
            };
            self.redo_stack.push(Snapshot {
                project: self.project.clone(),
                command: snapshot.command.clone(),
            });
            self.project = snapshot.project;
            undone.push(snapshot.command);
        }
        Ok(CommandResponse::ok(json!({ "undone": undone })))
    }

    fn cmd_project_state(&self) -> CommandResponse {
        CommandResponse::ok(json!({ "project": &self.project }))
    }

    fn cmd_project_save(&self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ProjectPathArgs = parse_args(args)?;
        let path = args.path.trim();
        if path.is_empty() {
            return Err(CommandError::invalid_args("path 不能为空"));
        }
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let file = ProjectFile {
            format_version: PROJECT_FORMAT_VERSION,
            project: project.clone(),
        };
        let text = serde_json::to_string_pretty(&file)
            .map_err(|error| CommandError::new("io_error", format!("序列化工程失败：{error}")))?;
        let target = Path::new(path);
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    CommandError::new("io_error", format!("创建工程目录失败：{error}"))
                })?;
            }
        }
        std::fs::write(target, text.as_bytes())
            .map_err(|error| CommandError::new("io_error", format!("写工程失败：{error}")))?;
        Ok(CommandResponse::ok(json!({
            "path": path,
            "bytes": text.len(),
        })))
    }

    fn cmd_project_open(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: ProjectPathArgs = parse_args(args)?;
        let path = args.path.trim();
        if path.is_empty() {
            return Err(CommandError::invalid_args("path 不能为空"));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|error| CommandError::new("io_error", format!("读工程失败：{error}")))?;
        let file: ProjectFile = serde_json::from_str(&text).map_err(|error| {
            CommandError::invalid_args(format!("工程文件不是合法 JSON：{error}"))
        })?;
        if file.format_version != PROJECT_FORMAT_VERSION {
            return Err(CommandError::invalid_args(format!(
                "不支持的工程格式版本：{}",
                file.format_version
            )));
        }
        let (next_track, next_device) = validate_project(&file.project)?;
        let token = self.snapshot("project.open");
        self.project = Some(file.project);
        self.next_track = next_track;
        self.next_device = next_device;
        Ok(CommandResponse::ok(json!({
            "path": path,
            "project": self.project_summary(),
        }))
        .with_undo(token))
    }

    fn cmd_track_remove(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: TrackRemoveArgs = parse_args(args)?;
        let track_id = args.track_id;
        let exists = self
            .project
            .as_ref()
            .is_some_and(|project| project.tracks.iter().any(|track| track.id == track_id));
        if !exists {
            return Err(CommandError::not_found(format!("找不到轨道：{track_id}")));
        }
        let token = self.snapshot("track.remove");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        project.tracks.retain(|track| track.id != track_id);
        Ok(CommandResponse::ok(json!({ "track_id": track_id })).with_undo(token))
    }

    fn cmd_device_remove(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: DeviceRemoveArgs = parse_args(args)?;
        let path = args.path;
        let (track_id, device_id) = parse_device_path(&path)?;
        let exists = self.project.as_ref().is_some_and(|project| {
            project.tracks.iter().any(|track| {
                track.id == track_id && track.devices.iter().any(|device| device.id == device_id)
            })
        });
        if !exists {
            return Err(CommandError::not_found(format!("找不到设备：{path}")));
        }
        let token = self.snapshot("device.remove");
        let project = self
            .project
            .as_mut()
            .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
        let track = project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| CommandError::not_found(format!("找不到设备：{path}")))?;
        track.devices.retain(|device| device.id != device_id);
        Ok(CommandResponse::ok(json!({ "path": path })).with_undo(token))
    }

    fn cmd_redo(&mut self, args: &Value) -> Result<CommandResponse, CommandError> {
        let args: UndoArgs = parse_args(args)?;
        let steps = args.steps.unwrap_or(1);
        if !(1..=100).contains(&steps) {
            return Err(CommandError::invalid_args("steps 必须在 1 到 100 之间"));
        }
        if self.redo_stack.is_empty() {
            return Err(CommandError::nothing_to_redo());
        }
        let mut redone = Vec::new();
        for _ in 0..steps {
            let Some(snapshot) = self.redo_stack.pop() else {
                break;
            };
            self.history.push(Snapshot {
                project: self.project.clone(),
                command: snapshot.command.clone(),
            });
            self.project = snapshot.project;
            redone.push(snapshot.command);
        }
        Ok(CommandResponse::ok(json!({ "redone": redone })))
    }

    fn project_summary(&self) -> Value {
        let Some(project) = &self.project else {
            return Value::Null;
        };
        json!({
            "name": &project.name,
            "sample_rate": project.sample_rate,
            "track_count": project.tracks.len(),
            "tracks": project.tracks.iter().map(|track| json!({
                "id": &track.id,
                "name": &track.name,
                "source": &track.source,
                "device_count": track.devices.len(),
            })).collect::<Vec<_>>(),
        })
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

fn parse_param_path(path: &str) -> Result<(&str, &str, &str), CommandError> {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (
            Some("tracks"),
            Some(track),
            Some("devices"),
            Some(device),
            Some("params"),
            Some(name),
            None,
        ) => Ok((track, device, name)),
        _ => Err(CommandError::invalid_args(format!(
            "参数路径不合法：{path}"
        ))),
    }
}

fn parse_device_path(path: &str) -> Result<(&str, &str), CommandError> {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some("tracks"), Some(track), Some("devices"), Some(device), None) => Ok((track, device)),
        _ => Err(CommandError::invalid_args(format!(
            "设备路径不合法：{path}"
        ))),
    }
}

fn parse_id(id: &str, prefix: &str) -> Option<u64> {
    let number = id.strip_prefix(prefix)?.parse::<u64>().ok()?;
    (number > 0).then_some(number)
}

fn validate_project(project: &Project) -> Result<(u64, u64), CommandError> {
    if project.name.trim().is_empty() {
        return Err(CommandError::invalid_args("工程名不能为空"));
    }
    if !(8_000..=192_000).contains(&project.sample_rate) {
        return Err(CommandError::invalid_args(
            "sample_rate 必须在 8000 到 192000 之间",
        ));
    }

    let mut track_ids = BTreeSet::new();
    let mut device_ids = BTreeSet::new();
    let mut max_track = 0;
    let mut max_device = 0;

    for track in &project.tracks {
        let Some(number) = parse_id(&track.id, "track-") else {
            return Err(CommandError::invalid_args(format!(
                "轨道 ID 不合法：{}",
                track.id
            )));
        };
        max_track = max_track.max(number);

        if !track_ids.insert(track.id.as_str()) {
            return Err(CommandError::invalid_args(format!(
                "轨道 ID 重复：{}",
                track.id
            )));
        }
        if track.name.trim().is_empty() {
            return Err(CommandError::invalid_args(format!(
                "轨道名不能为空：{}",
                track.id
            )));
        }
        if let Source::Sine { frequency } = &track.source {
            if !frequency.is_finite()
                || *frequency <= 0.0
                || *frequency >= f64::from(project.sample_rate) / 2.0
            {
                return Err(CommandError::invalid_args(format!(
                    "轨道频率不合法：{}",
                    track.id
                )));
            }
        }

        for device in &track.devices {
            let Some(number) = parse_id(&device.id, "device-") else {
                return Err(CommandError::invalid_args(format!(
                    "设备 ID 不合法：{}",
                    device.id
                )));
            };
            max_device = max_device.max(number);

            if !device_ids.insert(device.id.as_str()) {
                return Err(CommandError::invalid_args(format!(
                    "设备 ID 重复：{}",
                    device.id
                )));
            }
            if device.kind.trim().is_empty() {
                return Err(CommandError::invalid_args(format!(
                    "设备类型不能为空：{}",
                    device.id
                )));
            }
            for (name, value) in &device.params {
                if !value.is_finite() {
                    return Err(CommandError::invalid_args(format!(
                        "设备参数不是有限数：{name}"
                    )));
                }
                if device.kind == "gain" && name == "gain" {
                    validate_gain(*value)?;
                }
            }
        }
    }

    Ok((max_track, max_device))
}

fn validate_gain(value: f64) -> Result<(), CommandError> {
    if !value.is_finite() || !(0.0..=4.0).contains(&value) {
        return Err(CommandError::invalid_args(
            "gain 必须是 0 到 4 之间的有限数",
        ));
    }
    Ok(())
}

fn track_gain(track: &Track) -> f64 {
    track
        .devices
        .iter()
        .filter(|device| device.kind == "gain")
        .filter_map(|device| device.params.get("gain").copied())
        .product::<f64>()
}

fn render_project(
    project: &Project,
    start_seconds: f64,
    duration_seconds: f64,
    path: &str,
) -> Result<Value, CommandError> {
    let sample_rate = project.sample_rate;
    let sample_rate_f64 = f64::from(sample_rate);
    let frames = (duration_seconds * sample_rate_f64).round() as usize;
    let mut mix = vec![0.0_f32; frames];
    for track in &project.tracks {
        let gain = track_gain(track);
        for (index, slot) in mix.iter_mut().enumerate() {
            let time = start_seconds + index as f64 / sample_rate_f64;
            let sample = match &track.source {
                Source::Silence => 0.0,
                Source::Sine { frequency } => {
                    (std::f64::consts::TAU * *frequency * time).sin() * 0.25
                },
            };
            *slot += (sample * gain) as f32;
        }
    }

    let mut peak = 0.0_f32;
    let mut sum_squares = 0.0_f64;
    let mut pcm = Vec::with_capacity(frames);
    for sample in &mix {
        let clamped = sample.clamp(-1.0, 1.0);
        peak = peak.max(clamped.abs());
        sum_squares += f64::from(clamped) * f64::from(clamped);
        pcm.push((clamped * 32_767.0) as i16);
    }
    let rms = if frames == 0 {
        0.0
    } else {
        (sum_squares / frames as f64).sqrt()
    };

    write_wav(Path::new(path), sample_rate, &pcm)
        .map_err(|error| CommandError::new("io_error", format!("写 WAV 失败：{error}")))?;

    Ok(json!({
        "path": path,
        "sample_rate": sample_rate,
        "frames": frames,
        "duration_seconds": frames as f64 / sample_rate_f64,
        "peak": peak,
        "rms": rms,
    }))
}

fn write_wav(path: &Path, sample_rate: u32, samples: &[i16]) -> io::Result<()> {
    let data_len = samples.len() * 2;
    let data_len_u32 = u32::try_from(data_len).map_err(|_| io::Error::other("WAV 数据过大"))?;
    let riff_len = 36_u32
        .checked_add(data_len_u32)
        .ok_or_else(|| io::Error::other("WAV 过大"))?;
    let mut bytes = Vec::with_capacity(44 + data_len);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&riff_len.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.saturating_mul(2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len_u32.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectCreateArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sample_rate: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackCreateArgs {
    name: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    frequency: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceAddArgs {
    track_id: String,
    kind: String,
    #[serde(default)]
    params: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamSetArgs {
    path: String,
    value: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderArgs {
    #[serde(default)]
    start_seconds: Option<f64>,
    #[serde(default)]
    duration_seconds: Option<f64>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UndoArgs {
    #[serde(default)]
    steps: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackRemoveArgs {
    track_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceRemoveArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectPathArgs {
    path: String,
}
