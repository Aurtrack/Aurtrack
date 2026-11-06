# 0014 GraphState 所有权

## 状态

已接受，待实现。

## 背景

`GraphPlan::process_block_voices` 目前每块在栈上创建 `GraphState`，无法保存跨 block 的节点值，也不是真正的计划持有状态。

`Playback::render(&self)` 只有共享引用，而 block 状态需要可变写入。

## 选项

1. `UnsafeCell<GraphState>` + 单音频线程安全证明；
2. `AtomicU64` 状态数组；
3. 把 `render` 改成 `&mut self`，改所有调用方。

## 决定

采用 **`UnsafeCell` + 单音频线程约束**：

- 状态随计划创建；
- 音频回调是唯一写入者；
- 控制线程只替换整个计划，不并发写状态；
- 用 `SAFETY` 注释明确不变量。

不采用原子状态，因为每 block 的节点值读写会显著增加成本。

不采用 `&mut self`，因为 `Playback` 通过 `Arc` 共享给音频和 UI 线程。

## 后续

1. 把 `GraphState` 放入 `GraphPlan`；
2. `process_block_voices` 借用计划持有的状态；
3. 迁移 `Resonator/Filter/Delay` 内部状态；
4. 差分测试和性能回归。
