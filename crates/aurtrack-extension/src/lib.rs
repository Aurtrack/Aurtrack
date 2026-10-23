//! 职责：
//! - 发现外部进程形式的 Aurtrack 扩展
//! - 校验扩展清单和命令声明
//! - 以 JSON stdin/stdout 调用扩展命令
//!
//! 禁止：
//! - 依赖会话、图、引擎或任何 UI crate
//! - 直接加载动态库
//! - 在音频线程里启动进程

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// 当前扩展 API 版本。
pub const EXTENSION_API_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub commands: Vec<ExtensionCommand>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionCommand {
    pub id: String,
    pub name: String,
    pub executable: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtensionInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub directory: String,
    pub commands: Vec<ExtensionCommand>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtensionFailure {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ExtensionReport {
    pub extensions: Vec<ExtensionInfo>,
    pub failures: Vec<ExtensionFailure>,
}

#[derive(Debug, Error)]
pub enum ExtensionError {
    #[error("读取扩展目录失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("扩展清单不合法：{0}")]
    Manifest(String),
    #[error("找不到扩展：{0}")]
    NotFound(String),
    #[error("扩展命令执行失败：{0}")]
    Execution(String),
    #[error("扩展返回的不是 JSON 对象")]
    InvalidResponse,
}

/// 默认扩展目录：`<配置目录>/Aurtrack/extensions`。
pub fn default_extension_directory() -> Option<PathBuf> {
    let config = std::env::var_os("AURTRACK_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .map(|path| path.join("Aurtrack"))
        })?;
    Some(config.join("extensions"))
}

/// 扫描一层扩展目录。坏扩展只记录错误，不影响其他扩展。
pub fn scan_extensions(directory: impl AsRef<Path>) -> Result<ExtensionReport, ExtensionError> {
    let directory = directory.as_ref();
    if !directory.exists() {
        return Ok(ExtensionReport::default());
    }
    let mut report = ExtensionReport::default();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match load_extension(&path) {
            Ok(info) => report.extensions.push(info),
            Err(error) => report.failures.push(ExtensionFailure {
                path: path.display().to_string(),
                reason: error.to_string(),
            }),
        }
    }
    report
        .extensions
        .sort_by(|left, right| left.name.cmp(&right.name));
    Ok(report)
}

pub fn load_extension(directory: &Path) -> Result<ExtensionInfo, ExtensionError> {
    let manifest_path = directory.join("manifest.json");
    let bytes = std::fs::read(&manifest_path)?;
    let manifest: ExtensionManifest = serde_json::from_slice(&bytes)
        .map_err(|error| ExtensionError::Manifest(error.to_string()))?;
    validate_manifest(&manifest, directory)?;
    Ok(ExtensionInfo {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        api_version: manifest.api_version,
        directory: directory.display().to_string(),
        commands: manifest.commands,
    })
}

/// 调用扩展命令；输入是 `args`，stdout 必须是一个 JSON 对象。
pub fn run_extension_command(
    info: &ExtensionInfo,
    command_id: &str,
    args: &Value,
) -> Result<Value, ExtensionError> {
    let command = info
        .commands
        .iter()
        .find(|command| command.id == command_id)
        .ok_or_else(|| ExtensionError::NotFound(format!("{}.{}", info.id, command_id)))?;
    let directory = PathBuf::from(&info.directory);
    let executable = safe_executable(&directory, &command.executable)?;
    let mut child = Command::new(&executable)
        .current_dir(&directory)
        .env("AURTRACK_EXTENSION_ID", &info.id)
        .env("AURTRACK_EXTENSION_API", EXTENSION_API_VERSION.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ExtensionError::Execution(error.to_string()))?;
    let request = serde_json::json!({
        "extension": info.id,
        "command": command.id,
        "args": args,
    });
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(request.to_string().as_bytes())
            .map_err(|error| ExtensionError::Execution(error.to_string()))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| ExtensionError::Execution(error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ExtensionError::Execution(format!(
            "退出码 {:?}：{}",
            output.status.code(),
            stderr.trim()
        )));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| ExtensionError::Execution(error.to_string()))?;
    if !value.is_object() {
        return Err(ExtensionError::InvalidResponse);
    }
    Ok(value)
}

fn validate_manifest(manifest: &ExtensionManifest, directory: &Path) -> Result<(), ExtensionError> {
    if manifest.api_version != EXTENSION_API_VERSION {
        return Err(ExtensionError::Manifest(format!(
            "不支持的 api_version：{}",
            manifest.api_version
        )));
    }
    validate_id(&manifest.id)?;
    if manifest.commands.is_empty() {
        return Err(ExtensionError::Manifest("至少需要一条命令".to_string()));
    }
    let mut ids = std::collections::BTreeSet::new();
    for command in &manifest.commands {
        validate_id(&command.id)?;
        if !ids.insert(&command.id) {
            return Err(ExtensionError::Manifest(format!(
                "重复的命令 ID：{}",
                command.id
            )));
        }
        safe_executable(directory, &command.executable)?;
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), ExtensionError> {
    if id.is_empty()
        || id.len() > 64
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(ExtensionError::Manifest(format!("ID 不合法：{id}")));
    }
    Ok(())
}

fn safe_executable(directory: &Path, executable: &str) -> Result<PathBuf, ExtensionError> {
    let relative = Path::new(executable);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ExtensionError::Manifest(
            "executable 必须是扩展目录内的相对路径".to_string(),
        ));
    }
    let path = directory.join(relative);
    if !path.is_file() {
        return Err(ExtensionError::Manifest(format!(
            "找不到可执行文件：{}",
            path.display()
        )));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_manifest_is_rejected() {
        let directory = PathBuf::from("target/extension-fixture");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("manifest.json"),
            br#"{"id":"bad id","name":"Bad","version":"0.1.0","api_version":1,"commands":[]}"#,
        )
        .unwrap();
        assert!(load_extension(&directory).is_err());
    }

    #[test]
    fn missing_directory_is_empty() {
        let report = scan_extensions("target/not-an-extension-directory").unwrap();
        assert!(report.extensions.is_empty());
        assert!(report.failures.is_empty());
    }
}
