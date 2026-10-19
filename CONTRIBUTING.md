# 参与 Aurtrack 开发

感谢你愿意帮忙。Aurtrack 还在 pre-alpha，接口随时会动。

## 许可证与贡献

Aurtrack 使用 **GPL-3.0-or-later**。

提交 pull request 即表示你同意你的贡献以相同条款授权（inbound = outbound）。
**本项目不使用 CLA**，没有版权转让，也没有需要签署的文件。

如果你希望把授权关系写得更明确，可以在提交里加一行签名：

```text
Signed-off-by: 你的名字 <you@example.com>
```

`git commit -s` 会自动加。这是加分项，不是必须项。

> 因为没有 CLA，本项目无法在将来更换许可证，除非取得每一位贡献者的同意。
> 这是有意为之：Aurtrack 永远是 GPL-3.0-or-later，不会有闭源版，也不会有 App Store 版本。

## 开发环境

- Rust 1.88 或更新（edition 2024）
- Node.js 24 或更新，pnpm 11 或更新
- 建议安装：

  ```sh
  cargo install cargo-nextest cargo-deny git-cliff
  pnpm install
  pnpm exec lefthook install
  ```

`pnpm install` 生成的 `pnpm-lock.yaml` 必须一起提交，CI 使用 `--frozen-lockfile`。

Windows 上建议顺手设置一下，否则中文文件名和提交信息可能显示成乱码。同时装上提交模板，每次 `git commit` 都会看到格式提示：

```sh
git config --local core.quotepath false
git config --local i18n.commitEncoding utf-8
git config --local commit.template .gitmessage
```

## 提交前检查

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
```

CI 会在 Linux、macOS、Windows 上跑同一套。本地全绿不保证 CI 全绿，但本地红一定 CI 红。

## 代码风格

格式和 lint 由工具强制，不靠 review：

- `rustfmt.toml` 管排版
- `Cargo.toml` 的 `[workspace.lints]` 管 lint
- `clippy.toml` 管 MSRV 与 API 变更策略

工具管不了、需要人来管的：

- 时间用 `Frame(i64)`，秒只出现在边界。
- 音频线程里不分配、不加锁。
- `unsafe` 必须写 `// SAFETY:`。
- 注释用中文，讲“为什么”，不讲“是什么”。
- 标识符、字段名、路径用英文。
- 术语以 `docs/glossary.md` 为准。

## 提交信息

使用 Conventional Commits：

```text
type(scope): 中文标题

用中文说明为什么需要这个改动、它做了什么。每行不超过 100 字符。

Refs #123
```

- type：`feat fix perf refactor docs test build ci chore revert`
- scope：`doc cmd engine graph dsp io host session proto script ui cli sdk ci deps repo release`
- 标题用中文、祈使语气、结尾不加句号、整行不超过 60 字符。
- 一个提交只做一件事，每个提交都应能编译并通过测试。

`type(scope)` 是机器解析用的标记，保持英文；标题和正文用中文。

完整规则以 `commitlint.config.mjs` 为准。`commitlint` 会在本地 `commit-msg` 钩子中校验；CI 校验的是 **PR 标题**，因为 squash 合并后它才是 `main` 上的提交信息。

## Pull Request

- 分支短命、单一目标。
- `main` 保持线性，PR 默认 **squash 合并**。
- 因为是 squash，**PR 标题就是提交信息**，必须是合法的 Conventional Commit。
- 每一步都有意义的大重构可以申请 rebase 合并，先问一声。
- CI 会按同一份 `commitlint` 配置校验 PR 标题，标题不合格会直接失败。
- 每个 PR 需要 CI 全绿。项目还小的时候，自己 review 自己没问题。

## 语言

- 代码、测试、文档注释：中文。
- 标识符、字段名、路径、CLI 参数：英文。
- 提交信息与 CHANGELOG：中文。
- `LICENSE` 与法律文本：英文。
- Issue 和讨论：中文优先，英文也欢迎。
