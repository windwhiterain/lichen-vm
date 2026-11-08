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
  - `f07f231b` test: 冻结注解绑定类型而非值（更锐的成因）
  - `932867c2` test: 冻结类型的字段读成 @any（成因测量）
  - `24fd5d42` tensor: tensor.lichen 初稿与两个阻塞
  - `da52e00e` test: typed-only selector 作答（旧笔记的拒绝已过时）
  - `63843e80` test: 包持有的 contract 在 rank 2 / rank 3 都通过
  - `f6b9d3dc` test: 契约库端到端（包持有 dynamic-rank kernel）
  - `a4caf9c3` test: 冻结包的 kernel body 测量
  - `c813a2f1` loop: 移除冻结函数桥
  - `96b58a15` compute: 循环入口值与外部节点所在块
  - `3691e5c2` docs: general contract API handoff
  - `fd763a15` lowlevel: 值记录一次读的目标与位置
  - `6771f993` checker: formal 与 tuple 元素记录元素类型
- 套件（`24fd5d42` 实测）：lowlevel `167 passed`；highlevel `136 passed`；compute `251 passed, 1 ignored`。
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
8. **库形状已端到端量通**（探针 `zz_frozen_body_probe`）：
   - 包内声明、编译并运行带 `@loop` 的 kernel 可行（`declared`，导出答案 `[3.0, 3.0]`）；
     循环在包构建期（活图）转换并冻结成**已编译 kernel 值**。
   - 导入方读得回该 kernel 值、其类型与函数（`kernel`、`kernel-type`、`kernel-function`）。
   - **程序用 import 的类型建参数并运行包导出的 kernel 可行**：`run` 答 `[6.0, 6.0]`
     （buffer 的 perspective 来自 `compute.write`）。
   - 把冻结**函数**交给 `compute.parallel` 仍按名字拒绝：
     `static (imported) functions are not kernel-compilable v1`（v1 门；库形状不需要它）。
   - 早先 `run` 的 Buffer-Buffer 失配是探针自己用了**普通 buffer**（本地 `ABuf` 类型、
     未经过 `write`/`read` 的 perspective），不是包边界缺陷；已复核并更正。
9. 包以裸表达式结尾时 export 是那个表达式（如 Array），import 期望 Struct 就会在 import 行
   `expected Struct, found Array`；包应以绑定结尾。
10. **3 维实例也通过**（`zz_dynamic_contract_probe` 的 rank 3 / contract 1）：
    `[330, 1554, 360, 1728, 390, 1902, 420, 2076, 450, 2250]`，与独立参考实现一致；
    2 维读数不变。
11. **契约库端到端通过**（`zz_frozen_body_probe` 的 `contract` / `contract3` 读数）：
    包 `contract_lib.lichen` 持有 dynamic-rank 收缩的全部类型、body（5 层嵌套 `@loop`、
    plan 驱动地址）和 kernel；包构建期转换循环并冻结已编译 kernel；程序用自己的 fill kernel
    填三个 buffer、经 import 的类型建 Meta/Arg，`plrun lib.kernel` 在 rank 2/contract 2 答
    `[2938, 7042, 3016, 7264, 3094, 7486, 3172, 7708, 3250, 7930]`，在 rank 3/contract 1 答
    `[330, 1554, 360, 1728, 390, 1902, 420, 2076, 450, 2250]`——都与参考一致。
    这就是 general contract API 的库形状：**库持有 kernel，宿主只给数据与 meta**。12. **typed-only selector 已可用**（`zz_tensor_probe` 的 `probe_a_typed_selector`）：
    `dimsOf := ('d = Dims) -> (d.shape.row, d.shape.column)` 作用在 two-pair 张量上答 `(2, 3)`。
    named-tensors §5.2 里「refused」的读数属旧版本；deferral 是 `01936889` 落的。
    形状表选择器因此可以「写一次、所有张量共用」。

13. **tensor 库包初稿已落**（`crates/lichen-compute/src/tensor.lichen`，用户选的语言侧 tensor 库）：
    `Pair`、`Tensor`（buffer + shape + stride + origin）、`dense2`/`view2`、`at2`、`read2`/`write2`。
    包能编译、函数可调用，但两个阻塞已测量（`zz_tensor_probe` 的 `probe_the_tensor_library`）：
    - **冻结类型作形参注解：已修复**（`3081828f`）。此前注解路径把形参 slot 按位置
      unify 到冻结的类型值上（`show_id` 返回类型形状、`x.shape` 答 `(@any,@any)`）。两条规则：
      冻结数组（含 static item）在 `requirement_is_open` 里算“开放”，且
      `instantiate_requirement_value` 让这样的数组留下 **fresh 槽**而不是冻结值本身。
      实测：`show_shape (2,3)`、`show_origin 0`、`show_row 2`、`show_id` 返回字面量的
      buffer/(2,3)/(3,1)/0，库函数 `t.at2` 对冻结 `Tensor` 字面量答 `5`。
      套件：compute 252、highlevel 136、lowlevel 167。
    - **库构造器：已改成单参数描述符并量通**（`567d875d`）。柯里化构造器的后续应用在检查期
      没有可解析的 callee（中间闭包的返回类型格未定），所以调用方对结果做字段读会被拒；
      改成 `dense2 ('d = Dense)` / `view2 ('v = View)` 后，冻结注解修复能把类型带过去。
      实测：`t.view2 t.View{…}` 造出的张量可读 `view.shape.row`，`t.at2 view t.Pair{…}` 答 `5`；
      直接字面量与 frozen-type 读数不变。
    - **kernel 里的 `read2` 仍停在嵌套 buffer 位置**（`(b)` 那个缺口）：`.in.t.buffer`
      被走查认作输入 `[1,0,0]`，但读没有匹配上。
9. 包以裸表达式结尾时 export 是那个表达式（如 Array），import 期望 Struct 就会在 import 行
   `expected Struct, found Array`；包应以绑定结尾。
10. **3 维实例也通过**（`zz_dynamic_contract_probe` 的 rank 3 / contract 1）：
    `[330, 1554, 360, 1728, 390, 1902, 420, 2076, 450, 2250]`，与独立参考实现一致；
    2 维读数不变。
11. **契约库端到端通过**（`zz_frozen_body_probe` 的 `contract` / `contract3` 读数）：
    包 `contract_lib.lichen` 持有 dynamic-rank 收缩的全部类型、body（5 层嵌套 `@loop`、
    plan 驱动地址）和 kernel；包构建期转换循环并冻结已编译 kernel；程序用自己的 fill kernel
    填三个 buffer、经 import 的类型建 Meta/Arg，`plrun lib.kernel` 在 rank 2/contract 2 答
    `[2938, 7042, 3016, 7264, 3094, 7486, 3172, 7708, 3250, 7930]`，在 rank 3/contract 1 答
    `[330, 1554, 360, 1728, 390, 1902, 420, 2076, 450, 2250]`——都与参考一致。
    这就是 general contract API 的库形状：**库持有 kernel，宿主只给数据与 meta**。12. **typed-only selector 已可用**（`zz_tensor_probe` 的 `probe_a_typed_selector`）：
    `dimsOf := ('d = Dims) -> (d.shape.row, d.shape.column)` 作用在 two-pair 张量上答 `(2, 3)`。
    named-tensors §5.2 里「refused」的读数属旧版本；deferral 是 `01936889` 落的。
    形状表选择器因此可以「写一次、所有张量共用」。

13. **tensor 库包初稿已落**（`crates/lichen-compute/src/tensor.lichen`，用户选的语言侧 tensor 库）：
    `Pair`、`Tensor`（buffer + shape + stride + origin）、`dense2`/`view2`、`at2`、`read2`/`write2`。
    包能编译、函数可调用，但两个阻塞已测量（`zz_tensor_probe` 的 `probe_the_tensor_library`）：
    - **冻结类型作形参注解：已修复**（`3081828f`）。此前注解路径把形参 slot 按位置
      unify 到冻结的类型值上（`show_id` 返回类型形状、`x.shape` 答 `(@any,@any)`）。两条规则：
      冻结数组（含 static item）在 `requirement_is_open` 里算“开放”，且
      `instantiate_requirement_value` 让这样的数组留下 **fresh 槽**而不是冻结值本身。
      实测：`show_shape (2,3)`、`show_origin 0`、`show_row 2`、`show_id` 返回字面量的
      buffer/(2,3)/(3,1)/0，库函数 `t.at2` 对冻结 `Tensor` 字面量答 `5`。
      套件：compute 252、highlevel 136、lowlevel 167。
    - **库构造器 `dense2`/`view2` 的返回类型仍未定**：给 body 加
      `(Tensor{…} : Tensor)` 会报 `expected Tensor…, found Struct`，所以调用方对
      `view2` 的结果做字段读仍会被拒；现在可以直接用 `t.Tensor{…}` 字面量绕开。
      测量（临时打印已移除）：`check_app` 对冻结 callee 读 `static_function_signature`
      的返回类型并物化成叶子；`at2`（返回 Int）的叶子有值，而 `view2`/`dense2` 的
      `return_type` 指向**共享 struct 模板节点**，冻结后该节点无值（`value=None`），
      叶子仍未定——所以调用方的字段读没有 decided 类型可用。
      **进一步测量**：`view2` 是柯里化链，签名读取只作用于**第一次应用**；后续应用的
      callee 是上一步的闭包结果，检查期解析不到（其返回类型格未定）。把 `view2` 改成
      **单参数结构**（`View` 描述符）也不行：已决定的结构字面量对冻结的结构类型注解
      会报 `expected Struct, found Struct`（值-值按位置 unify，是冻结注解修复未覆盖的
      另一半——actual 已 decided 时）。
    - **嵌套 buffer 的读没有匹配上**：tensor 放在 kernel 的 `.in.t` 时，通道走查把 buffer
      认作输入 `[1, 0, 0]`，但 `read2` 读 `p.in.t.buffer` 报
      `read's buffer argument is not an input buffer of the parallel parameter`。
    这两个缺口清掉，tensor 库才算可用。

## 下一步（按用户裁定排序）

1. **tensor 库剩余的两个口子**（按用户选的 B）：
   (a) ~~库构造器的返回类型~~：已用单参数描述符解决（见实测 13）。
   (b) kernel 参数里嵌套 buffer 的读匹配（本轮又测到三条事实）：
   - **kernel body 里不能调用库函数**：`compute.parallel` 的 lowering 只支持 cross-kernel
     callee（`kernel body Apply is supported only for a cross-kernel (kernel-value) callee v1`），
     所以 `at2`/`read2` 不能出现在 kernel body 里；body 必须内联地址算术与 read。
   - **内联后卡在冻结字段类型**：`p.in.t.origin` 报 `container's type is not decided`——
     `p.in.t` 的字段类型是冻结的 `Tensor`，而 kernel body 的 deferred read 没有调用点可解析
     （deferral 只在 dynamic callee 的 apply 处解析，`compute.parallel` 是 native 调用）。
   - **`read2` 路径只差 buffer 位置**：clone 里 `t.buffer` 的链 base class 与参数 value 的
     任何子项 class 都不相交，class 回退因此不中；`expr_value(slot.expr)` 给的是类型侧
     root。方向：让参数侧持有 kernel body 的**声明参数类型值**（而不是只有 template kind），
     或在 `plrun` 调用处解析 body 的 deferred reads。
   - **已落地的小步**（`addf6cc9`）：`type_is_concrete` 现在把「计算出来的类型」（读到的冻结
     类型）视为 concrete，于是内联 body 的字段读能过检查、进入 kernel 自己的走查；实测
     `read-inline` 现在停在 `a struct parameter field read names buffer … that struct's
     fields are None`（走查沿参数**类型值**走，`.t` 处的值是未定 cell，冻结 Tensor 的
     字段名读不到）。
2. **plan 由形状表驱动**：探针里的 plan 仍由 `fill_plan` 硬编码 stride/extent。把具名张量的
   stride 表（`@table_count`/`@table_key`，或库里的结构值）变成 plan buffer，才是
   「形状表 → 地址」的最后一环。
2. **契约函数收尾**：库导出 `contract := 'shape -> 'data -> ...`，内部生成 plan、跑 kernel、
   返回结果与 merge/without 出来的形状；程序只给形状与数据。
3. **落成真实示例**：`examples/` 会进 README 渲染（`lichen-tools` 的 readme 渲染器），
   加之前先取得许可。
4. 冻结函数作为 kernel body（v1 门）不再是库形状的必需项；若仍要做，需要
   `lower_loop`/`ActiveLoop` 持 `AnyFunctionId` 并从冻结图取状态节点（capture 边界见实测事实 7）。
5. 若继续用 `value_reads`，只在语言层记录真实存在的常量位置；动态位置需要另一条判据。

## 已知坑

- `check_app` 对 `range_children(argument)` 直接 `state[element]` 会越界 panic，勿重试。
- 计算 `Lower` 侧 `record_element_type` 会触发 `&Module` 借用错误，勿改 `Lower` 持有方式。
- `elementwise_chain` 的 digest 是发射布局的指纹：改 lowering 后它会变，更新 pin 即可，
  但先确认 canonical form 的其他断言仍通过。
- 提交只 add 自己改的路径；不要提交 `README.md`。

## 交接提示

- 压缩上下文后从「下一步」第 1 条继续。
- 需要用户裁决时用 `ask_user_question`，不要用正文提问。
