# CLAP 宿主 v0.1

## 范围

第一版只做：

- 扫描 `<配置目录>/Aurtrack/plugins` 下的 `.clap` 文件
- 读取 CLAP 描述符
- 在离线缓冲上运行立体声效果器
- 读取参数表和状态扩展能力
- 暂不做实时音频线程集成、参数写回、状态保存和崩溃隔离

## 边界

`aurtrack-host` 依赖 `clack-host`，不依赖 `aurtrack-engine`、`aurtrack-session` 或 UI。  
CLAP 类型不能出现在核心图、工程格式和引擎计划里。

## 扫描

`plugin.scan` 调用 `scan_directory`。坏插件记录在 `failures`，不会让整次扫描失败。

## 离线处理

`process_stereo_offline` 验证：

- 插件可以加载
- 有 PluginFactory
- 可以激活
- 固定块 512 帧
- 立体声输入和立体声输出
- 可以正常停用

它不是在实时音频线程里运行的宿主。实时集成需要单独设计插件线程、无锁缓冲和参数队列。
