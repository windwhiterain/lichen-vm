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
   - **tensor 驱动的收缩已量通**（`eb495a6f`）：程序声明 `Tensor`/`Pair`，kernel 参数放两个
     tensor，body 用 `p.in.<t>.buffer` 读、用 `origin`/`stride` 算地址、`@loop` 累加收缩维，
     结果 `[22, 28, 49, 64]`（A 2×3 × B 3×2）。加上 `read-live` 的转置读数
     `[1,3,5,2,4,6]`，named-tensor 形态端到端可用。
   - **kernel 参数用程序声明的类型就能读嵌套 tensor**（本轮量通，`6bc4ce40`）：把
     `Pair`/`Tensor`/`ReadIn` 声明在程序里（live），kernel body 内联地址算术和
     `compute.read`，同一套 2×3 列主序/行主序张量读出
     `[1,2,3,4,5,6]` 与 `[1,3,5,2,4,6]`。规则：**kernel 参数类型必须是程序声明的**（role walk
     在 live 模块里读字段名）；冻结类型留给宿主侧，kernel 边界用程序自己的类型或扁平 buffer。
   - **冻结字段读的 value 槽是有意未定的，kernel 走查看不到它**（本轮实测）：`t.Tensor`
     的读在检查期返回 value=3679（未定）、type=3681（kind expr）——延迟是为了让调用点解析
     字段类型（冻结注解修复依赖它）。把该读物化成 decided 会让 `frozen-type` 读数回退
     `((@any,@any), none, …)`，所以不能改读本身。可行的方向是**检查器记录“这个未定读代表
     哪个冻结节点”**（`Module::frozen_reads`），kernel 的只读走查跟随它；本轮已实现并实测
     卡在：kernel body 的 lowering 走的是**克隆节点**（走查看到 3977/3946 等克隆，映射键
     是原节点 3679），所以映射必须随 clone 复制，或键放在 clone 共享的节点上。
     **落地一**（`8b702de7`，无物化）：检查器把“未定读 → 冻结节点”的**引用**记进
     `Module::frozen_reads`（常量位置直接取冻结字段值；名字位置经字段声明查），只读走查
     查表跟随；`t.Tensor` 声明的读记录为 `Static(325)`。
     **落地二**（`d5afed9c`）：检查器为每个函数保留**参数注解的值**（
     `Module::parameter_annotations`），`Ctx::resolve_kernel_reads` 钩子由
     `compute.parallel` 调用，并用 `NativeArg.term` 解决“函数实参的值格要到定义后才填”
     的问题。
     **本轮实测的卡点**：钩子能拿到 body 的函数与注解，但 **body 的字段读不是 deferred
     读**（容器类型是 struct kind，concrete=true），所以“延迟读解析”的钩子无事可做。
     **本轮又测到两条**（155）：(1) **body 的读是晚检查的**——`compute.parallel` 的
     build 时钩子跑在 body 读被检查之前；把解析放到整个 check 之后（定义 pass 之后）才能
     看到这些读；(2) **读链的根容器匹配不上任何函数的参数值**：记录形参的 value cell 后
     再解析，所有函数仍是 `matching=0`——读链的根是**模板节点**（如 `compute` 库模板里的
     396/504，或 body 的 391/661/684），不是用户在 check 期建的函数参数 cell。下一步：
     建立**模板 ↔ 实例**的参数映射（或从模板的形参 value cell 出发），再沿读链解析。
     **委派**：两个只读 subagent 都在产出前失败（诊断只有 `[object Object]`），已删除；
     改为内联调查。
     **内联调查的结论**（155 末）：程序被**多次 check**（`Mode::Check`、`Mode::Evaluate`，
     还有一次在 `statements-eval` 之后）；body 的读 `n/in/t/buffer` 是在**较后的那一次**
     的语句循环里被检查的，而各检查点的“晚解析”看到的读表里没有它们（每次 27 条、全是
     库/前一轮的读）。结论：需要按 check pass 正确记录/解析。
     **原则更新（用户裁定：不能延迟）**：不放宽为“延迟到调用点解析”，而是**在检查期**用
     引用解析。落地（`e728065d`）：每个函数记录形参的 value cell；`check_named_field` 在
     读出 `value_node` 后，沿“参数注解的类型字面量”解析该字段的**声明类型**，冻结字段只记
     引用（`frozen_reads`），不拷贝；kernel 走查按节点或 class root 跟随引用。
     **已打通**（`12ec5adb`）：原因找到了——注解的要求会**实例化一份新的类型字面量副本**
     （fresh cells），所以实例里的字段读（358）不是模板里记录过的读（51）。修法：在
     `instantiate_requirement` 里把模板的**冻结引用复制到 fresh cell**（纯引用），并让
     `walk_role` 也沿冻结引用下潜。实测：最小程序（`p.in.t.buffer` + 冻结 `t.Tensor`）编译
     并运行出 `[1,2,3,4]`；张量收缩的两个变体（程序声明类型 / 冻结 `t.Tensor`）都答
     `[22,28,49,64]`。全程无物化、无延迟、static/dynamic 同一条引用路径。
   - **冻结产物保存了全部值；`t.Tensor` 运行时读出的就是类型值**（本轮实测，`426e42ea`）：
     直接检查 artifact：export 的 11 个绑定值都在（4 个类型是 type value，5 个函数是
     Function），`t.Tensor` 单独运行得到 `((?a, Float, none), (@any,@any), (@any,@any),
     @any): Struct`。所以**冻结不是问题**；kernel 走查看到的是“类型层字段的值 = struct
     kind（节点 7 = TypeStruct）”，而不是类型值。修复方向：让检查器给类型层字段存**类型值**
     （而不是 kind），或让走查沿该字段的 type 槽找类型值。
2. **plan 由形状表驱动**（已按用户裁定 A「值携带」落地，`2f68adc4`）：plan 现在是**宿主数据**
   ——`Shape{.n, .s0 … .s11}` 的第一个整数是 lane 数，其余是十二个 plan 项；plan kernel 只把它
   物化进 plan buffer，不再硬编码 stride/extent。rank 2 答 `[2938, 7042, …]`、rank 3 答
   `[330, 1554, …]`。**实测规则**：kernel 的发射 extent 是参数**第一个整数域**（把 rank 放第一
   时只跑了 rank 个 lane）。**已落地**（`0d97c524`）：宿主用形状表 `[Dim; 4]`（每维 a_stride/b_stride/extent）
   经 `shape_value` 编成 `Shape` 值，程序只做物化；两 rank 读数不变。剩余：把这套
   helper 提升为**产品 API**（走 serve 的 RunArguments 路径），以及 `examples/` 里的真实示例
   （需用户许可，会进 README 渲染）。
2. **契约函数收尾**：已落地**程序侧**的 `contract := 'arg -> ((compute.plrun lib.kernel arg) : Struct)`
   （`d8ac82ac`）：rank 2 答 `[2938, 7042, …]`、rank 3 答 `[330, 1554, …]`。**库内**写同样的
   函数被拒绝（`the container's type is not decided here`），所以工作拼法是“库持有 kernel、
   宿主持有调用”。剩余：plan 仍由 `fill_plan` 硬编码；形状表 → plan 的路线（值携带 vs 类型级
   表）待裁定。
3. **落成真实示例**：`examples/` 会进 README 渲染（`lichen-tools` 的 readme 渲染器），
   加之前先取得许可。
4. 冻结函数作为 kernel body（v1 门）不再是库形状的必需项；若仍要做，需要
   `lower_loop`/`ActiveLoop` 持 `AnyFunctionId` 并从冻结图取状态节点（capture 边界见实测事实 7）。
5. 若继续用 `value_reads`，只在语言层记录真实存在的常量位置；动态位置需要另一条判据。

## 具名爱因斯坦形态（用户裁定 A：具名表）

拼写与实测（`zz_dynamic_contract_probe.rs`）：

- 形状/步长是**具名表** `table{string, Int}`，名字集合是 `set{string}`；
- `x <== "i"` 按名取值、`"i" @in names` 判成员、`if n @in ns then s <== n else 0` 做缺名保护；
- **参数类型注解要用冒号**：`('s : ShapeTable)`、`('ns : NameSet)`、`('n : string)`；`=` 是等价
  细化，不是类型注解（`('s = ShapeTable)` 报 `expected ?, found Type`）；计算出来的类型
  （`Buf` 应用、数组）不能作注解（`expected Struct/Array, found raw[…Struct/Array]`），
  所以 buffer 参数不注解、由调用点统一。
- **已验证**：具名表 + 查找 + 集合成员（`(2, 3, 1)`）、plan kernel 在普通函数里由具名表推导
  plan（`[12, 0, 2, 0, 1, 5, 4, 20, 3, 1, 5, 4]`）、`plrun` 在普通函数里跑 kernel 并读结果
  （`[1, 2, 3, 4]`）。
- **卡点一**（`@in` 守卫）：`if n @in ns then s <== n else 0` 当 `n` 是**参数**时，`@in` 的条件在
  检查期未定，`if` 的结果在调用点也**没有**被解析（读数是 `none`）；字面量键则正常。
- **变体（已改成）**：两张形状表都列出**全部维**，不在该操作数上的维其 stride 写 0——于是
  推导不需要成员守卫，只做表查找。**卡点二**：这样组合后 `contract` 调用处仍报
  `this field read cannot be checked: the container's type is not decided here`（读数 98,6，
  调用行）；`plrun` 单独在函数里跑、读结果域是通的，所以剩余是一个**在调用点未解析的域读**。
  已定位（`refuse_undecided_container` 加临时打印）：被拒的是 **`(plrun p_kernel ps).plan`**
  ——容器是 **Apply**（`compute.plrun` 调用），在函数内构造 `In{… .plan dp.plan}` 的**深求值**时
  其结果类型未定，于是域读被拒（非 formal-rooted，不能延迟）。去掉 `: Struct` 注解会引入
  `expected Table, found Struct`，所以注解是必需的。下一步：让 `plrun` 的结果类型在构造
  聚合值时已是 kernel 的 codomain（或把该域读按 formal-rooted 延迟）。
  **尝试 1 的实测**（`NativeOp::result_type` 默认 `None`，`ParLaunchOp` 答第三实参的值；
  边界用 fresh cell 与之 unify）：第三实参**其实总是 `: Struct` 注解本身**（`args=3`、
  declared = `Struct` 标记，`DBG_RT` 实测），不是 kernel 的 `.O` 类型；把结果类型 unify 成
  `Struct` 标记会让 `compute.parallel` 的 lower 报 **definition cycle**。所以路线 1 需要的是
  kernel 的 `.O` 类型值（`build` 在冻结模板上拿不到，见 `ParLaunchOp::build` 的 Invariant），
  不是注解值。已回退，树绿。**建议改走路线 2**（把该域读按 formal-rooted 延迟到调用点）。

## 函数内两次 plrun 的实测（本轮）

- **内联到顶层的命名契约给出参考值**（`abb4cfbb`，`probe_a_named_inline`）：
  `[2938.0, 7042.0, 3016.0, 7264.0, 3094.0, 7486.0, 3172.0, 7708.0, 3250.0, 7930.0]`，
  全程 SPEC 合规的 `: Struct` 注解。所以布局不是瓶颈。
- 单独实测都通过（新探针）：
  `probe_a_plan_in_literal`（函数内 plan 内核 + 域读进聚合值再返回）→ plan 12 项；
  `probe_two_runs`（一个函数里两次 plrun）→ plan 12 项；
  `probe_a_buffer_parameter`（未注解的 buffer 参数进结构体域再读回）→ `[1,2,3,4]`；
  `probe_a_parameter_key`（参数为键的表查找）→ `(2, 3)`。
- **函数包装的命名契约仍不行**：表推导 plan 时答案 `raw none`；把 plan 改成函数参数
  （`probe_a_contract_kernel_in_function`）时在 `compute.parallel` 的编译处报
  `integer divide by zero`（span 72,11）——kernel 编译把 `p.in.plan` 的读按**参数模板**
  求值（模板 buffer 的读为 0），于是解码循环的 `% extent` 除以零。
  这是 kernel 编译/lowering 的缺陷（参数来源的 plan 读不能按模板求值），不是语言层。

### 参数 buffer 的实测（本轮）

- `probe_a_parameter_buffers`：kernel 形状与契约 kernel 同构（`{.meta 4 个 Int, .in Buffers{2 个 buffer}}`），
  两个 buffer 都从**函数参数**进来，kernel 读它们 → `[3, 4, 5, 6]` ✓。参数 buffer 能进 kernel。
- 契约 kernel 在函数里（`probe_a_contract_kernel_in_function`）仍在 `compute.parallel` 处
  `integer divide by zero`；加 `DBG_IN` 实测**三个输入都解析成 array**（position 0/1/2，path [1,0]/[1,1]/[1,2]），
  所以缺陷在输入解析**之后**：kernel 执行时 plan 的 extent 读到 0（或 `@loop` 展开把模板读折叠成 0）。
  把 `ps`/`dp` 挪到 `kernel :=` 之前无效（trap 仍在）。
  下一步：在 kernel 编译的 `@loop` 展开/常量折叠处打点，确认 `p.in.plan` 的读是否被折叠成模板的 0。

### 链式 plrun 的根因（本轮，只读调查）

- 新复现：`probe_a_parameter_buffers`（函数里两次 plrun，第二次的参数域读第一次的结果）
  答 `raw none`。它在 `ParLaunch` 的 named-form 输入解析处静默返回 None。
- 实测根因：输入解析拿到的是**声明的 `Buf` 包装模板**（5 项：`[Table, TypeId(100), Array, Array, Array]`），
  它第 2 项 `.native` 是 `@any` 洞；读的 base 就是这份声明，所以无论怎样“索取”读节点，
  只能求值出声明，拿不到第一次 plrun 的运行期结果。参数被绑到声明（`plrun := 'k = K -> 'a = (k.I _) -> …`，
  `compute.lichen:19`），调用点的实参值没有替换掉参数类里的声明。
- 试过的修法（已回退）：在解析处用 `class_operation_owner` 命名持有操作的类成员并 `evaluate_node_deep_ref` 求值。
  读节点本身被求值成声明，payload 仍是空 cell，无效；套件仍 267 绿。
- 待办：把“实参的答案替换参数声明”的落点定下来（`write_node_answer` 规则，`equality.rs:427`），
  即检查器的参数注解路径、apply clone、还是 plrun 的 native build；约束是不物化、不加机制、static/dynamic 一致。

### 输入解析读的是“类的值”，不是被求值的读（本轮实测）

- 在 `apply_parameter_check` 里对开放参数槽调用既有 `write_node_answer`（answer 规则）**确实写入**
  （`agrees=true, open=true`），参数槽拿到调用点答案；但链式 plrun 仍 `raw none`。
- 决定性对照：函数内 `compute.collect d1.a` → `[1, 2, 3, 4]` ✓（同一个读，由 VM 求值操作数后取到运行期 buffer）。
- 链式路径的输入解析：`value_at_path` 的 `slot_value_of` 取的是**类里的已定值**（声明的静态 `ABuf/Buf` 包装，
  `items[0/1]` 是 Static 的 Table/TypeId），所以 `buf_payload` 拿到的是声明的 `.native`，`run_input` 拒绝
  （不是 `ComputeValue::Buffer`）。collect 路径能工作，是因为 VM 先**求值**了读节点。
- 下一步（不物化、不加机制）：让输入解析保留**路径上的字段节点**（读），先由 VM 求值该节点，
  再取它的值做 `buf_payload`——与 collect 路径同一语义；`value_at_path` 现在直接取类值，跳过了这一步。
- 按用户更新的 SPEC（`docs/language-spec.md`：Unify 段 + Any 段 “Only merge if another side is right
  side and undecided”）核对：实现的 wildcard 分支 `is_wildcard(va) && vb.is_none()` 与 SPEC 一致；
  把“右侧是 Any 也合并”的改动是错误的，已回退。
- 实测（本轮）：把输入解析改成“取字段的值节点并 `evaluate_node`”后：
  - `has_no_result_yet` 为 **false**（该节点的 run stamp 已置位）——它的 `Index` operation 已经跑过，
    答案是**类型值**；`.native` 的 payload node 无值。
  - 读的 base 链是 `Index(d1 值, 位置)`，`d1` 的值就是第一次 plrun 的结果；把 `compute.lichen` 的
    `r = k.O` 去掉也**不改变**这个读数，所以类型值的合并不在 plrun lambda 里，而在别处（候选：
    结构体实例化时字段与类型字段值的 unify，即 SPEC §Struct 的 `v[k] = e1` 那一步）。
  - 结论：与 Any 规则无关；障碍是“第一次 plrun 的结果值已经是类型值”，读只是在读这个值。
- 本轮再往下一层（`DBG_CHAIN`）：读的 base 链是
  `9538 (Index, 有值=类型值) ← 9536 (Index, 无值) ← 9168 (无值、**无 operation** 的裸 cell)`。
  也就是说，字面量里这个读的容器值是一个**空 cell**（没有 operation、没有值）；同一读在函数内直接
  `collect d1.a` 是好的（`[1,2,3,4]`）。所以差别在“读被放进结构体字段”这条构造路径：它的容器值
  没有与第一次 plrun 的 apply 节点并入同类，运行时结果也就填不进这个空 cell。
- 下一步：比较 `check_struct_value` 构造字段值时用的容器值与直接 `d1.a` 用的容器值（两者是否同一个
  node / 是否同类），按 SPEC 的 `e3 = v[k]`（e3 就是你写的表达式）定位构造路径的偏差。

## 已知坑

- `check_app` 对 `range_children(argument)` 直接 `state[element]` 会越界 panic，勿重试。
- 计算 `Lower` 侧 `record_element_type` 会触发 `&Module` 借用错误，勿改 `Lower` 持有方式。
- `elementwise_chain` 的 digest 是发射布局的指纹：改 lowering 后它会变，更新 pin 即可，
  但先确认 canonical form 的其他断言仍通过。
- 提交只 add 自己改的路径；不要提交 `README.md`。

## 交接提示

- 压缩上下文后从「下一步」第 1 条继续。
- 需要用户裁决时用 `ask_user_question`，不要用正文提问。
