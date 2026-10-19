# Aurtrack

一个免费开源的数字音频工作站，围绕可编程的文档模型、基于 CLAP 的设备、以及从预设到深度声音设计的节点图构建。

**状态：** pre-alpha。第一条竖切已经能跑：命令 → 会话 → WAV → CLI / MCP / 桌面窗口。

- 许可证：[GPL-3.0-or-later](LICENSE)
- 参与开发：[CONTRIBUTING.md](CONTRIBUTING.md)
- AI 协作规则：[AGENTS.md](AGENTS.md)
- 术语表：[docs/glossary.md](docs/glossary.md)
- 命令规范：[specs/commands.md](specs/commands.md)

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

- **MCP**（给 AI 客户端）：

  ```sh
  cargo run -p aurtrack-mcp
  ```

- **冒烟测试**：

  ```sh
  bash scripts/smoke.sh
  ```

当前能力：7 条命令、撤销、16 位 WAV 渲染、CLI、MCP、最小桌面窗口。

## 仓库结构

```text
crates/     Rust workspace 成员（命令、会话、CLI、MCP）
shells/     桌面与移动外壳
ui/         桌面前端（当前是静态页面）
specs/      命令规范、协议与图 IR 语义
scripts/    冒烟测试与工具脚本
docs/       架构决策记录与设计笔记
```

## 下一步

节点图、真实音频设备、CLAP 插件宿主与采样级自动化。
