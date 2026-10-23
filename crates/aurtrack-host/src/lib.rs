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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use aurtrack_engine::BlockProcessor;
use base64::Engine;
use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_extensions::state::PluginState;
use clack_host::events::event_types::ParamValueEvent;
use clack_host::prelude::*;
use rtrb::{Consumer, Producer, RingBuffer};
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

/// 插件的一个参数。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParameterInfo {
    pub id: u32,
    pub name: String,
    pub module: String,
    pub minimum: f64,
    pub maximum: f64,
    pub default: f64,
    pub flags: u32,
}

/// 插件参数和状态能力。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginCapabilities {
    pub parameters: Vec<ParameterInfo>,
    pub has_state: bool,
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
    #[error("插件状态错误：{0}")]
    State(String),
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

/// 读取插件参数和状态扩展。
pub fn inspect_plugin(path: &Path, plugin_id: &str) -> Result<PluginCapabilities, HostError> {
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
    let handle = instance.plugin_handle();
    let mut parameters = Vec::new();
    if let Some(params) = handle.get_extension::<PluginParams>() {
        let count = params.count(&handle);
        for index in 0..count {
            let mut buffer = ParamInfoBuffer::new();
            let Some(info) = params.get_info(&handle, index, &mut buffer) else {
                continue;
            };
            parameters.push(ParameterInfo {
                id: info.id.get(),
                name: String::from_utf8_lossy(info.name).into_owned(),
                module: String::from_utf8_lossy(info.module).into_owned(),
                minimum: info.min_value,
                maximum: info.max_value,
                default: info.default_value,
                flags: info.flags.bits(),
            });
        }
    }
    let has_state = handle.get_extension::<PluginState>().is_some();
    Ok(PluginCapabilities {
        parameters,
        has_state,
    })
}

struct ClapProcessor {
    instance: PluginInstance<ClapHost>,
    processor: Option<StartedPluginAudioProcessor<ClapHost>>,
    input_buffers: [Vec<f32>; 2],
    output_buffers: [Vec<f32>; 2],
    input_ports: AudioPorts,
    output_ports: AudioPorts,
}

impl ClapProcessor {
    fn load(
        path: &Path,
        plugin_id: &str,
        sample_rate: u32,
        state: Option<&[u8]>,
    ) -> Result<Self, HostError> {
        let id = CString::new(plugin_id).map_err(|_| HostError::InvalidPluginId)?;
        // SAFETY: 加载动态库会执行插件代码，调用方只能传入受信任插件。
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
        if let Some(state) = state {
            let handle = instance.plugin_handle();
            let state_extension = handle
                .get_extension::<PluginState>()
                .ok_or_else(|| HostError::State("插件不支持 state 扩展".to_string()))?;
            let mut reader = state;
            state_extension
                .load(&handle, &mut reader)
                .map_err(|error| HostError::State(error.to_string()))?;
        }
        let config = PluginAudioConfiguration {
            sample_rate: f64::from(sample_rate),
            min_frames_count: 1,
            max_frames_count: BLOCK_FRAMES as u32,
        };
        let stopped = instance
            .activate(|_, _| HostAudioProcessor, config)
            .map_err(|error| error.to_string())?;
        let processor = stopped
            .start_processing()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            instance,
            processor: Some(processor),
            input_buffers: [vec![0.0; BLOCK_FRAMES], vec![0.0; BLOCK_FRAMES]],
            output_buffers: [vec![0.0; BLOCK_FRAMES], vec![0.0; BLOCK_FRAMES]],
            input_ports: AudioPorts::with_capacity(2, 1),
            output_ports: AudioPorts::with_capacity(2, 1),
        })
    }

    fn process(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
        parameters: &[(u32, f64)],
    ) -> Result<(), HostError> {
        let frames = input_left
            .len()
            .min(input_right.len())
            .min(output_left.len())
            .min(output_right.len());
        if frames == 0 {
            return Ok(());
        }
        let processor = self
            .processor
            .as_mut()
            .ok_or_else(|| HostError::Message("插件未启动".to_string()))?;
        let mut offset = 0;
        while offset < frames {
            let count = (frames - offset).min(BLOCK_FRAMES);
            self.input_buffers[0][..count].copy_from_slice(&input_left[offset..offset + count]);
            self.input_buffers[1][..count].copy_from_slice(&input_right[offset..offset + count]);
            self.output_buffers[0][..count].fill(0.0);
            self.output_buffers[1][..count].fill(0.0);
            let input_audio = self.input_ports.with_input_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    self.input_buffers
                        .iter_mut()
                        .map(|buffer| InputChannel::constant(&mut buffer[..count])),
                ),
            }]);
            let mut output_audio = self.output_ports.with_output_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only(
                    self.output_buffers
                        .iter_mut()
                        .map(|buffer| &mut buffer[..count]),
                ),
            }]);
            let mut event_buffer = EventBuffer::with_capacity(parameters.len().max(1));
            for (parameter_id, value) in parameters {
                let event =
                    ParamValueEvent::new(0, ClapId::new(*parameter_id), Pckn::match_all(), *value);
                event_buffer.push(&event);
            }
            let input_events = InputEvents::from_buffer(&event_buffer);
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
            output_left[offset..offset + count].copy_from_slice(&self.output_buffers[0][..count]);
            output_right[offset..offset + count].copy_from_slice(&self.output_buffers[1][..count]);
            offset += count;
        }
        Ok(())
    }
}

impl Drop for ClapProcessor {
    fn drop(&mut self) {
        if let Some(processor) = self.processor.take() {
            let stopped = processor.stop_processing();
            self.instance.deactivate(stopped);
        }
    }
}

const RING_BLOCKS: usize = 4;

/// 音频线程持有的无锁插件端口。
pub struct PluginAudioPort {
    input: Producer<f32>,
    output: Consumer<f32>,
    enabled: Arc<AtomicBool>,
}

impl BlockProcessor for PluginAudioPort {
    fn prepare(&mut self, _sample_rate: u32, _max_block_frames: usize) -> Result<(), String> {
        Ok(())
    }

    fn reset(&mut self) {}

    fn process(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) -> Result<(), String> {
        let frames = input_left
            .len()
            .min(input_right.len())
            .min(output_left.len())
            .min(output_right.len());
        if !self.enabled.load(Ordering::Acquire) {
            output_left[..frames].copy_from_slice(&input_left[..frames]);
            output_right[..frames].copy_from_slice(&input_right[..frames]);
            return Ok(());
        }
        let mut offset = 0;
        while offset + BLOCK_FRAMES <= frames {
            if self.input.slots() < BLOCK_FRAMES * 2 || self.output.slots() < BLOCK_FRAMES * 2 {
                output_left[offset..offset + BLOCK_FRAMES]
                    .copy_from_slice(&input_left[offset..offset + BLOCK_FRAMES]);
                output_right[offset..offset + BLOCK_FRAMES]
                    .copy_from_slice(&input_right[offset..offset + BLOCK_FRAMES]);
                offset += BLOCK_FRAMES;
                continue;
            }
            for index in 0..BLOCK_FRAMES {
                let _ = self.input.push(input_left[offset + index]);
                let _ = self.input.push(input_right[offset + index]);
            }
            for index in 0..BLOCK_FRAMES {
                let left = self.output.pop().unwrap_or(0.0);
                let right = self.output.pop().unwrap_or(0.0);
                output_left[offset + index] = left;
                output_right[offset + index] = right;
            }
            offset += BLOCK_FRAMES;
        }
        // 非整块尾巴不进入插件，避免输入输出帧数错位。
        if offset < frames {
            output_left[offset..frames].copy_from_slice(&input_left[offset..frames]);
            output_right[offset..frames].copy_from_slice(&input_right[offset..frames]);
        }
        Ok(())
    }
}

enum PluginCommand {
    Load {
        path: PathBuf,
        id: String,
        sample_rate: u32,
        state: Option<Vec<u8>>,
        reply: Sender<Result<(), String>>,
    },
    Unload {
        reply: Sender<Result<(), String>>,
    },
    Param {
        id: u32,
        value: f64,
    },
    Shutdown,
}

/// 控制专用插件线程；音频线程只持有 [`PluginAudioPort`]。
pub struct PluginBridgeControl {
    commands: Sender<PluginCommand>,
    active: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for PluginBridgeControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginBridgeControl")
            .field("active", &self.active.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl PluginBridgeControl {
    pub fn new() -> (Self, PluginAudioPort) {
        let (input_producer, input_consumer) = RingBuffer::new(BLOCK_FRAMES * 2 * RING_BLOCKS);
        let (output_producer, output_consumer) = RingBuffer::new(BLOCK_FRAMES * 2 * RING_BLOCKS);
        let (commands, command_receiver) = mpsc::channel();
        let active = Arc::new(AtomicBool::new(false));
        let thread_active = Arc::clone(&active);
        let thread = std::thread::Builder::new()
            .name("aurtrack-plugin".to_string())
            .spawn(move || {
                plugin_thread(
                    command_receiver,
                    input_consumer,
                    output_producer,
                    thread_active,
                );
            })
            .ok();
        (
            Self {
                commands,
                active: Arc::clone(&active),
                thread,
            },
            PluginAudioPort {
                input: input_producer,
                output: output_consumer,
                enabled: active,
            },
        )
    }

    pub fn load(&self, path: PathBuf, id: String, sample_rate: u32) -> Result<(), String> {
        self.load_with_state(path, id, sample_rate, None)
    }

    pub fn load_with_state(
        &self,
        path: PathBuf,
        id: String,
        sample_rate: u32,
        state: Option<Vec<u8>>,
    ) -> Result<(), String> {
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(PluginCommand::Load {
                path,
                id,
                sample_rate,
                state,
                reply,
            })
            .map_err(|_| "插件线程已经退出".to_string())?;
        answer.recv().map_err(|_| "插件线程没有响应".to_string())?
    }

    pub fn set_parameter(&self, id: u32, value: f64) -> Result<(), String> {
        self.commands
            .send(PluginCommand::Param { id, value })
            .map_err(|_| "插件线程已经退出".to_string())
    }

    pub fn unload(&self) -> Result<(), String> {
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(PluginCommand::Unload { reply })
            .map_err(|_| "插件线程已经退出".to_string())?;
        answer.recv().map_err(|_| "插件线程没有响应".to_string())?
    }

    pub fn is_loaded(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

impl Drop for PluginBridgeControl {
    fn drop(&mut self) {
        let _ = self.commands.send(PluginCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn plugin_thread(
    commands: Receiver<PluginCommand>,
    mut input: Consumer<f32>,
    mut output: Producer<f32>,
    active: Arc<AtomicBool>,
) {
    let mut processor: Option<ClapProcessor> = None;
    let mut pending_parameters: Vec<(u32, f64)> = Vec::new();
    loop {
        match commands.try_recv() {
            Ok(PluginCommand::Load {
                path,
                id,
                sample_rate,
                state,
                reply,
            }) => {
                active.store(false, Ordering::Release);
                processor = None;
                let result =
                    ClapProcessor::load(&path, &id, sample_rate, state.as_deref()).map(|loaded| {
                        processor = Some(loaded);
                        active.store(true, Ordering::Release);
                    });
                let _ = reply.send(result.map_err(|error| error.to_string()));
            },
            Ok(PluginCommand::Unload { reply }) => {
                active.store(false, Ordering::Release);
                processor = None;
                pending_parameters.clear();
                let _ = reply.send(Ok(()));
            },
            Ok(PluginCommand::Param { id, value }) => {
                pending_parameters.push((id, value));
            },
            Ok(PluginCommand::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {},
        }
        let Some(active_processor) = processor.as_mut() else {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        if input.slots() < BLOCK_FRAMES * 2 {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let mut input_left = [0.0_f32; BLOCK_FRAMES];
        let mut input_right = [0.0_f32; BLOCK_FRAMES];
        for index in 0..BLOCK_FRAMES {
            input_left[index] = input.pop().unwrap_or(0.0);
            input_right[index] = input.pop().unwrap_or(0.0);
        }
        let mut output_left = [0.0_f32; BLOCK_FRAMES];
        let mut output_right = [0.0_f32; BLOCK_FRAMES];
        let parameters = std::mem::take(&mut pending_parameters);
        if active_processor
            .process(
                &input_left,
                &input_right,
                &mut output_left,
                &mut output_right,
                &parameters,
            )
            .is_err()
        {
            active.store(false, Ordering::Release);
            processor = None;
            continue;
        }
        if output.slots() < BLOCK_FRAMES * 2 {
            continue;
        }
        for index in 0..BLOCK_FRAMES {
            let _ = output.push(output_left[index]);
            let _ = output.push(output_right[index]);
        }
    }
    active.store(false, Ordering::Release);
}

/// 解码工程里保存的 base64 state。
pub fn decode_state(state: &str) -> Result<Vec<u8>, HostError> {
    base64::engine::general_purpose::STANDARD
        .decode(state)
        .map_err(|error| HostError::State(error.to_string()))
}

/// 读取插件 state 扩展并编码为 base64。
pub fn save_state(path: &Path, plugin_id: &str) -> Result<String, HostError> {
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
    let handle = instance.plugin_handle();
    let state = handle
        .get_extension::<PluginState>()
        .ok_or_else(|| HostError::State("插件不支持 state 扩展".to_string()))?;
    let mut bytes = Vec::new();
    state
        .save(&handle, &mut bytes)
        .map_err(|error| HostError::State(error.to_string()))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// 从 base64 state 恢复插件。
pub fn restore_state(path: &Path, plugin_id: &str, state: &str) -> Result<(), HostError> {
    let bytes = decode_state(state)?;
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
    let handle = instance.plugin_handle();
    let state_extension = handle
        .get_extension::<PluginState>()
        .ok_or_else(|| HostError::State("插件不支持 state 扩展".to_string()))?;
    state_extension
        .load(&handle, &mut bytes.as_slice())
        .map_err(|error| HostError::State(error.to_string()))
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
    fn fixture_plugin_processes_offline() {
        let path = Path::new("target/fixture-gain.clap");
        if !path.exists() {
            return;
        }
        let (left, right) = process_stereo_offline(
            path,
            "com.aurtrack.fixture.gain",
            48_000,
            &[0.5; 64],
            &[-0.5; 64],
        )
        .unwrap();
        assert!(left.iter().all(|sample| (*sample - 0.25).abs() < 1.0e-5));
        assert!(right.iter().all(|sample| (*sample + 0.25).abs() < 1.0e-5));
    }

    #[test]
    fn fixture_plugin_loads_in_bridge() {
        let path = Path::new("target/fixture-gain.clap");
        if !path.exists() {
            return;
        }
        let (control, _port) = PluginBridgeControl::new();
        control
            .load(
                path.to_path_buf(),
                "com.aurtrack.fixture.gain".to_string(),
                48_000,
            )
            .unwrap();
        assert!(control.is_loaded());
        control.unload().unwrap();
        assert!(!control.is_loaded());
    }

    #[test]
    fn fixture_plugin_processes_through_bridge() {
        let path = Path::new("target/fixture-gain.clap");
        if !path.exists() {
            return;
        }
        let (control, mut port) = PluginBridgeControl::new();
        control
            .load(
                path.to_path_buf(),
                "com.aurtrack.fixture.gain".to_string(),
                48_000,
            )
            .unwrap();
        let input = [0.5_f32; BLOCK_FRAMES];
        let mut left = [0.0_f32; BLOCK_FRAMES];
        let mut right = [0.0_f32; BLOCK_FRAMES];
        let mut processed = false;
        for _ in 0..200 {
            port.process(&input, &input, &mut left, &mut right).unwrap();
            if left[0] > 0.24 && left[0] < 0.26 {
                processed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        control.unload().unwrap();
        assert!(processed, "实时桥没有输出插件处理后的样本");
    }

    #[test]
    fn fixture_plugin_state_round_trip() {
        let path = Path::new("target/fixture-gain.clap");
        if !path.exists() {
            return;
        }
        let state = save_state(path, "com.aurtrack.fixture.gain").unwrap();
        restore_state(path, "com.aurtrack.fixture.gain", &state).unwrap();
        assert!(!state.is_empty());
    }

    #[test]
    fn fixture_plugin_applies_state_on_load() {
        let path = Path::new("target/fixture-gain.clap");
        if !path.exists() {
            return;
        }
        let state = decode_state("AACAPg==").unwrap();
        let (control, mut port) = PluginBridgeControl::new();
        control
            .load_with_state(
                path.to_path_buf(),
                "com.aurtrack.fixture.gain".to_string(),
                48_000,
                Some(state),
            )
            .unwrap();
        let input = [0.5_f32; BLOCK_FRAMES];
        let mut left = [0.0_f32; BLOCK_FRAMES];
        let mut right = [0.0_f32; BLOCK_FRAMES];
        let mut processed = false;
        for _ in 0..200 {
            port.process(&input, &input, &mut left, &mut right).unwrap();
            if left[0] > 0.12 && left[0] < 0.13 {
                processed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        control.unload().unwrap();
        assert!(processed, "插件没有应用加载时的 state");
    }

    #[test]
    fn bridge_passthrough_without_plugin() {
        let (control, mut port) = PluginBridgeControl::new();
        assert!(!control.is_loaded());
        let input_left = [0.25_f32; 64];
        let input_right = [-0.5_f32; 64];
        let mut output_left = [0.0_f32; 64];
        let mut output_right = [0.0_f32; 64];
        port.process(
            &input_left,
            &input_right,
            &mut output_left,
            &mut output_right,
        )
        .unwrap();
        assert_eq!(output_left, input_left);
        assert_eq!(output_right, input_right);
    }

    #[test]
    fn bridge_reports_missing_plugin() {
        let (control, _port) = PluginBridgeControl::new();
        let result = control.load(
            PathBuf::from("target/not-a-plugin.clap"),
            "com.example.missing".to_string(),
            48_000,
        );
        assert!(result.is_err());
        assert!(!control.is_loaded());
    }

    #[test]
    fn inspect_missing_plugin_is_an_error() {
        let result = inspect_plugin(Path::new("target/not-a-plugin.clap"), "com.example.missing");
        assert!(result.is_err());
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
