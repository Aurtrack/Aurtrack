//! 职责：
//! - 打开系统默认输出设备
//! - 在音频回调里按块拉取 `aurtrack-engine` 的信号
//!
//! 禁止：
//! - 在回调里分配、加锁、写日志（错误回调除外）
//! - 依赖 UI 或会话

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::cast_possible_truncation,
    clippy::cast_lossless
)]

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use aurtrack_engine::{BlockProcessor, Playback};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream};

#[derive(Debug)]
pub enum AudioError {
    NoDevice,
    Config(String),
    UnsupportedFormat(SampleFormat),
    Build(String),
    Play(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => write!(formatter, "找不到默认输出设备"),
            Self::Config(message) => write!(formatter, "读取设备配置失败：{message}"),
            Self::UnsupportedFormat(format) => write!(formatter, "不支持的采样格式：{format:?}"),
            Self::Build(message) => write!(formatter, "创建音频流失败：{message}"),
            Self::Play(message) => write!(formatter, "启动音频流失败：{message}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// 保持音频流存活；丢弃它就会停止播放。
pub struct AudioOutput {
    _stream: Stream,
    sample_rate: u32,
    device_name: String,
}

impl fmt::Debug for AudioOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AudioOutput")
            .field("sample_rate", &self.sample_rate)
            .field("device_name", &self.device_name)
            .finish_non_exhaustive()
    }
}

/// 音频线程创建的句柄。只包含可 `Send` 的元数据。
#[derive(Debug)]
pub struct AudioHandle {
    sample_rate: u32,
    device_name: String,
    healthy: Arc<AtomicBool>,
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl AudioHandle {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// 音频流出错后会变为 `false`；调用方应丢弃句柄并重新打开设备。
    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 在专用线程里打开设备并保持音频流存活。
///
/// `cpal::Stream` 在部分平台不是 `Send`，所以不能直接放进 Tauri 状态；
/// 这里让线程持有它，主线程只拿元数据。丢弃句柄会唤醒线程并释放音频流，
/// 避免切换设备或重开输出后留下旧流。
pub fn spawn(playback: Arc<Playback>) -> Result<AudioHandle, AudioError> {
    spawn_with_master(playback, None)
}

/// 打开音频设备，并在主输出后挂一个实时块处理器。
pub fn spawn_with_master(
    playback: Arc<Playback>,
    master: Option<Box<dyn BlockProcessor>>,
) -> Result<AudioHandle, AudioError> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let (stop, stop_signal) = std::sync::mpsc::channel();
    let healthy = Arc::new(AtomicBool::new(true));
    let stream_health = Arc::clone(&healthy);
    let thread = std::thread::Builder::new()
        .name("aurtrack-audio".to_string())
        .spawn(move || match start(playback, master, &stream_health) {
            Ok(output) => {
                let metadata = (output.sample_rate(), output.device_name().to_string());
                if sender.send(Ok(metadata)).is_err() {
                    return;
                }
                let _ = stop_signal.recv();
                drop(output);
            },
            Err(error) => {
                let _ = sender.send(Err(error));
            },
        })
        .map_err(|error| AudioError::Build(error.to_string()))?;

    match receiver.recv() {
        Ok(Ok((sample_rate, device_name))) => Ok(AudioHandle {
            sample_rate,
            device_name,
            healthy,
            stop,
            thread: Some(thread),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        },
        Err(_) => {
            let _ = thread.join();
            Err(AudioError::Build("音频线程提前退出".to_string()))
        },
    }
}

impl AudioOutput {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

/// 打开默认输出设备并立即开始静音输出。
fn start(
    playback: Arc<Playback>,
    master: Option<Box<dyn BlockProcessor>>,
    healthy: &Arc<AtomicBool>,
) -> Result<AudioOutput, AudioError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(AudioError::NoDevice)?;
    let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
    let supported = device
        .default_output_config()
        .map_err(|error| AudioError::Config(error.to_string()))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let config = cpal::StreamConfig {
        channels: supported.channels(),
        sample_rate: supported.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };

    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, &config, playback, master, channels, healthy)?,
        SampleFormat::I16 => build::<i16>(&device, &config, playback, master, channels, healthy)?,
        SampleFormat::U16 => build::<u16>(&device, &config, playback, master, channels, healthy)?,
        other => return Err(AudioError::UnsupportedFormat(other)),
    };

    stream
        .play()
        .map_err(|error| AudioError::Play(error.to_string()))?;

    Ok(AudioOutput {
        _stream: stream,
        sample_rate,
        device_name,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    playback: Arc<Playback>,
    mut master: Option<Box<dyn BlockProcessor>>,
    channels: usize,
    healthy: &Arc<AtomicBool>,
) -> Result<Stream, AudioError>
where
    T: SizedSample + FromSample<f32> + 'static,
{
    let stream_health = Arc::clone(healthy);
    // 预分配块处理缓冲，回调里只做算术。
    let mut scratch = vec![0.0_f32; 8192];
    let mut left = vec![0.0_f32; 8192];
    let mut right = vec![0.0_f32; 8192];
    let mut output_left = vec![0.0_f32; 8192];
    let mut output_right = vec![0.0_f32; 8192];
    if let Some(processor) = master.as_mut() {
        let _ = processor.prepare(config.sample_rate.0, scratch.len());
    }
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let frames = data.len() / channels;
                let mut offset = 0;
                while offset < frames {
                    let chunk = (frames - offset).min(scratch.len());
                    playback.render(&mut scratch[..chunk]);
                    if let Some(processor) = master.as_mut() {
                        left[..chunk].copy_from_slice(&scratch[..chunk]);
                        right[..chunk].copy_from_slice(&scratch[..chunk]);
                        output_left[..chunk].fill(0.0);
                        output_right[..chunk].fill(0.0);
                        if processor
                            .process(
                                &left[..chunk],
                                &right[..chunk],
                                &mut output_left[..chunk],
                                &mut output_right[..chunk],
                            )
                            .is_ok()
                        {
                            for index in 0..chunk {
                                scratch[index] = (output_left[index] + output_right[index]) * 0.5;
                            }
                        }
                    }
                    for (index, sample) in scratch[..chunk].iter().enumerate() {
                        let value = T::from_sample(*sample);
                        let base = (offset + index) * channels;
                        for channel in 0..channels {
                            data[base + channel] = value;
                        }
                    }
                    offset += chunk;
                }
            },
            move |error| {
                stream_health.store(false, Ordering::Release);
                eprintln!("音频流错误：{error}");
            },
            None,
        )
        .map_err(|error| AudioError::Build(error.to_string()))?;
    Ok(stream)
}
