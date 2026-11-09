# 0015 声部 Repeat 的 block 内核

## 状态

提议中。

## 背景

`PianoPlan::sample_block` 已经能把无状态声部按 64 帧处理，并与逐采样等价。

但钢琴节点图顶层是 `Repeat`：

```text
Repeat
  count       来自输入
  carried     跨迭代状态
  outputs     映射到 body 端口
  body        通常是 GroupCall
```

`VoiceSsa` 只处理线性节点，不能直接编译 `Repeat`，所以 `PianoPartial` 特化仍在。

## 不采用展开

展开为：

```text
count × body 节点数
```

钢琴默认是 10 × 73 ≈ 730 个节点，超过声部节点预算，也失去运行时 count。

## 决定

新增 `Repeat` 的 block 内核，不展开节点：

```text
for iteration in 0..count
    body.process_block(
        context.iteration = iteration,
        body_inputs,
        body_output,
    )
    carried = body_output[..carried]
    output  = body_output[outputs]
```

需要先补：

```text
VoicePlan::eval_input_block(index, context, frames)
```

用于计算 count 和 carried 的 block 值。

## 约束

- count 必须是整块常量；逐帧变化时回退逐采样。
- carried 暂只支持 `carried == 0`；非零先回退。
- body 的 `outputs[0]` 必须等于 body.output；不支持时回退。
- 所有回退保留原逐采样语义。

## 后续

1. `eval_input_block` 只支持无状态子图；
2. 单层 Repeat 差分测试；
3. carried 支持；
4. 嵌套 Repeat；
5. 删除 `PianoPartial`。
