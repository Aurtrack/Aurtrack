# Aurtrack

一个免费开源的数字音频工作站，围绕可编程的文档模型、基于 CLAP 的设备、以及从预设到深度声音设计的节点图构建。

**状态：** pre-alpha。第一条竖切已经能跑：命令 → 会话 → WAV → CLI / MCP / 桌面窗口。

- 许可证：[GPL-3.0-or-later](LICENSE)
- 参与开发：[CONTRIBUTING.md](CONTRIBUTING.md)
- AI 协作规则：[AGENTS.md](AGENTS.md)
- 术语表：[docs/glossary.md](docs/glossary.md)
- 命令规范：[specs/commands.md](specs/commands.md)
- 节点图规范：[specs/graph.md](specs/graph.md)
- 插件边界：[docs/adr/0006-plugin-boundary.md](docs/adr/0006-plugin-boundary.md)
- CLAP 宿主：[specs/plugins.md](specs/plugins.md)
- 应用扩展：[specs/extensions.md](specs/extensions.md)

## 现在能跑什么

需要 Rust 1.88 或更新；首次编译会下载依赖。

- **桌面窗口**：双击 `gui.bat`，或：

  ```sh
  cargo run -p aurtrack-desktop
  ```

- **无头演示**：双击 `demo.bat`，生成 `demo.wav` 并交给系统播放器。

- **命令行**：

  ```sh
  cargo run -p aurtrack-cli -- describe
  cat examples/demo.jsonl | cargo run -p aurtrack-cli -- --stdin
  ```

  CLI 与 MCP 默认连接运行中的桌面会话；CLI 连不上会警告并使用本地一次性运行时，MCP 连不上会直接退出，需显式加 `--local`。会话文件路径可用 `AURTRACK_SESSION_FILE` 覆盖。

- **MCP**（给 AI 客户端）：

  ```sh
  cargo run -p aurtrack-mcp
  ```

- **冒烟测试**：

  ```sh
  bash scripts/smoke.sh
  ```

当前能力：50 条命令（含实时走带、BPM/拍号/循环区间、节拍器、ADSR 包络、低通滤波、DAG 节点连线与坐标、节点复制、CLAP 插件扫描、主输出插入与状态 blob、应用扩展调用、工程保存/打开、片段/音符编辑、批量选择、跨轨移动、复制、粘贴与量化）、撤销/重做、16 位 WAV 渲染、实时播放、分支轨道节点图（`format_version: 4`）、CLI、MCP、Svelte 桌面窗口（轨道视图、小节网格、片段/钢琴卷帘编辑、端口连线节点工作区、中英文界面）。

界面默认中文，右上角可切换 EN / 中文；构建期可用 `VITE_AURTRACK_LOCALE=en` 覆盖，便于截图与语言回归测试。

## 仓库结构

```text
crates/     Rust workspace 成员（命令、会话、图、引擎、音频、运行时、CLI、MCP）
shells/     桌面与移动外壳
ui/         桌面前端（Svelte 5 + Vite）
specs/      命令规范、协议与图 IR 语义
scripts/    冒烟测试与工具脚本
docs/       架构决策记录与设计笔记
```

## 下一步

CLAP 插件节点、参数写回、扩展面板注册、自动化。
