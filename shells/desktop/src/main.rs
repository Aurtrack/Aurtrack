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
use std::sync::Mutex;

use aurtrack_command::{CommandError, CommandRequest, CommandResponse};
use aurtrack_session::Session;
use tauri::State;

struct AppState {
    session: Mutex<Session>,
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
    let state = AppState {
        session: Mutex::new(Session::default()),
    };
    if let Err(error) = tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![execute, render_path, open_file])
        .run(tauri::generate_context!())
    {
        eprintln!("Aurtrack 启动失败：{error}");
        std::process::exit(1);
    }
}
