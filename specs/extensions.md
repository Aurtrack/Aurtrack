# 应用扩展规范 v0.1

应用扩展是可被 Aurtrack 调用的外部进程。第一版只支持命令提供者，不支持自定义面板和节点。

## 目录

```text
<配置目录>/Aurtrack/extensions/<extension-id>/
  manifest.json
  hello.exe
```

配置目录优先使用 `AURTRACK_CONFIG_DIR`，否则使用平台配置目录。

## 清单

```json
{
  "id": "com.example.hello",
  "name": "Hello",
  "version": "0.1.0",
  "api_version": 1,
  "commands": [
    {
      "id": "hello",
      "name": "Say hello",
      "executable": "hello.exe"
    }
  ]
}
```

- `id` 只允许 ASCII 字母、数字、`-`、`_`、`.`，最长 64
- `api_version` 当前只能是 `1`
- `executable` 必须是扩展目录内的相对路径，不能包含 `..`
- 至少一条命令

## 调用协议

宿主把 JSON 写到进程 stdin：

```json
{
  "extension": "com.example.hello",
  "command": "hello",
  "args": { "name": "Aurtrack" }
}
```

进程 stdout 必须是一个 JSON 对象；stderr 用于错误说明。退出码非零视为失败。

宿主会设置：

```text
AURTRACK_EXTENSION_ID
AURTRACK_EXTENSION_API
```

## 安全

扩展进程拥有当前用户权限，不是沙盒。第一版只扫描显式扩展目录，不自动下载、不加载动态库。扩展不能直接改工程内存，只能通过返回值和后续命令影响工程。
