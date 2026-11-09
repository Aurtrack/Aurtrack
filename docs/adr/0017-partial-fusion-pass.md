# 0017 Repeat/PianoPartial 融合

## 状态

提议中，待实现。

## 背景

VoicePlan block ABI v2 已经能执行真实钢琴：

```text
Output
→ Damper
→ Vca
→ Add
→ RepeatResult
→ Repeat
→ GroupCall
→ PianoPartial
```

功能通过，但性能：

```text
8 音 2.802 秒
12 音 3.507 秒
```

旧特化 kernel：

```text
8 音约 0.80 秒
```

## 差距

每次 Repeat 迭代都会递归执行：

```text
GroupCall
Add
Vca
Damper
PianoPartial
```

同一个常量/上下文节点被重复求值。

## 方案

新增融合 pass：

```text
RepeatResult
  source = Repeat
  body 链 = GroupCall → Add → PianoPartial
→ PartialLoop {
      count,
      pitch,
      velocity,
      inharmonicity,
      brightness,
      decay,
      position,
      offsets,
  }
```

块执行：

```text
for frame
    sum = 0
    for partial in 0..count
        sum += piano_partial_sample(...)
    output[frame] = sum
```

## 条件

只融合：

```text
count 是整块常量
PianoPartial 参数不依赖 Iteration 之外的块值
body 只包含线性 Add/Vca/GroupCall
```

否则保留 ABI v2 递归路径。

## 顺序

1. 提取 PartialLoop 描述；
2. 与 `piano_partial_sample` 对齐；
3. 真实钢琴差分；
4. 8/12 音性能；
5. 删除 `PianoPartial` 结构特化。
