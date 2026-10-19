# Aurtrack

一个免费开源的数字音频工作站，围绕可编程的文档模型、基于 CLAP 的设备、以及从预设到深度声音设计的节点图构建。

**状态：** pre-alpha。包括工程文件格式在内，一切都不稳定。

- 许可证：[GPL-3.0-or-later](LICENSE)
- 参与开发：[CONTRIBUTING.md](CONTRIBUTING.md)
- AI 协作规则：[AGENTS.md](AGENTS.md)
- 术语表：[docs/glossary.md](docs/glossary.md)

## 仓库结构

```text
crates/     Rust workspace 成员（核心、引擎、图、IO、宿主、CLI）
shells/     桌面与移动外壳
ui/         TypeScript 前端
specs/      文档 schema、会话协议、图 IR 语义
docs/       架构决策记录与设计笔记
```

目前还没有实现。第一个里程碑是「无头核心 + CLI」：能打开工程、建轨、加设备、渲染出 WAV。
