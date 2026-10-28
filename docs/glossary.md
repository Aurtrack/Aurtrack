# 术语表

本表是 Aurtrack 文档、注释、提交信息里译法的唯一来源。**新增术语先加到这里，再写进代码。**

## 使用规则

1. 标识符、字段名、协议键、CLI 参数一律英文，不翻译。
2. API 名称（例如 `clap_host_audio_ports`）永远保持原文。
3. 中英文混排时，英文两侧加空格：`CLAP 插件`、`f32 缓冲`、`Voice 域`。
4. 中文句子用全角标点；代码片段和标识符用反引号。
5. 同一个概念只用一个译法，不要在同义词之间摇摆。
6. 拿不准就先查这张表，表里没有就补一行，再继续写。

## 架构与工程

| 英文 | 中文 | 备注 |
| --- | --- | --- |
| document model | 文档模型 | |
| datablock | 数据块 | 借用 Blender 的概念 |
| stable ID | 稳定 ID | 不译作“唯一标识符” |
| command bus | 命令总线 | |
| command | 命令 | |
| transaction | 事务 | |
| undo / redo | 撤销 / 重做 | |
| session | 会话 | |
| protocol | 协议 | |
| snapshot | 快照 | |
| event | 事件 | |
| view state | 视图状态 | 不进工程文件、不进撤销栈 |
| device | 设备 | |
| node | 节点 | |
| node group | 节点组 | |
| graph | 图 | |
| port | 端口 | |
| connection | 连接 | |
| socket | 端口 | 与 port 统一用「端口」 |
| socket value | 端口值 | 组实例上未连接输入的实例值 |
| socket default | 端口默认值 | 组接口上的模板默认值 |
| group input | 组输入 | Blender 的 Group Input 节点 |
| group output | 组输出 | Blender 的 Group Output 节点 |
| note event | 音符事件 | |
| note input | 音符输入 | `audio.note`，当前轨道的钢琴卷帘事件源 |
| note separate | 分离音符 | 把 Note 拆成 pitch、velocity、gate |
| gate | 门 | 控制事件或声音是否通过 |
| add | 加法 | 两个输入相加，没有系数 |
| mix | 混合 | Blender 式混合，`Factor` 在第一个输入 |
| factor | 系数 | 混合节点中 A 到 B 的插值位置 |
| time | 时间 | 当前音符内的局部时间 |
| seed | 种子 | 当前音符的确定性随机种子，由序号经白噪声转换 |
| note index | 音符序号 | `Separate Note` 的 index 输出，当前音符在轨道事件列表中的序号 |
| white noise | 白噪声 | 1D 确定性哈希，输入相同则输出相同 |
| envelope | 包络 | 只输出控制值，不直接处理音频 |
| VCA | 压控放大器 | 音频 × 控制值 |
| noise | 噪声 | 只输出噪声音频源 |
| patch | 音色 | 与 preset 区分 |
| preset | 预设 | |
| parameter | 参数 | |
| automation | 自动化 | |
| modulation | 调制 | |
| macro | 宏控件 | |
| signal domain | 信号域 | |
| voice | 声部 | 复音语境；人声语境用「人声」 |
| block | 块 | 音频处理的基本单位 |
| tick | tick | 音乐时间最小单位；不译作“滴答” |
| PPQ | PPQ | 每四分音符的 tick 数；当前为 960 |
| sample | 采样 | |
| sample rate | 采样率 | 不译作“采样频率” |
| buffer | 缓冲区 | |
| latency | 延迟 | |
| plugin delay compensation (PDC) | 插件延迟补偿 | |
| host | 宿主 | |
| render | 渲染 | |
| bounce | 导出 | |
| freeze | 冻结 | |
| offline | 离线 | |
| real-time | 实时 | |
| deterministic | 确定性 | |
| headless | 无头 | |

## 音频与音乐

| 英文 | 中文 | 备注 |
| --- | --- | --- |
| track | 轨道 | |
| clip | 片段 | |
| region | 区域 | |
| take | 录音条 | 同一次录音的多个版本 |
| bus | 总线 | |
| send | 发送 | |
| return | 返回 | 返回轨道 |
| master | 主输出 | |
| mixer | 混音器 | |
| channel strip | 通道条 | |
| stem | 分轨 | |
| sidechain | 侧链 | |
| synthesizer | 合成器 | |
| envelope | 包络 | |
| LFO | 低频振荡器 | 首次出现写「低频振荡器（LFO）」，之后用 LFO |
| filter | 滤波器 | |
| cutoff | 截止频率 | |
| resonance | 谐振 | |
| gain | 增益 | |
| pan | 声像 | |
| metronome | 节拍器 | |
| tempo | 速度 | |
| time signature | 拍号 | |
| quantization | 量化 | |
| swing | 摇摆 | |
| velocity | 力度 | |
| pitch | 音高 | |
| note | 音符 | 逐音符事件数据，不是单个浮点数 |
| polyphony | 复音 | |

## 不翻译的词

这些是格式名、缩写或业界惯例，保留英文原文：

`CLAP`、`VST3`、`LV2`、`AU`、`AUv3`、`MIDI`、`MPE`、`ABI`、`DSP`、`DAW`、`UI`、`CLI`、`AI`、`JSON`、`WAV`、`FLAC`、`MP3`、`OSC`、`SIMD`、`FFT`
