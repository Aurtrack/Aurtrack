## 这个 PR 做了什么

<!-- 一句话说明。 -->

## 为什么

<!-- 关联 issue，例如 Refs #123。 -->

## 检查

- [ ] PR 标题符合 `type(scope): 中文标题`（squash 后它就是提交信息）
- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 通过
- [ ] 涉及音频线程的改动没有分配、加锁、日志或系统调用
- [ ] 新增的术语已补进 `docs/glossary.md`
