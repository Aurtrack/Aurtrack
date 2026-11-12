# 0023 GraphState 堆分配与控制线程栈预算

## 状态

已接受。

## 背景

桌面版点击播放后进程以 `0xC00000FD` 退出。Windows 事件日志显示
`aurtrack-desktop.exe` 栈溢出，复现路径：

```text
CMD transport.play
PLAY ensure_audio end
PLAY build_plan begin
thread 'main' has overflowed its stack
```

崩溃发生在主线程构建播放计划，不在音频线程，也不是 block 内核。

根因是 `GraphState` 内联了 256 个节点的多帧状态：

```rust
values: [[f64; 64]; MAX_GRAPH_NODES]
resonators: [[f64; 4]; MAX_GRAPH_NODES]
filters: [[f64; 2]; MAX_GRAPH_NODES]
highpass: [f64; MAX_GRAPH_NODES]
```

仅 `values` 就约 131KB。`GraphPlan` 在 `build_plan`、`compile_audio_graph`
和返回路径中按值传递，debug 构建的临时副本叠加后击穿 Windows 主线程约
1MB 的栈。

## 决定

`GraphStateCell` 改为堆持有：

```rust
pub struct GraphStateCell(Box<UnsafeCell<GraphState>>);
```

- 控制线程构建计划时只分配一次；
- 音频线程通过 `Box` 解引用访问，仍然无分配、无锁；
- `GraphPlan` 的栈上大小降到 KB 级；
- 增加回归测试 `graph_plan_keeps_large_state_on_heap`，要求
  `size_of::<GraphPlan>() < 4096`。

## 后果

- debug 与 release 桌面版点击播放不再栈溢出；
- `cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`
  通过；
- 8 音 Playback 约 277ms，12 音约 408ms，block 与回退计数不变。

## 已知限制

- 该修复针对“状态内联导致计划过大”，不限制未来新增的大块栈上临时对象；
- 新增大数组字段时应优先放 `Box`、`Vec` 或计划持有的预分配缓冲；
- Windows 主线程栈大小由宿主决定，不能依赖默认 8MB。
