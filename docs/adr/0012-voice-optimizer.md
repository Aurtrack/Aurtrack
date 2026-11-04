# 0012 VoicePlan 通用优化器

## 状态

已接受。

## 背景

`voice.piano_partial` 的 lowering 解决了物理钢琴和弦性能，但它是一条针对内置 `piano.partial` 组的专用规则：

- 其他组不会受益；
- 嵌套组、重复计算和死节点仍然进入音频线程；
- 编译器没有通用的图优化阶段。

## 决定

在 `aurtrack-engine` 内加入 `VoicePlan::optimize()`，在 `aurtrack-session` 的 `compile_voice` 末尾执行。

### pass 顺序

```text
递归优化 Repeat / GroupCall 的 body
→ 常量折叠 + 代数化简
→ 纯节点 CSE
→ 死代码消除
→ 重复两轮
```

### 常量折叠

支持：

```text
Add Multiply Subtract Divide Power Modulo
Floor Ceil Round Exp Sqrt IntToFloat
GreaterThan LessThan MapRange Clamp Sin Abs
```

### 代数化简

```text
x + 0       → x
x * 1       → x
x * 0       → 0
x - 0       → x
x / 1       → x
x ^ 1       → x
```

### CSE

只合并无状态纯节点，键是操作符加输入索引。根节点不参与替换。

### 死代码消除

从 `output`、`outputs`、`node_outputs` 反向遍历。`RepeatResult` 和 `GroupResult` 的 `source` 字段也视为依赖，避免删除 Repeat/GroupCall。

## 与专用 lowering 的关系

优化器是通用 pass 框架，但不会自动从任意组图合成 `VoiceOp::PianoPartial`。内置 partial 组的 lowering 保留为一条特化规则，因为它是语义融合，不是普通代数化简能推导出来的。

也就是说：

- DCE、折叠、CSE、嵌套优化对所有 VoicePlan 生效；
- 物理钢琴的实时性能仍依赖内置组 lowering；
- 用户修改 `piano.partial` 后 lowering 跳过，优化器仍会跑，但不会恢复同样的融合效果。

## 验证

- `voice_optimizer_folds_constants_and_dce`
- `voice_optimizer_eliminates_duplicate_subexpressions`
- `repeat_zone_tests` 全部通过，确认 Repeat/GroupResult 依赖没有被 DCE 误删。
- `physical_piano_track_renders` 仍检查 lowering 后的 `PianoPartial`。

## 后续

要彻底摆脱专用 lowering，需要：

1. Repeat 索引常量传播；
2. 通用表达式融合；
3. 组调用的语义特化注册表；
4. 音频线程上的融合表达式求值器。

这些还没有实现。

## 实验记录

曾实现一版通用 GroupCall 表达式融合：把无状态组体编译成表达式树。结果 8 音物理钢琴的 2 秒渲染从约 0.85 秒退化到约 6.2 秒，原因是递归表达式求值没有共享节点，且每个 partial 都要重复求值。该 pass 已停用。

随后尝试后序指令流 VM，去掉递归和 Box 追逐。第一版栈深度错误导致静音/崩溃；修正 `Clamp`/`Sine` 的栈深度后仍只输出极低电平，未能证明语义正确，因此整段 VM 实验被回退。表达式结构只保留为后续代码生成的基础，不参与当前编译。
