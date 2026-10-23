# CLAP 宿主 v0.1

## 范围

第一版只做：

- 扫描 `<配置目录>/Aurtrack/plugins` 下的 `.clap` 文件
- 读取 CLAP 描述符
- 在离线缓冲上运行立体声效果器
- 读取参数表和状态扩展能力
- 主输出实时插入：专用插件线程、无锁立体声块交换、固定 512 帧、一块延迟
- state blob 以 base64 读写，工程持久化留给下一阶段
- 暂不做参数写回、插件节点和崩溃隔离

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

实时主输出走 `PluginBridgeControl`：插件实例只在专用线程里创建和销毁，音频回调只持有 `PluginAudioPort` 和两个 `rtrb` 环形缓冲。当前固定 512 帧，非整块尾巴直通，避免帧数错位。
