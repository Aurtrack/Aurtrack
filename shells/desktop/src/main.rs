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
use aurtrack_ipc::ServerHandle;
use aurtrack_runtime::{AudioInfo, Runtime};
use tauri::State;

struct AppState {
    runtime: Arc<Mutex<Runtime>>,
    _server: ServerHandle,
}

#[tauri::command]
fn execute(
    state: State<'_, AppState>,
    request: CommandRequest,
) -> Result<CommandResponse, CommandError> {
    let mut runtime = state
        .runtime
        .lock()
        .map_err(|_| CommandError::new("internal_error", "运行时锁已损坏"))?;
    runtime.execute(&request)
}

#[tauri::command]
fn audio_info(state: State<'_, AppState>) -> Result<AudioInfo, CommandError> {
    let runtime = state
        .runtime
        .lock()
        .map_err(|_| CommandError::new("internal_error", "运行时锁已损坏"))?;
    Ok(runtime.audio_info())
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

#[tauri::command]
fn pick_project_path(mode: String) -> Option<String> {
    let dialog = rfd::FileDialog::new()
        .add_filter("Aurtrack 工程", &["json"])
        .set_file_name("project.aurtrack.json");
    let path = if mode == "save" {
        dialog.set_title("保存 Aurtrack 工程").save_file()
    } else {
        dialog.set_title("打开 Aurtrack 工程").pick_file()
    };
    path.map(|path| path.to_string_lossy().into_owned())
}

fn main() {
    let mut runtime = Runtime::new();
    if let Err(error) = runtime.ensure_audio() {
        eprintln!("音频设备不可用：{error}");
    }
    let runtime = Arc::new(Mutex::new(runtime));

    let server = {
        let runtime = runtime.clone();
        aurtrack_ipc::start_server(move |request| {
            let mut runtime = runtime
                .lock()
                .map_err(|_| CommandError::new("internal_error", "运行时锁已损坏"))?;
            runtime.execute(request)
        })
    };
    let server = match server {
        Ok(server) => server,
        Err(error) => {
            eprintln!("本地会话服务启动失败：{error}");
            std::process::exit(1);
        },
    };

    let state = AppState {
        runtime,
        _server: server,
    };
    if let Err(error) = tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            execute,
            render_path,
            open_file,
            audio_info,
            pick_project_path
        ])
        .run(tauri::generate_context!())
    {
        eprintln!("Aurtrack 启动失败：{error}");
        std::process::exit(1);
    }
}
