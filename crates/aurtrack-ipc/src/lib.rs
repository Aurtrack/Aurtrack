//! 职责：
//! - 桌面宿主进程内启动本地会话服务
//! - 提供 CLI / MCP 使用的客户端与本地/远程后端
//!
//! 禁止：
//! - 在协议层写业务逻辑
//! - 绑定非回环地址

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::needless_pass_by_value
)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::ffi::OsString;
use std::fmt;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aurtrack_command::{CommandError, CommandRequest, CommandResponse, ErrorResponse};
use aurtrack_runtime::Runtime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SESSION_FILE: &str = "aurtrack-session.json";
const SESSION_FILE_ENV: &str = "AURTRACK_SESSION_FILE";
const SESSION_DIR: &str = "Aurtrack";
const IO_TIMEOUT: Duration = Duration::from_secs(15);

fn session_dir() -> PathBuf {
    dirs::cache_dir()
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join(SESSION_DIR)
}

fn resolve_session_file(
    override_path: Option<OsString>,
    directory: PathBuf,
    file_name: &str,
) -> PathBuf {
    override_path.map_or_else(|| directory.join(file_name), PathBuf::from)
}

fn session_file() -> PathBuf {
    let name = if cfg!(test) {
        "aurtrack-session-test.json"
    } else {
        SESSION_FILE
    };
    if cfg!(test) {
        return session_dir().join(name);
    }
    resolve_session_file(std::env::var_os(SESSION_FILE_ENV), session_dir(), name)
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    port: u16,
    #[allow(dead_code)]
    pid: u32,
}

/// 桌面宿主持有的服务句柄；丢弃时清理端口文件。
#[derive(Debug)]
pub struct ServerHandle {
    port: u16,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        let path = session_file();
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(info) = serde_json::from_str::<SessionFile>(&content) {
                if info.port == self.port {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}

/// 在 127.0.0.1 的随机端口上启动会话服务，并把端口写入临时文件。
pub fn start_server<F>(handler: F) -> std::io::Result<ServerHandle>
where
    F: Fn(&CommandRequest) -> Result<CommandResponse, CommandError> + Send + Sync + 'static,
{
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let info = SessionFile {
        port,
        pid: std::process::id(),
    };
    let encoded =
        serde_json::to_vec(&info).map_err(|error| std::io::Error::other(error.to_string()))?;
    let path = session_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, encoded)?;

    let handler = Arc::new(handler);
    std::thread::Builder::new()
        .name("aurtrack-ipc".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    continue;
                };
                let handler = handler.clone();
                std::thread::spawn(move || handle_connection(stream, handler));
            }
        })?;

    Ok(ServerHandle { port })
}

fn handle_connection<F>(stream: TcpStream, handler: Arc<F>)
where
    F: Fn(&CommandRequest) -> Result<CommandResponse, CommandError> + Send + Sync + 'static,
{
    let Ok(writer_stream) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(stream);
    let mut writer = BufWriter::new(writer_stream);

    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let payload = match serde_json::from_str::<CommandRequest>(&line) {
            Ok(request) => match handler(&request) {
                Ok(response) => serde_json::to_string(&response),
                Err(error) => serde_json::to_string(&ErrorResponse::from(error)),
            },
            Err(error) => serde_json::to_string(&ErrorResponse::from(CommandError::invalid_args(
                format!("请求格式错误：{error}"),
            ))),
        };
        let Ok(payload) = payload else {
            break;
        };
        if writeln!(writer, "{payload}").is_err() {
            break;
        }
        if writer.flush().is_err() {
            break;
        }
    }
}

#[derive(Debug)]
pub enum ClientError {
    NoSession,
    Io(std::io::Error),
    Protocol(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSession => write!(
                formatter,
                "没有找到运行中的 Aurtrack 会话（会话文件：{}）",
                session_file().display()
            ),
            Self::Io(error) => write!(formatter, "连接会话失败：{error}"),
            Self::Protocol(message) => write!(formatter, "会话协议错误：{message}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// 到桌面会话服务的阻塞客户端。
pub struct SessionClient {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
}

impl fmt::Debug for SessionClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionClient")
            .finish_non_exhaustive()
    }
}

impl SessionClient {
    pub fn connect() -> Result<Self, ClientError> {
        let path = session_file();
        let content = std::fs::read_to_string(&path).map_err(|_| ClientError::NoSession)?;
        let info: SessionFile = serde_json::from_str(&content)
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        if info.port == 0 {
            return Err(ClientError::Protocol(format!(
                "会话文件端口为 0（{}）",
                path.display()
            )));
        }
        let stream = TcpStream::connect(("127.0.0.1", info.port))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let reader = BufReader::new(stream.try_clone()?);
        let writer = BufWriter::new(stream);
        Ok(Self { reader, writer })
    }

    pub fn execute(&mut self, request: &CommandRequest) -> Result<CommandResponse, CommandError> {
        let payload =
            serde_json::to_string(request).map_err(|error| ipc_error(error.to_string()))?;
        self.writer
            .write_all(payload.as_bytes())
            .map_err(|error| ipc_error(error.to_string()))?;
        self.writer
            .write_all(b"\n")
            .map_err(|error| ipc_error(error.to_string()))?;
        self.writer
            .flush()
            .map_err(|error| ipc_error(error.to_string()))?;

        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .map_err(|error| ipc_error(error.to_string()))?;
        if line.trim().is_empty() {
            return Err(ipc_error("会话连接已关闭".to_string()));
        }
        let value: Value =
            serde_json::from_str(&line).map_err(|error| ipc_error(error.to_string()))?;
        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            serde_json::from_value(value).map_err(|error| ipc_error(error.to_string()))
        } else {
            let error: CommandError =
                serde_json::from_value(value.get("error").cloned().unwrap_or(Value::Null))
                    .map_err(|error| ipc_error(error.to_string()))?;
            Err(error)
        }
    }
}

fn ipc_error(message: String) -> CommandError {
    CommandError::new("ipc_error", message)
}

/// CLI / MCP 的执行后端：连得上桌面就共享会话，否则退回本地运行时。
#[derive(Debug)]
pub enum Backend {
    Local(Runtime),
    Remote(SessionClient),
}

impl Backend {
    pub fn local() -> Self {
        Self::Local(Runtime::new())
    }

    pub fn remote() -> Result<Self, ClientError> {
        SessionClient::connect().map(Self::Remote)
    }

    pub fn execute(&mut self, request: &CommandRequest) -> Result<CommandResponse, CommandError> {
        match self {
            Self::Local(runtime) => runtime.execute(request),
            Self::Remote(client) => client.execute(request),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn server_and_client_round_trip() {
        let server =
            start_server(|request| Ok(CommandResponse::ok(json!({ "echo": request.cmd }))))
                .unwrap();
        let mut client = SessionClient::connect().unwrap();
        let response = client
            .execute(&CommandRequest {
                cmd: "demo".to_string(),
                args: json!({}),
            })
            .unwrap();
        assert_eq!(response.data["echo"], "demo");
        drop(server);
    }

    #[test]
    fn session_file_override_wins() {
        let path = resolve_session_file(
            Some(OsString::from("custom-session.json")),
            PathBuf::from("cache"),
            SESSION_FILE,
        );
        assert_eq!(path, PathBuf::from("custom-session.json"));
    }

    #[test]
    fn session_file_defaults_to_cache_dir() {
        let path = resolve_session_file(None, PathBuf::from("cache"), SESSION_FILE);
        assert_eq!(path, PathBuf::from("cache").join(SESSION_FILE));
    }
}
