# ADR 0006：插件边界

- 状态：接受
- 日期：2026-10-23

## 背景

Aurtrack 最终要同时支持两类扩展：

1. 实时音频插件，例如 CLAP 合成器和效果器。
2. Blender 式应用扩展，例如自定义命令、面板、导入导出器和节点类型。

这两类东西的失败代价不同。音频插件跑在音频线程附近，崩溃会直接中断声音；应用扩展可能来自不可信来源，崩溃不应该拖垮整个 DAW。它们不能共用一套 ABI，也不应该让 `engine` 认识 CLAP。

## 决定

### 两类扩展

```text
audio.processor   native 音频插件；第一版只做 CLAP
app.extension     应用扩展；第一版只做外部进程命令提供者
```

`audio.processor` 由 `aurtrack-host` 负责加载和适配。  
`app.extension` 由独立的扩展管理器负责发现、权限和进程生命周期。

### 核心不认识插件格式

`aurtrack-engine` 只定义块处理器契约，不认识 CLAP、VST3 或任何厂商 API。  
`aurtrack-host` 把 CLAP 适配成核心契约。以后增加 VST3/LV2 时，只增加适配器。

### 处理器契约

实时处理器必须满足：

- `prepare(sample_rate, max_block_frames)`
- `reset()`
- `process(input, output)`，固定块大小上限
- 明确的 latency 与 tail
- 参数更新走无锁队列
- 音频线程禁止分配、加锁、日志和系统调用

第一版只支持单声道或立体声效果器，不支持插件自己的 MIDI 输入和自定义 UI。

### 状态

工程只保存：

```text
plugin_id
plugin_version
state_blob
```

不把厂商参数名硬编码进工程格式。参数界面由宿主根据插件描述生成。

### 隔离

- native CLAP 第一版进程内加载，接受崩溃风险。
- 应用扩展不能直接加载动态库，优先 WASM 或独立进程。
- 扩展清单必须声明能力和 API 版本。

### 发现与加载

第一版只扫描显式插件目录：

```text
<config>/Aurtrack/plugins
```

不做插件商店、自动下载和后台扫描。扫描结果缓存，但插件文件变化时重新读取。

## 不做

- 不把 CLAP 类型泄漏进 `aurtrack-session`、`aurtrack-graph` 或 `aurtrack-engine`。
- 不让扩展直接改工程内存。
- 不让应用扩展跑在音频线程。
- 不在第一版支持插件自定义 GUI 和 MPE。

## 迁移顺序

1. `aurtrack-host`：CLAP 扫描与描述符。
2. 引擎块处理器契约。
3. CLAP 效果器最小音频闭环。
4. 扩展清单、发现、命令调用。
5. UI：插件浏览器和参数面板。

## 后果

好处是核心保持干净，插件格式可以替换，扩展系统能独立演进。  
代价是第一版需要额外的适配层，CLAP 崩溃隔离和实时参数队列要后面补。
