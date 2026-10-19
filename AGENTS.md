# AGENTS.md

给 AI 编码代理的规则。动手前先读这一份。

## 项目是什么

Aurtrack 是一个免费开源的数字音频工作站，围绕三个想法构建：

1. 单一可寻址文档模型：每个实体都有稳定 ID 和路径。
2. 所有编辑走命令总线：撤销、脚本、测试、AI 共用一条写入通道。
3. 设备即节点组：新手看到自动生成的面板，高手打开节点图。

核心是 Rust，桌面外壳是 Tauri + TypeScript，许可证 GPL-3.0-or-later。

## 架构红线（不可协商）

- 依赖方向：`doc`、`engine`、`graph` 不得依赖 `io`、`host`、`script` 或任何 UI crate。
- 音频线程禁止分配、加锁、日志、阻塞和系统调用。
- 所有状态变更走命令总线，UI 和脚本不得直接改文档。
- 引擎里不允许出现 UI 概念：没有像素、颜色、布局、面板类型。
- UI 是薄客户端：只发命令、渲染快照，不持有引擎指针。
- 公开类型保持稳定：公开枚举和可选字段结构体加 `#[non_exhaustive]`。

如果需求与以上冲突，停下来问，不要绕过。

## AI 友好红线

- 每个能力都必须能通过命令注册表表达；不允许存在「只有界面能做」的操作。
- 每个命令必须有稳定 ID、参数 JSON Schema、撤销语义和一句中文说明。
- 每个实体必须可寻址：稳定 ID + 路径。
- 引擎必须能无头运行；CLI 是第一个客户端，不是附属品。
- 面向 agent 的返回值要省 token：摘要、字段选择、分页、diff，不倾倒全量数据。
- MCP 只是适配器，不允许在 MCP 层写业务逻辑或绕过命令注册表。

## 语言策略

Aurtrack 是中文优先项目：散文用中文，标识符用英文。

- **注释与文档：简体中文。** `//`、`///`、`//!` 一律中文。
- **标识符、路径、JSON 字段、CLI 参数、协议键：英文。** 这些是机器接口，永远不翻译。
- **提交信息：中文。** `type(scope)` 保持英文。
- **用户可见字符串：i18n key**，源语言 `zh-CN`，不得硬编码。
- **许可证与法律文本：英文**，必须与官方文本一致。

术语以 `docs/glossary.md` 为准；新增术语先补词表，再写代码。

## 注释规则

- 只写“为什么”，不写“是什么”。禁止复述下一行代码。
- 允许的标签只有五个，标签本身保持英文：`SAFETY`、`PERF`、`NOTE`、`TODO(#123)`、`HACK`。
  它们是给 grep 和工具看的标记，不是散文。
- 禁止注释掉的代码、作者署名、日期、逐行解说。
- 中英文混排时英文两侧加空格；句子用全角标点；标识符用反引号。
- 长注释手动换行，按显示宽度算，不要超过约 80 列（中文一个字算两列）。
- 每个 crate 的 `lib.rs` 顶部必须有模块文档：

  ```rust
  //! 职责：
  //! - ...
  //!
  //! 禁止：
  //! - ...
  ```

## 代码规则

- 测试之外禁止 `unwrap()` / `expect()`。
- 库用 `thiserror`，二进制可以用 `anyhow`。
- 时间统一用 `Frame(i64)`，秒只出现在 IO 和 UI 边界。
- 音频缓冲用 `f32`，参数值用 `f64`。
- 单位和 ID 用 newtype（`SampleRate`、`Gain`、`TrackId`）。
- `unsafe` 必须配 `// SAFETY:` 说明不变量。
- 错误信息用中文：简短、不带句号、说清下一步该做什么。

## 提交规则

Conventional Commits：`type(scope): 中文标题`

- type：`feat fix perf refactor docs test build ci chore revert`
- scope：`doc cmd engine graph dsp io host session proto script ui cli sdk ci deps repo release`
- 标题用中文、祈使语气、结尾不加句号（中英文句号都不要），整行不超过 60 字符。
- 正文用中文说明“为什么”，每行不超过 100 字符。
- 一个提交只做一件事；每个提交都必须能编译并通过测试。
- 破坏性变更加 `BREAKING CHANGE:` 页脚。
- PR 标题在 CI 中按同一份配置校验；squash 合并后它就是 `main` 上的提交信息。

示例：

```text
feat(graph): 为 Voice 域端口加入推断

节点现在可以声明 Voice 输入。Block 率的值流入 Voice 端口时，
编译器自动插入广播；反向则必须经过显式的 reduce 节点。

Refs #42
```

## 说“完成”之前

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```

如果某项检查跑不了（缺工具、还没有 crate），明确说出来，不要假装通过。
