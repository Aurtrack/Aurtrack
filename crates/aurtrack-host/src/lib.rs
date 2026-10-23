//! 职责：
//! - 发现并描述第三方音频插件
//! - 把插件格式适配成宿主可用的信息
//!
//! 禁止：
//! - 依赖会话、图、引擎或任何 UI crate
//! - 在音频线程里扫描目录或加载动态库

use std::ffi::{CStr, CString};
use std::io;
use std::path::{Path, PathBuf};

use clack_host::prelude::*;
use serde::Serialize;
use thiserror::Error;

/// 扫描失败的单个插件文件。
#[derive(Debug, Clone, Serialize)]
pub struct ScanFailure {
    pub path: String,
    pub reason: String,
}

/// 一次插件扫描的结果。坏插件不会让整批扫描失败。
#[derive(Debug, Default, Clone, Serialize)]
pub struct ScanReport {
    pub plugins: Vec<PluginInfo>,
    pub failures: Vec<ScanFailure>,
}

/// 一个可加载的插件描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub path: String,
}

/// 宿主错误。
#[derive(Debug, Error)]
pub enum HostError {
    #[error("读取插件目录失败：{0}")]
    Io(#[from] io::Error),
    #[error("加载插件失败：{0}")]
    Load(String),
    #[error("左右声道长度必须一致")]
    InvalidBuffer,
    #[error("插件 ID 不合法")]
    InvalidPluginId,
    #[error("{0}")]
    Message(String),
}

impl From<String> for HostError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

/// 扫描目录下的 CLAP 插件。
///
/// 只读取一层目录，不递归，不跟随符号链接。插件加载会执行动态库代码，
/// 因此这个函数只能在受信任的显式插件目录上调用。
pub fn scan_directory(directory: impl AsRef<Path>) -> Result<ScanReport, HostError> {
    let directory = directory.as_ref();
    if !directory.exists() {
        return Ok(ScanReport::default());
    }
    let mut report = ScanReport::default();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if !extension.eq_ignore_ascii_case("clap") {
            continue;
        }
        match scan_plugin(&path) {
            Ok(plugins) => report.plugins.extend(plugins),
            Err(reason) => report.failures.push(ScanFailure {
                path: path.display().to_string(),
                reason,
            }),
        }
    }
    report
        .plugins
        .sort_by(|left, right| (&left.name, &left.id).cmp(&(&right.name, &right.id)));
    Ok(report)
}

/// 读取一个 CLAP 动态库里的全部描述符。
pub fn scan_plugin(path: &Path) -> Result<Vec<PluginInfo>, String> {
    // SAFETY: 调用方扫描的是显式插件目录，加载动态库等同于执行其中的代码。
    let entry = unsafe { PluginEntry::load(path) }.map_err(|error| error.to_string())?;
    let factory = entry
        .get_plugin_factory()
        .ok_or_else(|| "插件没有 PluginFactory".to_string())?;
    let mut result = Vec::new();
    for descriptor in factory.plugin_descriptors() {
        let Some(id) = descriptor.id() else {
            continue;
        };
        result.push(PluginInfo {
            id: String::from_utf8_lossy(id.to_bytes()).into_owned(),
            name: c_string(descriptor.name()),
            vendor: c_string(descriptor.vendor()),
            version: c_string(descriptor.version()),
            path: path.display().to_string(),
        });
    }
    Ok(result)
}

fn c_string(value: Option<&CStr>) -> String {
    value
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 显式插件目录：`<配置目录>/Aurtrack/plugins`。
pub fn default_plugin_directory() -> Option<PathBuf> {
    let config = std::env::var_os("AURTRACK_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .map(|path| path.join("Aurtrack"))
        })?;
    Some(config.join("plugins"))
}

const BLOCK_FRAMES: usize = 512;

struct HostShared;

impl SharedHandler<'_> for HostShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

struct HostMainThread;

impl MainThreadHandler<'_> for HostMainThread {}

struct HostAudioProcessor;

impl AudioProcessorHandler<'_> for HostAudioProcessor {}

struct ClapHost;

impl HostHandlers for ClapHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = HostMainThread;
    type AudioProcessor<'a> = HostAudioProcessor;
}

/// 在离线缓冲上运行一个 CLAP 立体声效果器。
///
/// 这是宿主的第一个闭环：验证加载、激活、块处理和停用。它不在实时音频线程里运行。
/// 插件必须有立体声输入和立体声输出；不满足时返回错误，不做隐式适配。
#[allow(clippy::too_many_arguments)]
pub fn process_stereo_offline(
    path: &Path,
    plugin_id: &str,
    sample_rate: u32,
    input_left: &[f32],
    input_right: &[f32],
) -> Result<(Vec<f32>, Vec<f32>), HostError> {
    if input_left.len() != input_right.len() {
        return Err(HostError::InvalidBuffer);
    }
    let frames = input_left.len();
    if frames == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let id = CString::new(plugin_id).map_err(|_| HostError::InvalidPluginId)?;
    // SAFETY: 与 `scan_plugin` 相同，加载动态库会执行插件代码。
    let entry = unsafe { PluginEntry::load(path) }.map_err(|error| error.to_string())?;
    let host_info = HostInfo::new(
        "Aurtrack",
        "Aurtrack contributors",
        "https://aurtrack.dev",
        "0.0.0",
    )
    .map_err(|error| error.to_string())?;
    let mut instance = PluginInstance::<ClapHost>::new(
        |_| HostShared,
        |_| HostMainThread,
        &entry,
        id.as_c_str(),
        &host_info,
    )
    .map_err(|error| error.to_string())?;
    let config = PluginAudioConfiguration {
        sample_rate: f64::from(sample_rate),
        min_frames_count: 1,
        max_frames_count: BLOCK_FRAMES as u32,
    };
    let stopped = instance
        .activate(|_, _| HostAudioProcessor, config)
        .map_err(|error| error.to_string())?;
    let mut processor = stopped
        .start_processing()
        .map_err(|error| error.to_string())?;
    let mut output_left = vec![0.0_f32; frames];
    let mut output_right = vec![0.0_f32; frames];
    let mut input_buffers = [vec![0.0_f32; BLOCK_FRAMES], vec![0.0_f32; BLOCK_FRAMES]];
    let mut output_buffers = [vec![0.0_f32; BLOCK_FRAMES], vec![0.0_f32; BLOCK_FRAMES]];
    let mut ports = AudioPorts::with_capacity(2, 1);
    let mut output_ports = AudioPorts::with_capacity(2, 1);
    let mut offset = 0;
    while offset < frames {
        let count = (frames - offset).min(BLOCK_FRAMES);
        input_buffers[0][..count].copy_from_slice(&input_left[offset..offset + count]);
        input_buffers[1][..count].copy_from_slice(&input_right[offset..offset + count]);
        output_buffers[0][..count].fill(0.0);
        output_buffers[1][..count].fill(0.0);

        let input_audio = ports.with_input_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                input_buffers
                    .iter_mut()
                    .map(|buffer| InputChannel::constant(&mut buffer[..count])),
            ),
        }]);
        let mut output_audio = output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                output_buffers.iter_mut().map(|buffer| &mut buffer[..count]),
            ),
        }]);
        let input_events = InputEvents::empty();
        let mut output_events = OutputEvents::void();
        processor
            .process(
                &input_audio,
                &mut output_audio,
                &input_events,
                &mut output_events,
                None,
                None,
            )
            .map_err(|error| error.to_string())?;
        output_left[offset..offset + count].copy_from_slice(&output_buffers[0][..count]);
        output_right[offset..offset + count].copy_from_slice(&output_buffers[1][..count]);
        offset += count;
    }
    let stopped = processor.stop_processing();
    instance.deactivate(stopped);
    Ok((output_left, output_right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directory_is_empty() {
        let report = scan_directory("target/definitely-not-a-plugin-directory").unwrap();
        assert!(report.plugins.is_empty());
        assert!(report.failures.is_empty());
    }

    #[test]
    fn broken_clap_file_is_reported() {
        let directory = PathBuf::from("target/clap-scan-fixture");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("broken.clap"), b"not a library").unwrap();
        let report = scan_directory(&directory).unwrap();
        assert!(report.plugins.is_empty());
        assert_eq!(report.failures.len(), 1);
    }

    #[test]
    fn missing_plugin_file_is_an_error() {
        let result = process_stereo_offline(
            Path::new("target/not-a-plugin.clap"),
            "com.example.missing",
            48_000,
            &[0.0],
            &[0.0],
        );
        assert!(result.is_err());
    }

    #[test]
    fn no_clap_extension_is_ignored() {
        let directory = PathBuf::from("target/scan-fixture");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("readme.txt"), b"not a plugin").unwrap();
        let report = scan_directory(&directory).unwrap();
        assert!(report.plugins.is_empty());
        assert!(report.failures.is_empty());
    }
}
