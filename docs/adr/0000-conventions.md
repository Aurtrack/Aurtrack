# ADR 0000：代码、注释与贡献约定

- **状态：** 已接受
- **日期：** 2026-10-19
- **决策者：** 项目维护者

## 背景

Aurtrack 是由单人维护、AI 辅助、GPL-3.0-or-later 的项目。绝大部分代码会由 agent 辅助写成，所以约定必须能被工具强制，而不是靠记忆。有三个问题必须在第一个 crate 出现之前定下来，因为事后修改代价很大：

1. 注释和文档用哪种语言？
2. 对贡献者在法律上要求什么？
3. `main` 长什么样？

## 决定

### 1. 语言：中文优先，标识符用英文

- 注释、rustdoc、模块文档一律简体中文。
- 标识符、字段名、路径、CLI 参数、协议键一律英文。
- 提交信息用中文，`type(scope)` 保持英文。
- 用户可见字符串走 i18n key，源语言 `zh-CN`。
- `LICENSE` 与法律文本保持英文。

理由：维护者是中文母语者，中文表达更精确、更快；AI 对中英文的处理没有实质差异；插件作者普遍用 AI 辅助开发，读中文文档不构成障碍。术语一致性由 `docs/glossary.md` 保证。

### 2. 贡献：无 CLA，inbound = outbound

- 项目永久使用 GPL-3.0-or-later。
- 提交 PR 即视为按项目许可证授权，不做版权转让，不签协议，不设 CLA 机器人。
- DCO 签名可选，不强制。
- 没有重新许可的路径：双许可、闭源版、App Store 发行从一开始就不存在。

### 3. 历史：squash 合并，`main` 线性

- PR 默认 squash 合并。
- 精心整理过的大型系列可以 rebase 合并。
- `main` 必须线性，禁止 merge commit。
- PR 标题即提交信息，必须符合 Conventional Commits。

### 4. 强制手段

| 规则 | 由什么强制 |
| --- | --- |
| 排版 | `rustfmt.toml`，CI 跑 `cargo fmt --check` |
| lint、`unwrap`、`unsafe` 文档 | `Cargo.toml` 的 `[workspace.lints]`，CI 跑 clippy |
| 许可证与安全公告 | `deny.toml`，CI 跑 `cargo deny check` |
| 提交格式 | `commitlint.config.mjs`、`lefthook.yml`、`.github/workflows/commitlint.yml` |
| CHANGELOG | `cliff.toml` |
| 语言、术语、架构红线 | `AGENTS.md`、`docs/glossary.md`、代码评审 |

## 后果

**正面**

- 母语表达，注释质量和决策记录的可读性更高。
- 术语统一有单一来源，AI 生成的内容不容易漂移。
- 贡献者零法律摩擦，项目身份明确。
- 历史干净，agent 产生的小提交不会污染 `main`。

**负面**

- 国际贡献者的门槛变高，`docs.rs` 上显示的是中文。
- 将来若要国际化，需要额外维护一层英文文档。
- 中文是双宽字符，行宽的观感和英文不同，需要人工控制换行。
- 无 CLA 意味着永远无法更换许可证；这是单向门，在第一个外部 PR 合并后关闭。

## 备注

- 允许的注释标签（`SAFETY`、`PERF`、`NOTE`、`TODO(#123)`、`HACK`）保持英文：它们是给工具和 grep 用的标记，不是自然语言。
- Conventional Commits 的 `type` 与 `scope` 保持英文：解析器要求 ASCII 标记，翻译会直接破坏工具链。
- 如果将来需要英文文档，做法是新增 `docs/en/` 与英文 README，而不是逐条翻译源码注释。
