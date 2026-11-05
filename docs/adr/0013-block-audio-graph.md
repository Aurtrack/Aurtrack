# 0013 Block 音频图执行

## 状态

已接受，迁移中。

## 背景

当前 `VoicePlan` 是逐采样标量解释器：

```text
sample(context) -> f64
```

每个节点每次采样写入一个 scratch 原子槽。这个模型保证了正确性，但导致：

- 每个 partial 一次函数调用；
- 无法做 block 级 CSE、LICM 和 SIMD；
- 性能压力最终变成 `PianoPartial` 语义特化；
- Cranelift 只能接在标量表达式上，无法生成原生循环。

Pd、SuperCollider、VCV Rack 都以 block 为调度单位；Faust、Max gen~、Cmajor 则在 block 内生成原生循环。

## 决定

音频图迁移到 block 执行：

```rust
node.process(state, inputs, outputs, frames)
```

默认 block：

```text
DEFAULT_BLOCK_SIZE = 64
```

节点状态显式保存在节点自己的状态区，不在音频线程分配。

## 迁移顺序

1. `VoicePlan::sample_block` 兼容层（已完成）；
2. `VoiceNodePlan` 节点 kernel；
3. block 调度器，按拓扑顺序执行；
4. Repeat / partial bank 原生循环；
5. 删除结构化 partial lowering；
6. 性能验收。

## 门槛

```text
8 音 2 秒 ≤ 1.0 秒
12 音 2 秒 ≤ 1.5 秒
```

## 当前状态

只完成第 1 步。节点 kernel、调度器和原生循环尚未实现，生产仍走逐采样标量计划和 partial 特化。
