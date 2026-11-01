# ADR 0010：Blender 式 Repeat Zone

## 状态

已接受。

## 背景

旧 Repeat Zone 只把第一个音频输入当作跨迭代携带值，其余输入是常量；多输出会重复执行整个循环体。它不足以表达 Blender Geometry Nodes 的 Repeat：

- 多组输入/输出成对携带
- 每组在每次迭代后把输出送回对应输入
- 迭代索引
- 任意嵌套
- 类型明确的端口

## 决策

### 端口成对

`voice.zone.repeat` 的 `zone_inputs` 与 `zone_outputs` 按位置配对：

```text
input[i] 初始值
  → 第 0 次迭代
  → output[i]
  → input[i] 下一次迭代
  → ...
最终 output[i] 作为区域结果
```

输入和输出数量必须一致，类型必须一致。当前支持 `Float`、`Int`、`Gate`、`Audio`；`Note` 暂不支持。

Zone 端口只表示**跨迭代携带值**。外部节点可以直接连到循环体内部节点，作为只读自由变量；它不进入携带链，每次迭代都读取同一个父图值。编译器把它放在携带槽位之后的 free input 槽。

### 单一循环子计划

编译器为一个 Zone 生成：

```text
VoiceOp::Repeat { body, outputs }
VoiceOp::RepeatResult { source, slot } × N
```

`Repeat` 只执行一次循环体，把最终结果写入计划级缓存；`RepeatResult` 只读取对应槽位。这样不会因为多输出重复执行状态节点。

### 执行语义

```text
count = 0                  → 输出初始值
count > 0                  → 迭代 count 次
iteration 从 0 开始
每次迭代读取上一次的携带值
每轮结束把 body 输出写回携带值
自由变量每轮保持父图当前值
```

音频线程只使用栈数组和固定缓存，不分配、不加锁。

### UI

- Zone 侧栏按“携带值”成对编辑，增删同时作用于输入和输出。
- 每个携带值有名称和类型，类型同步到两端。
- 拖动排序同时移动配对两侧。
- 上下边界节点自动同步端口。
- 外部值可以直接连进 Zone 内部；音高/力度等不参与迭代的值不再占用携带端口。
- 组节点的未连接数值端口在节点内显示参数输入框。

## 非目标

- 不做 Geometry/Collection 端口；Aurtrack 目前是 Voice/Audio 域。
- 不做字段广播或 Blender 的 Sockets/Fields 区分。
- 不把 Zone 编译器改成解释器；每个音频采样仍然走编译后的计划。

## 后续

- 支持 `Note` 端口。
- 缓存按计划实际 Repeat 输出数动态分配；单个 Zone 上限 64。
- UI 显示每个携带值在上一次迭代的当前值。
