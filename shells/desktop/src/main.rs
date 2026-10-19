#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::needless_pass_by_value
)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aurtrack_command::{CommandError, CommandRequest, CommandResponse};
use aurtrack_engine::{Playback, PlaybackPlan, TrackPlan};
use aurtrack_io::AudioHandle;
use aurtrack_session::{Project, Session, Source};
use serde::Serialize;
use tauri::State;

const LOOP_SECONDS: f64 = 5.0;

struct AppState {
    session: Mutex<Session>,
    playback: Arc<Playback>,
    audio: Mutex<Option<AudioHandle>>,
    sample_rate: u32,
}

#[derive(Debug, Serialize)]
struct AudioInfo {
    available: bool,
    sample_rate: u32,
    device: Option<String>,
}

#[derive(Debug, Serialize)]
struct TransportSnapshot {
    playing: bool,
    position: f64,
}

#[tauri::command]
fn execute(
    state: State<'_, AppState>,
    request: CommandRequest,
) -> Result<CommandResponse, CommandError> {
    let mut session = state
        .session
        .lock()
        .map_err(|_| CommandError::new("internal_error", "会话锁已损坏"))?;
    session.execute(&request)
}

#[tauri::command]
fn render_path() -> String {
    std::env::temp_dir()
        .join("aurtrack-demo.wav")
        .to_string_lossy()
        .into_owned()
}

#[tauri::command]
fn open_file(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if path.extension().and_then(|extension| extension.to_str()) != Some("wav") {
        return Err("只允许打开 wav 文件".to_string());
    }
    if !path.exists() {
        return Err("文件不存在".to_string());
    }
    open_with_system(&path).map_err(|error| error.to_string())
}

#[tauri::command]
fn audio_info(state: State<'_, AppState>) -> Result<AudioInfo, CommandError> {
    let audio = state
        .audio
        .lock()
        .map_err(|_| CommandError::new("internal_error", "音频状态锁已损坏"))?;
    Ok(match audio.as_ref() {
        Some(handle) => AudioInfo {
            available: true,
            sample_rate: handle.sample_rate(),
            device: Some(handle.device_name().to_string()),
        },
        None => AudioInfo {
            available: false,
            sample_rate: 0,
            device: None,
        },
    })
}

#[tauri::command]
fn transport_play(state: State<'_, AppState>) -> Result<(), CommandError> {
    let session = state
        .session
        .lock()
        .map_err(|_| CommandError::new("internal_error", "会话锁已损坏"))?;
    let project = session
        .project()
        .ok_or_else(|| CommandError::not_found("还没有工程，先执行 project.create"))?;
    let plan = build_plan(project, state.sample_rate);
    drop(session);
    state.playback.set_plan(plan);
    state.playback.play();
    Ok(())
}

#[tauri::command]
fn transport_stop(state: State<'_, AppState>) {
    state.playback.stop();
}

#[tauri::command]
fn transport_seek(state: State<'_, AppState>, seconds: f64) -> Result<(), CommandError> {
    if !seconds.is_finite() {
        return Err(CommandError::invalid_args("seconds 必须是有限数"));
    }
    state.playback.seek(seconds);
    Ok(())
}

#[tauri::command]
fn transport_state(state: State<'_, AppState>) -> TransportSnapshot {
    TransportSnapshot {
        playing: state.playback.is_playing(),
        position: state.playback.position_seconds(),
    }
}

#[tauri::command]
fn set_master_gain(state: State<'_, AppState>, gain: f64) -> Result<(), CommandError> {
    if !gain.is_finite() || !(0.0..=4.0).contains(&gain) {
        return Err(CommandError::invalid_args("gain 必须在 0 到 4 之间"));
    }
    state.playback.set_master_gain(gain);
    Ok(())
}

fn build_plan(project: &Project, sample_rate: u32) -> PlaybackPlan {
    let tracks = project
        .tracks
        .iter()
        .map(|track| {
            // 当前竖切：实时音量由 master gain 控制。
            // 设备链增益只在离线渲染里生效，等音频图落地后再统一。
            let frequency = match &track.source {
                Source::Sine { frequency } => Some(*frequency),
                Source::Silence => None,
            };
            TrackPlan {
                frequency,
                gain: 1.0,
            }
        })
        .collect();
    PlaybackPlan {
        sample_rate,
        loop_seconds: LOOP_SECONDS,
        tracks,
    }
}

fn open_with_system(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg("start")
            .arg("")
            .arg(path.as_os_str())
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

fn main() {
    let playback = Arc::new(Playback::new(48_000));
    let audio = match aurtrack_io::spawn(playback.clone()) {
        Ok(handle) => Some(handle),
        Err(error) => {
            eprintln!("音频设备不可用：{error}");
            None
        },
    };
    let sample_rate = audio.as_ref().map_or(48_000, AudioHandle::sample_rate);
    let state = AppState {
        session: Mutex::new(Session::default()),
        playback,
        audio: Mutex::new(audio),
        sample_rate,
    };
    if let Err(error) = tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            execute,
            render_path,
            open_file,
            audio_info,
            transport_play,
            transport_stop,
            transport_seek,
            transport_state,
            set_master_gain
        ])
        .run(tauri::generate_context!())
    {
        eprintln!("Aurtrack 启动失败：{error}");
        std::process::exit(1);
    }
}
