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

输入和输出数量必须一致，类型必须一致。当前支持 `Float`、`Int`、`Gate`、`Audio`、`Note`。

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

Repeat Input 额外提供 `iteration` 输出，类型 `Int`，从 0 开始；循环体里的
`voice.zone.input` 会把它绑定到 `VoiceOp::Iteration`。默认钢琴不再使用独立的
`p-index` 节点。

`Note` 端口在 Voice 域里是当前音符上下文，不是可比较的数值状态；它按
上下文直通处理，不进入数值携带槽。需要逐迭代不同音符事件时应使用音频或
显式数值状态，而不是依赖 Note 携带。

音频线程只使用栈数组和固定缓存，不分配、不加锁。

### UI

- Zone 侧栏按“携带值”成对编辑，增删同时作用于输入和输出。
- 每个携带值有名称和类型，类型同步到两端。
- 拖动排序同时移动配对两侧。
- 上下边界节点自动同步端口。
- 外部值可以直接连进 Zone 内部；音高/力度等不参与迭代的值不再占用携带端口。
- 组节点的未连接数值端口在节点内显示参数输入框。
- Repeat Input 底部显示 `Iteration` 输出端口。

### 旧工程迁移

旧工程的 `piano.voice` 可能包含：

```text
pitch/velocity 作为 Zone 携带对
p-pitch / p-velocity 上下文节点
p-index 独立迭代节点
p-open-* 常量节点
```

`project.open` 会调用幂等迁移：

- 把携带对改回外部自由变量，并补 `note-separate → p-partial` 直连
- 删除 `p-pitch` / `p-velocity`
- 删除 `p-open-*`，值写入 `p-partial.params`
- 删除 `p-index`，改接 `p-in.iteration → p-partial.index`

也可以显式执行 `project.migrate`。迁移不改 `format_version`，重复执行无副作用。

### 迭代预览

`voice.preview` 的诊断结果包含 `repeat_traces`：

```text
zone         Zone 头节点 ID
iterations   每一轮结束时的数值携带槽
```

UI 在 Zone 侧栏点击“采样迭代”即可看到当前播放头下 `#0`、`#1` … 的实际携带值。
追踪只在离线诊断路径启用；音频路径传入 `None`，不分配，不记录。

## 非目标

- 不做 Geometry/Collection 端口；Aurtrack 目前是 Voice/Audio 域。
- 不做字段广播或 Blender 的 Sockets/Fields 区分。
- 不把 Zone 编译器改成解释器；每个音频采样仍然走编译后的计划。

## 后续

- 预览按当前播放头采样一次，还不会在播放时持续刷新。
