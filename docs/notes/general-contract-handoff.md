# 通用契约 API 交接（动态秩探针）

## 目标

- 持久目标：完成 general contract API 实现（具名张量 / 形状表内核）。
- 当前子目标：动态秩收缩探针通过，输出 `[2938, 3874, 3016, 3952, 3094, 4030, 3172, 4108, 3250, 4186]`。
- 探针：`crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs`（print-only，调查期间留在树里）。

## 用户裁定（必须遵守）

1. 不做物化（no materialization）。
2. 动态与静态共用一个接口，一视同仁。
3. freeze 之前先 normalize；`@loop` 标记要落在 static 函数上。
4. 拒绝跨模块 `FunctionId`；`StaticModule.dynamic_functions` 这类桥要移除。
5. 写入必须在语言层（checker）完成；compute 只读。
6. 写新测试需用户许可；改 Invariant 需用户许可。
7. 每小步提交（即使红），不 `git stash`。

## 仓库状态

- 工作树：`C:\resource\lichen-vm`；分支 `dev`；HEAD `fd763a15`。
- `git status`：`README.md`（既有改动，勿提交）、`docs/notes/general-contract-handoff.md`（本文件，未提交）。
- 最近提交（新→旧）：
  - `fd763a15` lowlevel: 值记录一次读的目标与位置
  - `6771f993` checker: formal 与 tuple 元素记录元素类型
  - `75453357` compute: tuple state 元素按读者表达式投影
  - `c8e1f354` lowlevel: 值记录产生它的表达式
- 套件（HEAD 实测）：compute `247 passed, 1 ignored`；highlevel `136 passed`。
- 探针（HEAD）：拒绝，信息为 `Bin(Int, Sub) reads ValueId(10), which block 2 receives as a parameter, and block 2 does not dominate block 8`。
- 门：`cargo fmt --all`、`comments-check --config comments-test.toml . --max-violations 0`。

## 相关代码

- `crates/lichen-lowlevel/src/loop_conversion.rs`
  - `LoopConversion { function: AnyFunctionId, state, tests, steps, exits }`；`loop_conversion_any`、`LoopGraph`。
  - `Module::parameter_value_path`（约 709 行）：先 `marked_state_path`；再 `value_read` 猜测；再值侧链；链上含读者匹配分支（`value_exprs` 找 `Index` 读者）。
  - 类匹配且路径为空时现在会落入读者分支（本提交改动）。
- `crates/lichen-lowlevel/src/lib.rs`
  - `Module.element_types: HashMap<NodeId, AnyNodeId>`（值节点 → 类型节点）。
  - `Module.value_exprs: HashMap<NodeId, Vec<NodeId>>`（值 → 表达式）。
  - `Module.value_reads: HashMap<NodeId, (NodeId, usize)>`（值 → 读目标与位置），本提交新增。
- `crates/lichen-lowlevel/src/resolve.rs`
  - `record_element_type(value, ty)`（`entry().or_insert`，首条生效）。
  - `element_type_of(node)`：按节点、`class_root`、`node_origin` 依次回退。
  - `element_value_node(node)`：节点值若是布局数组则取其首项，否则取节点自身。
- `crates/lichen-highlevel/src/checker.rs`
  - `value_of`（约 1772 行）：记录 `set_expr_value(term, value)`。
  - `check_term`（约 1806 行起）：尾部有 `anchor_marks(e)` 与 skeleton 绑定。
- `crates/lichen-highlevel/src/checker/indexing.rs`
  - `check_index`（约 31 行）：`array_value = value_of(array)`、`index_value = value_of(index)`、`position = constant_position(index) + 1`；本提交在末尾写 `record_value_read(value_node, array_value, position - 1)`。
- `crates/lichen-highlevel/src/checker/lambda.rs`
  - `check_lam` 为每个 formal 记录元素类型（已提交）。
- `crates/lichen-compute/src/compute/body.rs`
  - `Lower` 持有 `&Module`，只能读；核心：`loop_owner`、`loop_value`、`value_inner`、`loop_state`、`lower_loop_steps`、`element_reader`。
  - 调试打印仍在：`[val]`（355 行）、`[dbg]`（422 行）；交付前移除。

## 实测事实（重要）

1. `element_types` 能登记循环入口值：在 `check_tuple_term` 用 `element_value_node(term)` 记录后，`element_types(15738) = Dynamic(17)`；探针从 `ValueId(10)` 进步为 `ValueId(0)`（block 0 参数，仍拒绝）。
2. 但该登记只是「tuple 元素」这一处，就让 compute 套件出现 33 个 FAILED（`fused_region`、`chain_merge`、`cooperative_product`…）。
3. 更早的「`value_of` 里对所有表达式值登记」让 compute 出现 17 个 FAILED。
4. 结论：`element_types` 的语义比「任意值 → 类型」更窄（元素槽位的形状权威），不能作为循环状态元素的通用登记表。
5. `value_reads` 已提交但未接入 `parameter_value_path` 的决策；`value_read(15734)` 为 None，`check_index` 记录到的是 `15738`（元素值），与 body 使用的 `15734` 不是同一节点，需对齐。
6. 语言层 `value_of` 没覆盖循环入口值：`s` 的 `state[e].val` 由 `check_index` 直接写。

## 下一步（推荐顺序）

1. 对齐节点：查清 `15734` 与 `15738` 的关系（`node_origin`、`class_root`、`element_index`、`element_expr`），让 `check_index` 记录的读值与 body 实际使用的值一致。
2. 用**专用**映射（`value_reads` 或新表）驱动 `parameter_value_path`，不要写 `element_types`；判定条件用「读目标 == 参数值格」或「读目标类 == 参数值类」，避免全域匹配导致套件回归。
3. 接入后跑探针；若仍拒绝，按 `[dbg]` 看 `loop_value` 取到的 slot 值是否来自正确块（当前 `ValueId(0)` 是 block 0 参数，说明进入块或 block_slots 仍未切换）。
4. 套件 + 门 + 提交；交付前移除 `[val]`/`[dbg]`。

## 已知坑

- `check_app` 对 `range_children(argument)` 直接 `state[element]` 会越界 panic；该写法已回退，勿重试。
- compute 侧 `record_element_type` 因 `&Module` 借用不可用；勿改 `Lower` 持有方式。
- 早前「全量登记」会静默改变 `element_type_of` 结果，破坏 merge/tiling 测试；任何登记改动先跑 compute 套件。

## 交接提示

- 压缩上下文后从「下一步」第 1 条继续。
- 需要用户裁决时用 `ask_user_question`，不要用正文提问。
- 提交只 add 自己改的路径；不要提交 `README.md`。
