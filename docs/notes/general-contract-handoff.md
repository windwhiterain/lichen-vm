# 通用契约 API 交接（动态秩探针已通过）

## 目标

- 持久目标：完成 general contract API 实现（具名张量 / 形状表内核）。
- 子目标状态：动态秩收缩探针**已通过**，输出
  `[2938, 7042, 3016, 7264, 3094, 7486, 3172, 7708, 3250, 7930]`（与独立参考实现一致）。
- 探针：`crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs`（print-only）。

## 用户裁定（必须遵守）

1. 不做物化（no materialization）。
2. 动态与静态共用一个接口，一视同仁。
3. freeze 之前先 normalize；`@loop` 标记要落在 static 函数上。
4. 拒绝跨模块 `FunctionId`；`StaticModule.dynamic_functions` 这类桥要移除。
5. 写入必须在语言层（checker）完成；compute 只读。
6. 写新测试需用户许可；改 Invariant 需用户许可。
7. 每小步提交（即使红），不 `git stash`。

## 仓库状态

- 工作树：`C:\resource\lichen-vm`；分支 `dev`。
- `git status`：`README.md`（既有改动，勿提交）。
- 相关提交（新→旧）：
  - `c813a2f1` loop: 移除冻结函数桥
  - `96b58a15` compute: 循环入口值与外部节点所在块
  - `3691e5c2` docs: general contract API handoff
  - `fd763a15` lowlevel: 值记录一次读的目标与位置
  - `6771f993` checker: formal 与 tuple 元素记录元素类型
- 套件（`c813a2f1` 实测）：lowlevel `167 passed`；highlevel `136 passed`；compute `247 passed, 1 ignored`。
- 门：`cargo fmt --all`、`comments-check --config comments-test.toml . --max-violations 0`。
- 调试打印 `[val]`、`[dbg]` 已从 compute 移除。

## 探针修复的两条规则（`96b58a15`）

1. **入口值是入口值，不是当前块的参数。** 外层函数里作为调用实参元素出现的节点，
   读到的是循环**进入时**的值；此前 `Lower::loop_value` 对这类节点返回
   `block_slots[slot]`（当前块的参数），于是循环体内闭包变量 `rank` 被当成状态读，
   `dim := rank - s(0)` 恒为 0。现在先按状态路径（`parameter_value_path` /
   `entry_state_path`）解析真状态读，解析不到才按实参元素身份返回
   `ActiveLoop::entry_values`。类根相等不做身份判据（同类的 Int 会互相误配）。
2. **不属于该 nest 的节点在需求它的块里发射。** 此前发射到 nest 的 entry 块；当该定义
   读取 nest 块参数时（嵌套循环的实参就可能如此），entry 不支配使用点，得到
   `block 0 does not dominate block 6`。现在 `value_inner` 只为 nest 拥有的节点切换
   游标，其余就地发射。

副作用：`elementwise_chain` 的合并片段 digest 随发射布局更新为
`857e7461e5468c1e`（该测试打印的 canonical form 未变，哈希仍自洽）。

## 相关代码

- `crates/lichen-lowlevel/src/loop_conversion.rs`
  - `Module::parameter_value_path`：先 `marked_state_path`，再值侧 `Index` 链；
    读者分支用 `value_exprs` + `element_types` 回退。
  - `Module::state_nodes`、`Module::entry_state_path`、`Module::read_position`。
- `crates/lichen-lowlevel/src/lib.rs` 与 `module.rs`
  - `element_types: HashMap<NodeId, AnyNodeId>`（值 → 类型，语义窄，勿当通用登记表）。
  - `value_exprs: HashMap<NodeId, Vec<NodeId>>`（值 → 陈述它的表达式）。
  - `value_reads: HashMap<NodeId, (NodeId, usize)>`（值 → 读目标与常量位置），暂时未接入。
- `crates/lichen-highlevel/src/checker/indexing.rs`
  - `check_index`：只有 `constant_position(index)` 有常量时才 `record_value_read`；
    探针的下标全是动态表达式，所以该表在探针里为空。
- `crates/lichen-compute/src/compute/body.rs`
  - `Lower` 持有 `&Module`（只读）；`loop_owner`、`loop_value`、`value_inner`、
    `loop_state`、`lower_loop_*`、`enter_loop_block`、`ActiveLoop`。
  - `LoopConversion` 的状态路径、tests/steps/exits 由 lowlevel 提供。

## 实测事实（重要）

1. 探针通过；旧交接文档里的期望值 `[2938, 3874, …]` 是错的（来自更早版本的程序），
   已按独立参考实现更正。
2. `element_types` 不能当“任意值 → 类型”登记表：全量登记破 17 个 compute 测试，
   只登记 tuple 元素破 33 个（merge/tiling 依赖元素槽位语义）。
3. `value_reads` 对动态下标无用（无常量位置），不要围绕它设计。
4. compute 侧不能写模块（`&Module`）；任何登记必须发生在语言层（checker）。
5. 嵌套循环的实参可以在外层循环的步块里；外部节点就地发射后支配关系成立。
6. 冻结桥已在三个套件里测量为死代码：`static_dynamic_function` 从未被询问，
   `set_static_origin` 从未被调用。程序自己的 lambda 在检查期打标，程序模块不会被冻结；
   检查前发生的冻结（prelude 9 个函数、compute 37 个函数）都不含 `@loop`。
   因此「freeze 早于 mark」在探针路径上不成立，真正缺的是冻结转换的下沉。
7. 冻结函数带 open captures 时，从冻结图单独下沉需要调用方的活值（capture 的对应关系），
   只有 capture-free 的冻结循环能纯用冻结图下沉。这是统一读法必须先回答的边界。

## 下一步（按用户裁定排序）

1. **冻结转换的下沉（统一读法）**：`lower_loop`/`ActiveLoop` 改持 `AnyFunctionId`，
   状态节点经 `state_nodes_any` 从冻结图取；步骤/出口里的冻结计算需要一个冻结节点的发射器。
   现有 `LoopGraph` 已实现两图查询（operation/items/value/class/parameter_value_path），
   先把 `Lower` 的循环机械改成走它，再决定 capture 的边界（见实测事实 7）。
   需要「kernel body 来自冻结包」的探针来测量——**写探针前先取得用户许可**。
2. 3 维实例（同样需要许可）与最终库形状（具名张量、形状表、契约函数）。
3. 若继续用 `value_reads`，只在语言层记录真实存在的常量位置；动态位置需要另一条判据。

## 已知坑

- `check_app` 对 `range_children(argument)` 直接 `state[element]` 会越界 panic，勿重试。
- 计算 `Lower` 侧 `record_element_type` 会触发 `&Module` 借用错误，勿改 `Lower` 持有方式。
- `elementwise_chain` 的 digest 是发射布局的指纹：改 lowering 后它会变，更新 pin 即可，
  但先确认 canonical form 的其他断言仍通过。
- 提交只 add 自己改的路径；不要提交 `README.md`。

## 交接提示

- 压缩上下文后从「下一步」第 1 条继续。
- 需要用户裁决时用 `ask_user_question`，不要用正文提问。
