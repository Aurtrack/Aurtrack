# ADR 0004：会话文件放在 known folder，不依赖 TEMP

- **状态：** 已接受
- **日期：** 2026-10-20
- **决策者：** 项目维护者

## 背景

Git Bash 等 POSIX 风格 shell 会把 `TEMP` / `TMP` 设成 `/tmp` 或 `/c/Users/...`。Windows 上的 Rust `std::env::temp_dir()` 会把这些值解析成 `C:\tmp\...`、`C:\c\...`，与桌面进程写入的真实临时目录不一致。

结果是：桌面能启动，CLI / MCP 也能读到端口文件——如果 shell 把 `/tmp` 映射到真实目录；但原生的 `aurtrack.exe` 按环境变量找文件时去了错误目录，于是误判“没有运行中的会话”，退回本地运行时。

我们实测了 `GetTempPathW` 和 `GetTempPath2W`：在 `TEMP=/tmp` 下，两者都返回 `D:\tmp\`，同样受环境变量影响，不能修复这个问题。

## 决定

- 会话文件改放在系统 known folder：

```text
{cache_dir}/Aurtrack/aurtrack-session.json
```

- Windows 上由 `dirs::cache_dir()` 走 known folder，不读 `TEMP`。
- 如果 known folder 不可用，回退到 `LOCALAPPDATA`，最后才回退到 `temp_dir()`。
- 服务启动时创建父目录；句柄丢弃时删除会话文件。
- 支持 `AURTRACK_SESSION_FILE` 显式覆盖路径，建议传绝对路径。
- CLI 连接失败时打印会话文件路径并警告，然后退回本地一次性运行时。
- MCP 连接失败时直接退出，不再静默退回本地运行时；必须显式加 `--local`。

## 后果

- 在 Git Bash / WSL / POSIX 风格 `TEMP` 下，CLI 和 MCP 仍能找到桌面会话。
- 会话文件不再是“系统临时目录里的普通文件”，而是 Aurtrack 自己的缓存目录；崩溃后可能留下过期文件，客户端连接失败会报出路径。
- 多用户机器上每个用户有独立会话文件，不会互相覆盖。
- `AURTRACK_SESSION_FILE` 给了受限沙箱和自定义启动方式的逃生口。
