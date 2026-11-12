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
- 已查两端：
  - 字面量 `Buffers{.a d1.a}` 走 `check_instantiate`：对**未定**的字段值不做 value 级 unify，
    只记 `field_sources[required_value] = actual_value`（`structs.rs:980` 附近）。
  - 输入的解析 `value_at_path` 取到的就是**读的值节点**（带已定类型值）；实测在该节点及其类根上
    `field_sources` 查不到来源（`direct=None, via_class=None`）。
  - 所以要么 `field_sources` 的键不在解析到的节点上（注册的是模板字段值，解析走的是读的值节点），
    要么该链接只有“叶子为空/`@any` 才跟随”（`recorded_leaf`）而这里叶子有值。
- 下一轮：把 `check_instantiate` 注册的 `required_value` 键与解析到的节点逐一对照（同一 node？同类？），
  再把“模板字段的 required 值 → 实际值”的链接接到解析路径上（不动任何 Invariant，不加机制）。
- 已对照（本轮 `DBG_MATCH`）：
  - `field_sources` 里有 `required=9633 → supplied=9538`，且**两者同类**（root 9538）——链接是在的、
    两端同类；解析到的链头 9538 就是这一类。
  - 类的 value 是**required 侧的类型值**（已定那侧）；`supplied`（读）的运行期答案没进来。
  - 读的 base 是 `9168`：root `8856`、**无值、无 operation**、owner 是它自己——即“绑定到 apply 表达式的
    值 cell”与 apply 节点没有连上（apply 的 `state[e].val = None`）。
  - 所以卡点已成一条：**函数内 `d1 := (apply …)` 的绑定值 cell 与 apply 节点没有并类**；读走空 cell，
    类的 value 停在 required 的类型值。
- 下一步：按检查器的绑定路径找出“绑定值 cell ↔ apply 节点”应有的并类点（顶层能工作说明顶层有这条链接），
  在函数内补齐同一语义（不动 Invariant、不加机制）。
- 已读检查器（本轮）：
  - `check_app` 里 `let apply = op_node(Apply, operands); let node = apply;`，
    `state[e].term = node`（**term 就是 apply op 节点**）、`state[e].ty = Some(c)`、`state[e].val = None`。
  - `ExprKind::Formal` 的 value = `expr_value_half(binding.term)`——对 apply op 节点它取不到“对的”
    value half；取不到时 `value_of` 兜底建 `Index(term, 0)`。
  - 实测读的 base（9168）**不是** `Index`（无 operation、无值），所以这条兜底没有出现在字面量路径上；
    即“字面量里读的容器值”与“`value_of` 的兜底 Index”不是同一个 node。
- 下一轮：在字面量的字段构造路径上打印 `value_of(argument_expr)` 的节点与 `state[argument_expr].val/term`，
  与直接读路径对照（两者是否同一 node），据此定位构造路径把容器值换成裸 cell 的位置。
- 已测（本轮 `DBG_READ` / `DBG_INST`）：
  - 绑定到 apply 的容器：`state.val = 9168`，`state.term = 9172`；9172 是表达式 `@raw[9168, 34, @any, @any]`
    （值槽、类型槽、两个属性）；9168 = `Index(9166, 0)`——`value_of` 对 apply 的兜底：**读值为 apply 节点 9166
    的第 0 项**。
  - 语义核对：apply 的运行期结果是结构体（name table/id/字段），`Index(结果, 0)` 取到的是**名字表**，不是结果本身；
    只有当 apply 的值本身是 `@raw[value, type]` 表达式对时这个兜底才对。
  - 试改“apply 的值 = apply 节点本身”（`state[e].val = Some(node)`，以及 `value_of` 对 apply 返回 term）：
    立刻破坏结构体字面量的字段读（`no field named to/at/value/from in the struct type Struct`），已回退。
- 下一轮：把“apply 的值”按 SPEC 落成**一个值位置**（`@raw[value, type]` 的 value 槽），运行期把结果写进这个位置，
  而不是对 apply 节点做 `Index(…, 0)`；先确认顶层为什么能工作（顶层的 value 槽是不是同一个位置）。
- 已比较（本轮 `DBG_READ2`）：
  - 顶层命名契约（`probe_a_named_inline`）的读：容器 val 节点在**检查期同样是未定**（`val_value=None`），
    所以“顶层值已定”不是差别。
  - 差别在**函数体被 clone**：函数内读的链是 `9538 → 9536 → 9168`，而 `9168 = Index(9166, 0)` 的 9166
    是**模板（检查期）的 apply 节点**；运行时执行的是 clone 的 apply 节点，模板的 9166 不跑。
    顶层不 clone（就地求值），所以同一段 wiring 在顶层成立。
  - 即：clone 重放把读的浅层节点（9538/9536）映射到了 clone 的块，但**没有把链底的 apply 节点（9166）映射到 clone 的 apply**。
- 下一轮：查 apply-clone 的重放映射（`applied_exprs`/`clone_parameters`/`replay_applied`），确认“读的值 = Index(模板 apply, 0)”
  这类深层引用在 clone 里应映射到 clone 的 apply 节点；按 `docs/notes/apply-clone-ownership.md` 的语义补映射（不动 Invariant、不加机制）。
- 已试（本轮）：把 apply 的 term 改成表达式对 `expr_of(apply, c)`（让 `value_of` 的 `Index(term, 0)` 取到 apply 节点本身）。
  结果：`TableGet` 求值处 panic（`evaluation.rs:461`，unreachable "TableGet target must be a table"），已回退。
  结论：apply 的 term = 裸 op 节点是**承重**的表示（不是随手可改的），改动要连带 `value_of`/读路径与 `TableGet` 的假设一起改。
- 更准确（本轮 `DBG_CALL`）：函数内读（215/216/217）的容器 term **不是**裸 apply 节点——`call=None`；
  它的 term 是表达式对 `@raw[9168, 34, @any, @any]`，值槽 9168 = `Index(9166, 0)`，9166 是 apply 节点。
  另一个读（231）的 term 才是裸 apply（`call=Some(12260)`）。
- 由 `Index(apply, 0)` 的语义可推：只有当 apply 的**值**本身是表达式对（`@raw[result, …]`）时，
  `Index(apply,0)` 才等于结果；原生 op（`$plrun`）的答案若是裸值，这条链就取到结果的第一项。
  顶层能工作说明顶层的 apply 值是对（返回值本身是表达式对）；函数内这条链的 apply 值/结果不同。
- 试改“容器 term 是裸 apply 时用 apply 节点当容器值”（`check_named_field`）：不破坏也不修复（`raw none`），已回退。
- 下一步：确认 `wire_apply_result` 写进 apply 节点的值是**表达式对**还是裸结果（`evaluation.rs` 的调用路径），
  以及函数 clone 后该值是否仍是同一对；这决定 `Index(apply,0)` 的读法是否成立。
- 已核对（本轮读码）：
  - `wire_apply_result(node, result, …)` 把**被应用体的值**直接写进 apply 节点（结果若为 ≥2 项数组且
    apply 节点类为空才写），不做额外包装。
  - `check_app` 的 term 是 `expr_of(apply, c)` 这个对；`value_of` 兜底 = `Index(term, 0)`——即
    **对 pair 取第 0 项 = apply 节点本身**，读法是正确的（前一轮把 9166 误当 apply；它其实是 pair）。
  - 所以 wiring 本身没问题，剩下的仍是 **clone**：pair/Index 节点是检查期造的（未打函数标签 → 按
    apply-clone 的规则**共享不克隆**），而 pair 第 0 项的 apply 节点是**打标签按次克隆**的；clone 的
    读仍指向**模板的 pair/apply**，模板的 apply 不跑。
- 下一步：在 clone 重放处把“读的值 = Index(模板 pair, 0)”按 clone 重新指向 clone 的 apply（共享 pair
  但第 0 项应取 clone 的 apply），参考 `docs/notes/apply-clone-ownership.md` 的标签/共享规则。
- 已测（本轮 `DBG_TAG`）：链上的节点（9538、9536、9168、9166、9159）**全部已注册在某个函数里**
  （`registered=true`）——即它们是有标签的模板节点，按 clone 规则本应被克隆。
  但运行期解析用的仍是这些**模板节点本身**（不是 clone 出来的新节点），说明**调用者（`run`）的 body clone
  没有克隆这个结构体字面量/它的字段读**（节点被共享而不是克隆），于是读的链仍停在模板的 apply——那个
  apply 在运行期不跑。
- 注：clone 的 `node_apply` 判据里 `proven_concrete && closed` 会**共享**；这里的字面量/读被判定为
  可共享（closed/共享），但它们内部含 apply（per-call），这是矛盾的来源。
- 下一步：在 `node_apply` 的 `template_content_is_closed`/`proven_concrete` 判定上核对：内容含 apply 的
  聚合值不应被判为 closed；按 `docs/notes/apply-clone-ownership.md` 的“只有闭合才共享”修正（不动 Invariant）。
- 已定位（本轮）：**返回形态 vs 实参形态**的差别在 `Module::evaluate_pattern_argument`
  （`apply.rs:89` 在 `apply_parameter_check` 前调用；实现 `function.rs:748`）：
  - 它先 `evaluate_node(argument)`，再按 pattern 的数组逐项**递归强制**参数（跳过 `shallow` 位置）；
  - 实参形态：定义 pass 会用模板参数把函数体走一遍（apply-time 检查），这次 apply 就强制了字面量的
    字段读 → 读在第一次 plrun 未跑时“跑过”并把类型值写进类（run stamp 置位）；运行期 clone 的启动
    再索取时看到“已跑”，值停在类型值 → `raw none`。
  - 返回形态：字面量是**返回值**，没有 pattern 检查去强制它；它在运行期被求值时才跑，拿到运行期值 ✓。
- 结论：不是 SPEC 的规则问题，是**定义 pass 的实参强制**把 per-call 值提前烘死（与“不物化”冲突）。
  下一步（不新加机制）：让定义 pass 的那次 apply 走**签名检查/惰性**路径，不在检查期 `evaluate_node` 实参；
  先核对 `ParameterCheck::Signature` 路径是否已具备所需信息，再把 `apply_parameter_check` 的强制限定到运行期 apply。
- 已试（本轮）：把 `evaluate_pattern_argument_inner` 里的 `evaluate_node(argument)` 换成“只取已有值”。
  结果：compute **145 失败**、highlevel 2 失败、lowlevel 8 失败——这个强制是**承重**的（定义 pass 靠它
  让 apply-time 检查触发、让 clone 拿到要复制的形状），不能拿掉。已回退，三套件恢复全绿。
- 因此修复点不在“强制与否”，而在**强制出来的值如何被 clone 用**：clone 的 `class_inside` 判据（类 scope）
  决定了这些节点被引用而非克隆，引用让定义 pass 的值留在原类里。下一步核对 `node_class_inside_scope`
  对“类里含全局类型值”的判定，与 SPEC 的 Clone 规则（class 在 f's scope 才 clone）逐条对齐。
- 已按 SPEC 逐条对齐（本轮）：这不是 clone 判据的错。
  - SPEC §Struct 的 `@raw v[k] = e1` 让槽与**类型字段值**（顶层类型表达式的节点）unify；按 Unify 段
    “any side is undecided → merge”，两者成一类；类的 scope = 两者的最近公共祖先 = 顶层 → 按 Clone 段
    “不在 f's scope → create a reference”，**引用是 SPEC 要求的**。
  - SPEC 的 Function 改写只对 **body_clone** 求值：`{let (p_clone -> body_clone) = @clone (…); a = p_clone;
    body_clone}`——**没有**“先用模板参数把模板 body 求值一遍”这一步。
  - 实现多做了这一步（定义 pass 的“body 走一遍以触发 apply-time 检查”），把值烘进类里；这些类又被
    SPEC 要求“引用”，于是烘进去的值跟着引用留到运行期。
- 结论：与 SPEC 的偏差在**定义 pass 以模板参数求值模板 body**（SPEC 没有这一步），不在 Struct/Unify/Clone。
  下一步（最小、不新加机制）：在 clone 的“引用”情形里，把**被引用且带烘死值**的 per-call 节点按 clone 已有的
  reset（run stamp/值）重置，使运行期重新求值；先核对 `node_apply` 的 reference 分支是否已有 reset 可复用。
- 已试（本轮）：在 `node_apply` 的 reference 分支（`!class_inside && !replay_operation && !wildcard`）对
  “有 operation、有值、已 runned”的节点做 `set_node_value(None)` + 清 run stamp。
  - 三套件全绿（267/136/167）；
  - `DBG_RESET` 实测：只重置了一个 **Apply** 节点（9323，值是被烘出来的 array），**没有**触到链上的
    读节点（Index）——说明链上的读不是走 reference 分支，而是走了别的路径（可能被 clone）。
  - 链式探针仍 `raw none`，所以这个 reset 不是充分修复；已回退（不留未证明的语义改动）。
- 对照读数（本轮 `DBG_REF`）：**好的形态也命中同一个 reference 分支**，且它的 Index 节点同样是
  `value=Some(Array), runned=true, class_inside=false`：
  - `probe_a_plan_in_literal`：`[ref] node=7722 value=Some(Array) runned=true class_inside=false`；
  - 失败的实参形态：链上 Index 的 reference 访问是 `value=None, runned=false`（还没烘过）；
  - 所以“引用 + 已烘值”本身不是判别点；差别在**烘的时机/之后是否被重新求值**：好的形态里那个读在
    运行期（第一次 plrun 之后）得到值，返回路径会再次求值它；实参形态没有这条再次求值的路径。
- 下一步：找“返回值求值”路径（`return`/`wire_apply_result`/apply 的 `evaluate_node(applied)`）里对
  读节点的再次求值（`write_node_answer` 的 answer-over-requirement），把它同样用在“实参在运行期
  被启动索取”这条路径上。
- 已试（本轮）：clone 的 `carried` 对 per-call 节点（`depends_on_parameter`）不再携带定义 pass 的答案
  （`template_answer && !depends_on_parameter`）。三套件全绿（267/136/167），但链式探针仍 `raw none`
  ——说明失败的那次读既不是被 clone 的、也不是携带答案的那一支。已回退（不留未证明的改动）。
- 现状小结（19 轮）：所有局部改法（answer 写入、class-owner 索取、字段值求值、apply term/val、容器取
  apply、reference reset、carried 关闭）**要么无效、要么破坏其他不变量**；失败只出现在
  “函数内 + 调用的实参字面量含另一次调用的结果”这一组合，而等价的**返回形态**正常。

## 已知坑

- `check_app` 对 `range_children(argument)` 直接 `state[element]` 会越界 panic，勿重试。
- 计算 `Lower` 侧 `record_element_type` 会触发 `&Module` 借用错误，勿改 `Lower` 持有方式。
- `elementwise_chain` 的 digest 是发射布局的指纹：改 lowering 后它会变，更新 pin 即可，
  但先确认 canonical form 的其他断言仍通过。
- 提交只 add 自己改的路径；不要提交 `README.md`。

## 交接提示

- 压缩上下文后从「下一步」第 1 条继续。
- 需要用户裁决时用 `ask_user_question`，不要用正文提问。

## 量到的判别点（顶层能工作 vs 函数内失败）

同一个输入解码位置，`resolved` 指向的值逐项展开：

- 顶层 `parallel_read_input_buffer`（答 22）：Buf 的 `.native` 项里有一项是
  `ComputeValue(Buffer(..., Int))` —— 真实 buffer 在；
- 函数内 + 用户结构体实参（`probe_a_function_user_struct_argument`，`raw none`）：同一位置的项
  全部 `value=None` —— 真实 buffer 不在。

所以两边的 `resolved` 结构相同（[table, id, ...]），差别只在那个 `.native` 位置有没有 buffer。
下一步就查这个位置的值是谁写的：顶层是定义 pass 就地求值写入，函数内的 clone 是否写成另一批节点，
使解码读到的这一份始终为空。

## 收窄到“实参字面量的槽”（本轮）

同一函数内：

- `compute.collect d.a` → `[1, 2, 3, 4]`（探针 `probe_a_function_direct_collect`）——第一次调用的结果与它的字段读本身是好的；
- `G (ABuf{.a d.a})` → `raw none`——同一个读经过**实参字面量的槽**就丢了。

按 SPEC 说：`e3 = v[k]`（§Struct 实例化）把 `e3`（`d.a` 的读）与 `v[k]`（类型字段 expr `e1`）
merge 成一个 class，而这两处的 value 不同：读自己的答案 = 真实 buffer，类的 value = `e1`（类型值）。
这违反 Lowlevel 段的 `Nodes inside same class must share same value`——一个 class 里出现了两个不同的 value。

所以修复点已缩到这个 unify 的结果上：merge 之后，类里只能有一个 value；`e1`（右）与 `e3` 的答案
（左）不同时，按 SPEC 只能有一个成立。下一步核对实现里是谁把两个值都留下了（`write_node_answer` 写
`e3` 的答案，而类里已有 `e1`），并按 SPEC 的“一个 class 一个 value”修正那一处。

## 输入解码读到的节点（本轮）

失败程序里，named-form 输入循环 `resolved` = 一个 `Index` 节点（类根是它自己），它的 value 是一个
`[table, id, ...]` 形状的数组；`buf_payload` 取到的“字段”里，那层 `.native` 位置是
`NodeId(4721)`：**无值、无 operation 的裸 cell**，`evaluate_node` 返回 None。

也就是说：解码读到的 `.native` cell，**没有任何东西写过**；真实 buffer 在另一个节点上。

下一步（收敛后）：量“给定表达式 `e3` 的实际 Buf 值”与“实例字段（wraps 那个裸 cell）”是否同一
class；若不同类，就是 `e3 = v[k]` 的递归 unify 没有在 `.native` 位置并类——按 Unify 的
“recursive into nodes both side refers”应有并类，缺的正是那一处。

## 分类结论（第 28 轮）

- 跳过定义 pass（`DBG_NO_DEFPASS`）后，失败程序的答案**仍是** `raw none`——所以“定义 pass 提前求值”
  不是本失败的直接原因。
- 直接把实参类型声明在函数内，答案 `[1, 2, 3, 4]` ✓。
- 按 SPEC 推：`e3 = v[k]`（§Struct 实例化）在检查期把字段读与类型字段值 `e1` 并成一类，类里已有
  `e1`（顶层类型值）；按 Lowlevel 段的 `Nodes inside same class must share same value`，类里就是
  `e1`，而 `e1` 的 `.native` 是 `@any`/空——所以程序按 SPEC 的规则就是 undecided。
- 类型声明在函数内时，该类落在函数 scope 内，Clone 规则按次克隆，读被重新求值，答案才是真实 buffer。

因此：这一形状（顶层类型 + 函数 + 用户结构体实参）在 SPEC 现有规则下是 undecided，实现与规则一致；
可用形态是“类型声明在函数内/块内”，或使用插件自己的参数结构（`compute.A`）。此前把它记成“实现缺陷”
的分类应当更正。

## 更正：函数包装形态是可表达的（第 30 轮）

上一轮的“函数包装不可表达”结论收回。按 high 评审的读法与实测：

- **可用的函数形态**（已测，`probe_a_named_contraction_whole_arg`）：

  ```lichen
  contract := ('arg = Arg) -> ((compute.plrun kernel arg) : Struct)
  out := contract ((Arg){.meta meta, .I In{.a data_a.a, .b data_b.b, .plan dp.plan}})
  compute.collect out.c : Array
  ```

  答出参考值 `[2938.0, 7042.0, 3016.0, 7264.0, 3094.0, 7486.0, 3172.0, 7708.0, 3250.0, 7930.0]: Array` ✓

- 关键在于：**实参 `Arg{…}` 在包装函数之外构造**（计划推导 `dp := plrun p_kernel ps` 也在那里），函数体里只有 `arg`，没有含 `dp.plan` 的类型化字面量 → 不触发跨 scope 的 class 引用。
- 限制：计划推导必须在调用方一侧；把整个 `Arg` 构造搬进函数体仍是上一轮测到的那种形态（`raw none`）。
- 上一轮把“引用”推成“永不重新求值”是过度推断：更准确的说法是“launch 解析到一个**没有 operation 的声明侧空 cell**”，而 SPEC 对此不表态。

## SPEC 对“launch 输入解析该走声明侧还是 supplied 侧”没有规定

`plrun` 的 native-buffer 路径选择不在 SPEC 里（SPEC 自称 “Illustration only, not precise”）。因此：
- 交付用**调用方构造 Arg 的函数包装形态**（已给参考值）；
- 若要“类型化字面量在函数体内构造”也成立，那才是需要 maintainer 决定的问题（`Parallel` 的输入解析在声明侧为空时是否应跟随 supplied 的实际答案）。

## 那个空 cell 是谁也不管的节点（第 31 轮）

- 失败程序里 `resolved` = `NodeId(9373)`，其 payload cell = `NodeId(4721)`；
- 给 `node_apply`（clone 走路）打点后，**4721 从未进入过 clone 走路**（既没有 clone 也没有 reference 记录）；
- 所以它是**检查期由实例化造出来的节点**，clone 走路不经过它，运行期也就没有任何东西写它——与它“无值、无 operation”的读数一致。

按 Merge 章节，类求值会触发类内节点的 operation；这个 cell 没有 operation，也不在任何被触发的路径上，所以它保持空。
下一步不能靠“再跑一遍 operation”修（上一轮已试，还破坏两个设备测试）；要看的是**实例化的槽为什么指向这个检查期 cell**，以及 clone 应当把它带到运行期的那一份是哪条路径。

## 结论：这是语言的问题（第 34 轮）

### 证据

1. 最小例子的解码处读数：

```
[src] resolved=NodeId(8628) root=NodeId(8628) payload=NodeId(3296) payload_root=NodeId(12446) source=None
```

实例字段的 node 是一个**孤立 class**（类根就是它自己），你写的 `d.a` 不在这个 class 里；`field_sources` 也没有链接。

2. 代码：`check_instantiate`（`crates/lichen-highlevel/src/checker/structs.rs`）里，只有
   `actual_value_is_decided` 时才执行 `check_unify(给定, 槽)`；给定 undecided 时 **跳过**了这一步
   （注释写着 “Undecided values cannot mismatch; check only type and attributes”）。
   而 SPEC §Struct 的改写是**无条件**的 `e3 = @raw v[...]`，Unify 段本身就规定
   “any side is undecided → merge”。

3. 实验：把该 `check_unify` 移出 `if`（两分支都执行），最小例子的失败**改变形态**——
   从 `raw none` 变成 kernel 编译诊断：
   `compute.parallel: a kernel body reached a node with neither a value nor an operation`。
   说明类的 merge 确实发生了（解码走到了合并后的节点），只是 kernel 编译路径还不能处理
   合并进来的 undecided 读。整包 compute 测试 40 个失败。

### 回答

**语言的问题**：§Struct 的 `e3 = v[k]` 对 undecided 的给定值没有执行；这不是 compute 独有的，
也不是插件读法的问题。修正它需要不止一行：还要让 kernel 编译路径接受“合并进来的 undecided 读”
（上面第 3 条的失败形态）。

## SPEC 修复的进度与实验（rounds 1-3）

已提交两步 SPEC 要求（WIP，红）：

- `70266a42`：§Struct 的 `e3 = v[k]` 无条件 unify；类求值触发类内所有成员 operation；
  kernel 编译对裸成员增加“类内 operation”回退。
- `75b514ae`：任何一次读都触发类求值（`evaluate_node_body` 去掉 `has_no_result_yet` 条件）。

### 实验（本轮）

对 `parallel_read_input_buffer` 逐个回退，确定是谁破坏它：

| 状态 | 结果 |
|---|---|
| 三处全部回退（基线） | `test result: ok. 1 passed` ✓ |
| 只保留类触发（回退 unify） | **失败**（检查期 `read's buffer argument is not an input buffer ...`） |
| 只保留 unify（回退类触发） | 也失败（答案变 0） |

读数：kernel 体里读的节点（如 `21535`）是一个**孤立 class**，参数槽的 expr 类根（如 `322`）与读链底
（`13218`）都不相等，`param_path` 的“槽 expr / 槽 value 同类”匹配没有命中；`parallel_buffer_pos`
的同类回退也没命中。

结论：两块 SPEC 要求本身都要实现，但**插件的参数路径解析仍按节点链启发式**匹配，没有按 SPEC 的
class 模型走；下一步是把 `param_path` / `parallel_buffer_pos` 的匹配改为“先求值类、再按类找参数槽”，
这是插件内部的改动，不新增语言机制。

## 第三块缺失：`@raw v[k] = e1`（round 4）

按 maintainer 的口径：Any 段“only merge if another side is right side and undecided”——
`e3 = v[k]` 里给定在左、`v[k]` 的 `@any` 在右，**不 merge**，class 保持 undecided。所以读的答案只能来自
**给定表达式自己的 value**（e3），不能指望类。

本轮量到：实例字段（`8628`）是**孤立 class**，`expr_value_half(8628) = None`，`field_sources` 在它上面
查不到；而 `check_instantiate` 记录 `field_sources` 的键是**模板字段**的 value（`required_value =
expr_value_half(field)`），值是给定 expr 的 value（`actual_value`）。

即：实例的槽（`v[k]`）与类型的字段值（`e1`）**没有并类**——SPEC 的 `@raw v[k] = e1` 这一步被
`instantiate_requirement_value` 的“复制开放槽”替代了（复制出新 cell，不是 unify）。

结论：要让解码顺 `field_sources` 找到给定值，必须先实现 `@raw v[k] = e1`（实例槽与类型字段值并类），
这样 `field_source_of` 的类根回退才能从实例槽走到模板字段、再到给定值（e3）。这仍是 SPEC 的原有步骤，
不是新机制。

## `v[k] = e1` 的第一次尝试（round 5）

在 `check_instantiate` 里加了两步：

1. 复制前捕获模板的字段项：`shape::value_items(template_value)` 跳过 `STRUCT_VALUE_FIELD_BASE` 的节点；
2. 在参数循环里先 `check_unify(实例槽, 模板字段项)`（SPEC 的 `@raw v[k] = e1`），再 `e3 = v[k]`。

结果：

- 最小探针仍 `raw none`；
- 能工作的 `parallel_read_input_buffer` 从通过变成失败，诊断 `expected Int, found Struct`。

说明捕获到的“模板字段项”不是一个字段的 value（可能是 expr 对或嵌套槽），与实例槽不是同一层，unify 直接冲突。
已回退，基线恢复（`parallel_read_input_buffer` 通过）。

下一步：量清楚实例槽与模板字段项各自是什么节点（value / expr 对 / 槽），再决定 `v[k] = e1` 的正确一端。

## 探针里的 launch 由谁求值（round 8）

- 最小程序里 `ParLaunch` 的算子**被求值**（`PLRUN eval recording=false` ×3），但**从不进入
  `run_parallel_kernel`**（`[pl] outcome` 打印从未出现）；
- `value_at_path` 在最小程序里解析出的标量叶子（`564`）始终 `value=None`；
- 对照的 `probe_a_function_direct_collect` 同样打印叶子 `None`，但 collect 仍答 `[1,2,3,4]`，
  而且**它的 `PLRUN` 也一次没打印**。

结论：测试跑的这些程序，launch 的结果不是由我插桩的这个动态求值臂算出来的——很可能走的是
**frozen/static 模块**的路径（测试 harness 会冻结），我的插桩只在动态模块里，所以看不到。

下一步：确认测试 harness 是否冻结程序（`run`/`evaluate_raw` 的 static 路径），以及 static 模块里
`ParLaunch`/输入解码的处理是否与动态路径一致——这正好也是“不等于 compute 特例”的落点。

## 冻结假设被排除（round 9）

`evaluate_raw` → `compile_with_imports_at` → `render_build`（`crates/lichen-language/src/run.rs:85-93`）：
直接在**动态模块**里 `evaluate_node_deep(build.root_val)`，没有冻结/static 路径。所以上一轮的
frozen 假设不成立。

剩余问题：最小程序里 launch 的 `ParLaunch` 臂进入了、但没走到 `run_parallel_kernel`（在输入解码里
提前返回）；而 collect 形态能拿到值，说明它的值来自另一条路径。这段调查已交给一个 coding subagent
（`launch-path-fixer`）继续，套件保持 `279 passed, 1 ignored`（`52876710`）。

## Launch-chain measurements (delegated slice)

Baseline `b3e1a986`'s parent (`0b55b661`): `test result: ok. 279 passed; 0 failed; 1 ignored`.

### Where the working collect probe's launch value comes from

`probe_a_function_direct_collect`: the launch runs during the `Evaluate` build's
**top-statements** pass — the program's execution is that pass, and `render_build` only
re-evaluates the root. Trace: `[phase] top-statements` → `PLRUN` → `[run] kernel …
leaves=[4] inputs=0` → `collect` answers `[1, 2, 3, 4]`. The ParLaunch arm does reach
`run_parallel_kernel` there; the earlier reading "never reaches it" holds for the
minimal probe only.

### The failing launch's early return

`probe_a_function_chain_minimal` enters the arm several times but the **producer** launch
never runs: its scalar leaf (`Fill{.n n}`) is absent in the second build
(`[launch leaf 0] … value=absent`), and the consumer's dependency short-circuits it in
the third. The consumer's input decode resolves `cfg.b`'s field to the read's value node
(an isolated class whose value is the type declaration's `Buf` wrapper), whose `.native`
is the declaration's `@any` cell; `recorded_buffer_payload` follows `field_sources` to a
check-time read with no value and returns `None`, so the arm exits at

    let Some(node) = payload else { return None };

with no `run_input` refusal. The other early returns before `run_parallel_kernel` are not
on this path.

### The differing step

The per-use copy (`instantiate_requirement`, `crates/lichen-highlevel/src/checker/templates.rs`)
bailed out when a field's type half was a **frozen ref**: `value_items(field)[1]` is
`Static(compute.Buf)` for every `compute.Buf` field, so
`items.get(1).and_then(|item| item.node.dynamic())` is `None` and the function returned the
template's own field expression. Measured: `template_field == slot_field == 186`,
`slot == required == 3294` — so `v[k] = e1` (57659de3) was a no-op and the instance shared
the declaration's field value, `.native` included.

Top level has no function apply, so the supplied read (`inbuf.z`) keeps its own class and
its runtime answer is the buffer (class `{9428}`). Inside a function the apply's parameter
unify merges the supplied read with the declaration wrapper (measured
`[equality] a=8640 b=186`), and a right-side `@any` never merges, so the answer cannot land
in the shared `.native` cell.

### The attempts, both reverted

- `b3e1a986`: the copy fix. The copy runs (`copied=NodeId(8686)`) and the failing class
  becomes four per-use copies plus the read. The probe still answers `raw none`: the decode
  reaches a supplied read node (`op=Index`, `runned=false`), but evaluating it does not
  force the producer apply. Compute suite: 276 passed, 3 failed —
  `buf_element_class_recorded`, "argument 0 of this dispatch is not a buffer", because the
  recording resolved the wrapper through `field_sources`, which the shared declaration node
  used to satisfy. Reverted in `745583ba`.
- An `evaluate_node_body` rule ("a class value holding an open cell is not an answer") plus
  a decode-side evaluation of the resolved node: no fix, and the class walk re-runs
  undecided operators on every read (trace 4.4k lines). Reverted.

### Remaining blocker

The direct decode has no mapping from the check-time instance tree to the runtime clone's
read: `parameter_projection`/`parameter_images` supply that for parameter images only, not
for a function-local binding such as `d`. Per Merge the class must end with the read's
answer, but the read's one attempt was consumed before the data existed (`[stamp]` measured
between the merge and the third build's top-statements), and the class value it was filled
with is the declaration's requirement.

Suite after both reverts: `test result: ok. 279 passed; 0 failed; 1 ignored`.

## 全套 SPEC 修复的组合测量（round 10）

把三件套叠加在 copy 修复之上（`a5ff2f66` 重新应用，随后回退）测到：

- **copy 修复确实建立了 per-use 链接**：解码处的 slot = `8700`，`field_sources` 有
  `source=Some(3443)`、payload `8718`；`probe_a_function_direct_collect` 仍答 `[1,2,3,4]`；
- 加上**无条件 `e3 = v[k]`** 后：套件 252 passed / 27 failed（无 copy 修复时是 40 failed）；
- 再叠加**类触发**后：`parallel_read_input_buffer` 在检查期失败；整包出现**挂起**（有测试循环），
  已终止；
- 最小链式探针的失败形态：kernel 编译报
  `a kernel body reached a node with neither a value nor an operation (node=3435)`；
  该节点是无来源（`field_sources` 无链接）、孤立的空 cell。

## 结论

- 语言侧（SPEC 的两步）实现后，瓶颈转到 **kernel 编译体**对 per-use 副本/空 cell 的处理；
- SPEC 的 Any 规则（右侧 Any 不 merge）意味着**类的值拿不到给定答案**，读者必须沿
  `field_sources` 走到给定节点——语言侧已经建立这条链接，剩下的是插件读者与 kernel 编译按它走；
- 这是一次插件内部的重构（参数路径、kernel 体的空 cell、recorded 路径），不是一行改动。

当前基线：套件 `279 passed, 1 ignored`，工作树干净（只剩 `README.md` 与 maintainer 的
`docs/language-spec.md` 编辑）。

## `_ = @any` 的方向规则（round 11）

按 Any 段的例子（`_ = @any` := `_`、`{a := _; @any = a}` := `@any`），在 `unify_inner` 的
`(None, Some)` 分支加了规则：**右侧是 `Any` 时不并类**（左侧保持空），而左侧是 `Any` 时仍并（右侧变 `Any`）。

- 套件计数不变（252 passed / 27 failed），所以它不是当前链的卡点；
- 但它与 SPEC 的例子一致，已提交（`116568f7`）。

当前链的卡点仍是：kernel 编译体在函数形态下遇到孤立空 cell（`3435`），以及 `e3 = v[k]` 对 `@any`
槽的填充语义。

## 通用 tracing 的设计（round 12）

compute 插件要“通用的 tracing 合法 lichen 结构”，即三个读者（直接解码、recorded 解码、kernel 编译体）
都只沿语言自身的结构走，不依赖具体形状或节点 id。可用的边（按优先级）：

1. **expr 的 value half**（`expr_value_half`）：一个表达式的值位置；
2. **struct 字段的 supplier**（`field_sources`，required → supplied）：SPEC §Struct 的
   `@raw v[k] = e1` / `e3 = v[k]` 记录的那条边；
3. **apply 的参数镜像**（`parameter_images` / `clone_parameters`）：形参的值节点 ↔ 实参的值节点；
4. **节点的 origin**（`node_origin`）：clone → 模板；
5. 只有以上都走不通时才是“空 cell”，拒绝。

本轮读数：kernel 编译体拒绝的 `NodeId(3435)` 位于 `BlockId(2)`，无 origin、无 `field_sources`（两个方向
都没有）、类根是它自己——是检查期的一个裸 cell，编译体沿读链走到了它。要判断它该由哪条边解析，需要
把它与 `self.params` 的 slot（expr / value）对照；下一步继续。

## 3435 不是参数槽（round 13）

kernel 编译体的 `self.params` 只有一个槽：`expr=540`、`value=535`；被拒绝的 `3435` 与两者的类根都
不相等。`3435` 位于 `BlockId(2)`、无 origin、无 `field_sources`、类根是它自己——即检查期由
`instantiate_requirement_value` 为开放槽造的 fresh cell（类型的字段值副本）。

所以编译体沿 `p.in.a` 的读链走到了**类型字段值的副本**，而不是参数槽本身；`parameter_path` 的基座
匹配因此失败，落到了 `opaque`。

下一步：确认 `p.in.a` 的 Index 链在检查期是以参数 `p` 的 value（槽 535）为基座，还是以类型字段值的
副本（3435）为基座；如果是后者，就是检查器构造读的位置需要按 SPEC 的 `s.value[...]` 对齐。

## 读链的基座是 per-use 副本（round 13）

编译体走到 `p.in.a` 的读链时，链上的 target 都在 `BlockId(3)`，其类根是它们自己（`11895`、`12076`、
`12078`），而参数槽是 `expr=3479`、`value=3474`——两者类根都不相等。

也就是说：copy 修复生效后，kernel 体读的基座是检查期造的 **per-use 副本**（同 block、高 id），
不是参数槽的 value；`parameter_path` 的基座匹配因此失败，落到 `opaque(3435)`。

下一步的分叉：

1. 让 `parameter_path` 把“副本 → 参数槽”的边（`field_sources` 的 required→supplied，或 apply 的
   参数镜像）接上，按这条边识别基座；
2. 或者让检查器构造读时直接以参数槽的 value 为基座（`s.value[...]` 的 SPEC 改写），副本只作为槽值。

两条都可行，但一条改插件、一条改检查器；需要 maintainer 定方向。

## 参数镜像边不覆盖当前读链（round 13 续）

按 maintainer 选的“插件侧接边”，在 `parameter_path` 的基座匹配里加了第三条：**参数的 image 类**
（`module.parameter_images`，formal → image）。实测：

- `parameter_images` 里的条目是 `12097→12425`、`8046→8807`、`1735→2142`、`9296→11498`、
  `11566→12963`、`4753→…` 等；
- 读链基座（`11895`、`12076`、`12078`，都在 `BlockId(3)`）**既不是任何 slot 的类，也不是任何
  image**；slot 的 expr/value 是 `3479/3474`，也不在 map 的 key 里。

结论：这条读链的基座不是“参数槽 / 参数镜像”能命名的东西；它是检查期的 per-use 副本。
`22a7a42d` 提交了这条边（无害，套件计数不变），但解决 27 个失败还需要找到能命名该副本的边
（候选：该副本的 `field_sources` required 侧、或检查器构造读时的基座选择）。

## 读链基座的 origin 是 Apply 结果（round 14）

`parameter_path` 的基座（`11895`、`12076`、`12078`）都有 `origin`，且 origin 是 **Apply** 节点：

```
origin=6947/3760  op=Apply  value=Array
```

origin 的类根（6947/3760）也不等于参数槽（3479/3474）。所以：

- 基座不是槽、不是 image、origin 也不是槽；
- 它是**被 apply 后的实参结果的 clone**——与 `parameter_path` 注释里的不变量一致
  （“a parameter the deep pass unified with its argument is a computation by codegen time”），
  即编译期看到的参数值就是实参那边的节点。

可选的下一条边（需要 maintainer 定）：

1. **`parameter_images` 的记录扩展到 apply 结果**：clone 参数时把“实参结果 → 形参槽”也记下来；
   检查器/低层已有 `clone_parameters`，扩一个映射即可（最贴合现有机制）；
2. **按 apply 的 operands 追**：基座的 origin 是 Apply，用 `node_function` + 该 apply 的实参
   节点对照 `self.params`（不动低层，但需要 apply 的 operand 仍可读）；
3. **接受“参数值即实参”**：编译期把基座当参数处理（在 `parameter_path` 里允许基座是
   Apply 结果），最省事但放宽了匹配。

## 3435 不在读链上：它是 codomain 的 Buf `.native` 槽（round 15，已修）

round 14 的三条候选边都假设被拒的 `3435` 是读链基座。**实测不是**：

1. kernel 编译分两次。`fid=1`（`mk`，slot `expr=540`/`value=535`）先拒绝；`fid=3`（`sum`，
   slot `expr=3479`/`value=3474`）就是 round 14 量到的那条 `p.in.a` 读链，它的
   `parameter_path` 基座匹配确实失败，但**不产生**拒绝（链上的节点都有自己的值/origin，
   走查继续）。报错里的 `3435` 来自 `fid=1`。
2. `fid=1` 的走查（`lower_index_function`）：codomain 的输出 = `3439`（`compute.write` 返回的
   `Buf` 实例）；`3439` 无 operation、无 origin；`parameter_path(3439)` 不中 →
   `first_slot_value(3439)` = `3435`（该实例的 `.native` 字段值 = `instantiate_requirement_value`
   为开放 `@any` 造的 fresh cell）→ `parameter_path_of(3435)` 不中 →
   `lower_definition(3435)` → `definition(3435)` 无 operation → `opaque(3435)`。
   即：**不是读链基座，而是声明实例的开放 `.native` 槽**。
3. 三条候选边为什么都命名不了它（实测）：
   - `3435` 的 `node_origin = None`（边 2、边 3 都要 origin=Apply，不可能）；
   - `clone_parameters` / `parameter_images` 对 `3435` 及其类根都无条目（边 1 要扩的
     “apply 结果 clone → 形参槽”没有 clone 可扩）；
   - `field_sources` 两个方向都空；类成员 `{3435, 3316, 8718, 8737, 9707}` 全部无值、无 operation
     （`9707` 的 origin 是 apply `2636`，但类里没有任何 operation，`defining_member` 仍答 Opaque）。
4. **能命名它的是既有的第四条边**：`3439` 自己有一条记录
   `field_sources[3439] = 3443`，`3443 = Index(2636, 0)`，`2636` 就是 `mk` body 里的
   `compute.write` apply；它的 clone `9626` 是 `ComputeOperator::Write`（`origin=2636`，
   args = `[to, at, value]`）。`Lower::supplied` 本来就沿这条边找供应商，只是它排在
   `first_slot_value` 之后，永远轮不到。

### 修法（`0cbf3ae2`，一行逻辑 + 一个小判定）

`definition` 的 no-operation 分支里，`first_slot_value` 只在槽**有定义**（有 operation 或有值）时
继续；开放槽（SPEC §Any 的 `@any` 不会被右侧 merge）落到既有 `supplied` 边。
声明实例（`compute.buf` 这类 `.native` 有 `$buf` op 的）行为不变。

- 探针：`probe_a_function_chain_minimal` **不再在 kernel 编译处拒绝**，答 `raw none`；
- 套件：`test result: FAILED. 252 passed; 27 failed` → `254 passed; 25 failed`（无新增失败），
  两个转为通过：`loop_write::a_body_that_is_a_fold_writes_inside_the_folds_step`、
  `refusal_over_an_answer::a_refusal_over_an_input_buffer_is_reported_beside_an_answer_that_arrived`。

### 剩余的两个 stop（本轮实测，都已不是 kernel 编译）

`raw none` 的原因在 launch 的输入解码，与 `3435` 无关：

1. **producer（`fid=1`）的标量叶**：`[leaf] node=8041 value=None`；其类 `{8041, 8068}` 都无值，
   且 `field_sources[8041] = 8041`（自指）——`recorded_leaf` 因此原地停住，`n` 的运行期值
   不在这个类里。这是“函数内实参字面量的槽”老问题的新读数（round 19/28/31 的同一族）。
2. **consumer（`fid=3`）的输入 buffer**：`[input] path=[1,0] resolved=8700 payload=None`；
   `buf_payload(8700)` 给 `8718`（开放），`8700` 的 `field_sources` 源是 `3443`（同一个 apply），
   `recorded_buffer_payload` 的投影循环返回 None。

两条都落在“检查期模板值 vs 运行期 clone 值”这条旧链上，需要 maintainer 定方向（本切片未动）。

### 结论

- round 13/14 的“读链基座”方向不是本探针的拒绝点；**三条候选边都命名不了 `3435`**（理由见上），
  能命名它的是 `field_sources` → apply 的既有边，且修复后拒绝消失。
- 余下的是 launch 解码的两个 stop（上面第 1、2 条），数值可复现，与 kernel 编译无关。

## 生产者 launch 在运行期不跑的原因（round 15）

- 消费者（`sum`）的 extent 叶子能解析到 2（`[leaf] node=8677 value=USize(2)`），但它随后在
  输入 buffer（path `[1,0]`）失败：`[resolve] 8700 → 3443`，两者 payload 都开、`3443` 没有
  `field_sources` 也没有 payload 来源，resolver 返回 `None`；
- 生产者（`fill`）的 launch 在检查期就因标量叶子（`8041`，无值）在 `_ => return None` 分支返回；
  运行期没有对应的 `[form]` 打印，说明**生产者的 launch 在运行期也没有成功进入执行**——它的
  实参字面量字段没有拿到形参 `n` 的 2。

剩余链条：形参 `n` 的值（2）→ 函数体内字面量 `(Fill){.n n}` 的 `.n` 字段（clone 后）；
这是逐次 clone 的形参值传播，下一轮从这里量。

## 生产者 launch 只在检查期求值一次（round 16）

按 kernel id 区分后：

```
[leaf] kernel=10704…（生产者 fill）  cfg=11496 leaf=8041 value=None   ← 唯一一次
[leaf] kernel=10305…（消费者 sum）  cfg=14204 leaf=8677 value=2       ← extent 两次
[leaf] kernel=10305…（消费者 sum）  cfg=14267 leaf=8677 value=2
```

- 生产者的 launch **只在检查期**用模板的 cfg（`11496`）和未定值的叶子（`8041`）求值了一次；
- 运行期消费者读 `d.a` 时，**没有再次 demand 生产者的 launch clone**——所以它的字面量字段永远拿不到 2。

`parameter_projection(container, 8041, block)` 对 8041 返回 `None`：它不是 Index 链（`path.is_empty()`），
而是一个纯值节点（形参 `n` 的值），所以投影机制不覆盖它。

下一步：量消费者在运行期读 `d.a` 时，它的基座（生产者的 launch clone）为什么没有被 demand——
是读的类已经有了检查期的 None、还是 clone 的 launch 节点没有被建立。

## 读的 value 短路了 Index 的 demand（round 17）

- `Index` 的求值本身会 demand 它的基座（`evaluate_node(operands[0])`），所以只要读**重新跑一次**，
  就会去 demand 生产者的 launch clone；
- 但读的类里有值（声明侧 `Buf`），闸门 `if let Some(value) = self.nodes[node].value { ... }`
  在 `has_no_result_yet(node)` 为假时直接返回值、不跑 operation；
- 克隆侧的规则在 `crates/lichen-lowlevel/src/function.rs:960-990`：`carried_answer` 只在模板答案是
  “已证明”时才 `stamp_node_ran_if(clone, true)`，否则克隆的 `runned` 应为 false（即应重跑）。

下一轮量：运行期读 `d.a` 的节点是模板节点还是 clone、它的 `runned`/`value` 各是什么——这决定闸门
是否应当按 SPEC 的“a value a unification wrote is not the operator's answer”把它当作未答、重跑一次。

## 解码读到的仍是模板节点（round 18）

`[state]` 读数（缓冲/负载：节点、是否 undecided、origin、操作）：

```
buffer=8700 Index undecided=true  origin=None
payload=8718 undecided=true origin=None
buffer=3443 Index undecided=true  origin=None
payload=9707 undecided=true origin=Some(2636)   ← write apply 的 clone
```

- 消费者运行期解码 `value_at_path` 落到 `8700`/`3443`（**模板节点，无 origin**），它们的值来自
  类的 unify（声明侧 `Buf`），不是 operator 的答案；
- 这两个读在检查期跑过并被 `ran_undecided` 打了 run stamp，所以闸门的 `has_no_result_yet`
  为假、不再重跑；
- 字面量 clone 的字段值节点被**共享**（无 origin），所以运行期没有 demand 生产者的 launch。

下一步：让字面量 clone 的字段值也走 clone（`node_apply` 的 value 映射），或者运行期重新打开
模板里未决的读（清 stamp）——前者更符合“每次使用一份副本”的 SPEC 语义。

## 字面量里的读被 clone，但解码走到模板节点（round 19）

- `[node_apply] node=NodeId(3443)` 出现两次 ✓：给定读（`d.a`）确实被 clone 了；
- 但 `[cfg] cfg=14204 path=[1,0] resolved=8700`：解码从 clone 的字面量出发，却在 `.in.a`
  处落到**模板的** `8700`；
- `8700` 不是 identity item（`identity=Some(false)`），所以它不是“声明身份”被有意共享的。

即：clone 的字面量里 `.in` 字段的值链（嵌套 `B1{...}` 的 struct 值）仍指向模板的结构，
`value_at_path` 顺着它取到模板的读/值节点；而 clone 出来的读（`3443` 的 clone）没有被解码走到。

下一步：量 clone 的 `.in` 字段槽的值（是 clone 的结构还是模板的结构），以及 `value_apply` 对嵌套
struct 值的映射为什么把 `.a` 的值节点留成了模板节点。

## clone 只到一层（round 20）

`[path]` 读数（消费者路径 `[1,0]`）：

```
at=1 field=14242(克隆的 .in 槽) → node=8725（无 origin，有值：映射出的数组）
at=0 field=8712（.a 槽，无 origin） → node=8700（无 origin）
```

- clone 的 `.in` 槽节点（`14242`）存在，但它的值是**映射出的数组**（`8725`，无 origin）；
- 该数组的 `.a` 槽（`8712`）却是**模板节点**（无 origin）——嵌套层级没有被 clone；
- 所以解码在 `.in.a` 处取到模板的 `8700`，而不是读的 clone。

即：`value_apply` 只映射了外层字面量一层，嵌套 struct 值里的字段槽仍是模板的。

## clone 判定的实测（round 21，按更新后的 SPEC §Clone）

SPEC §Clone（用户更新）：“undecided node, if its class is in function f's scope, must be cloned,
otherwise reference it.”

`[scope]` 读数：

```
node=3443 root=3443 inside=true  depends=true   → cloned ✓
node=8725 root=41   inside=false depends=true   → referenced（共享）
```

`8725` 是 `value_apply` 为 `.in` 字段值映射出的新数组，undecided（proven=false、closed=false），
但它的类根是 `41`（声明侧），所以 `class_inside_scope` 判为 false → 被引用而不是 clone；
它的字段槽（`8712`/`8700`）因此保持模板节点，解码永远取到模板读。

待确认的偏差点（三条候选，需要 SPEC 的原意）：

1. **“its class is in f's scope”** 是否指节点**自己所在的 scope**（`8725` 是在函数体里被
   `value_apply` 造出来的），而不是合并后类根（`41`）的 scope？
2. 还是 **“Clone an undecided node will detached from its original class”** 要求 undecided 节点
   无论类根如何都必须 clone（并脱离原类）？
3. 还是 `8725` 本不该 undecided（它映射自一个已定结构，只是元素里含模板读）？

## 按更新后 §Clone 再读（round 21）

更新后的规则：

- “undecided node, if its class is in the applying function's scope, must be cloned, otherwise
  reference it”；
- “for efficiency: clone recursive into a decided node if the deep_decided flag is false；
  reference node outside the applying function's scope”；
- “Clone an undecided node will detached from its original class for let-polymorphism.”

实测对照：`3443`（模板读）inside=true → cloned ✓；`8725`（`.in` 字段的映射值）undecided，
但类根是声明侧 `41` → outside → referenced。

两条 SPEC 规则的联合效果：

1. §Struct 的 `v[k] = e1` 把 per-use 副本与声明侧字段 unify（两边都已定时会下钻并合类），
   所以副本的类根落在声明作用域；
2. §Clone 于是判它在 applying function 的作用域之外 → 引用而不是 clone。

因此**按字面**实现是自洽的；但若 per-use 副本要保持“脱离原类”（§Clone 最后一句），那么
`v[k] = e1` 不应让副本并入声明侧的类——两种读法给出相反的结论。

我按字面读：`v[k] = e1` 合类 → 副本在声明侧 → 解码走类 → 取到模板读；要按“副本脱离”，
则解码必须沿给定表达式的供给边走到它在本次调用里的 clone（`3443` 的 clone 确实存在，
但其映射只存在 per-apply 的 `ctx.remap` 里，没有全局可查的边）。

## 最小纯 lichen 错误例（round 22）

把 §Struct 与 §Clone 按 SPEC 实现后，最小的纯 lichen 失败例子是：

```lichen
T := {.a 1}
f := 'n -> (T{.a n}).a
f 2
```

读数：`refused [("expected Int, found Int", Some((4, 3)))]`。

同形状但值相同（`T := {.a 7}; f 7` 或嵌套 `Inner{.x 7}`）答 `7: Int` ✓。

机制：§Struct 的 `v[k] = e1` 把字段的**声明值**（`1`）与**给定值**（`2`）unify；Unify 段规定
“两边已定且不等 → 错误”，于是拒绝。实现按 SPEC 是对的——它暴露的是“字段声明值是已定值时，
该字段就不能再被赋予别的值”这一语义。

嵌套版本（更接近 compute 的类型写法）：

```lichen
Inner := {.x @any : Int}
T := {.a (Inner{.x 1})}
f := 'n -> (T{.a (Inner{.x n})}).a.x
f 7
```

同样被拒：`expected Int, found Int`（span `(5, 3)`）；把声明值换成 `7`（与给定相同）则答 `7: Int`。

探针：`probe_a_spec_struct_minimal_error`、`probe_a_spec_struct_and_clone`
（`crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs`，print-only）。

另：类型函数字段（`Buf := 'p -> {...}; T := {.a (Buf Param)}`）会 panic
`TableGet target must be a table`——但该 panic 在 SPEC 实现之前的基线（`4d2cb04e`）上同样出现，
是既有缺陷，不是本次 SPEC 实现引入的。

## Any 字段的读数与诊断修复（round 23）

三个最小纯 lichen 形状的实测：

| 程序 | 结果 |
|---|---|
| `T := {.a @any : Int}; f := 'n -> (T{.a n}).a; f 2` | `2: Int` ✓ |
| `Inner := {.x @any : Int}; T := {.a Inner}; f := 'n -> (T{.a Inner{.x n}}).a.x; f 7` | **`none: Int`** |
| `T := {.a 1}; f := 'n -> (T{.a n}).a; f 2` | refused `expected 1, found 2` |

- **Any 的情形不报错**，而是给出 `none`：`v[k] = e1` 先把槽与 `Inner`（`.x` 是 `@any`）并类，随后
  `e3 = v[k]` 下钻到 `.x`——给定值 7 在左、`@any` 在右，按 §Any 不并类，于是类里仍是 `@any`，
  读出来就是 undecided（`none`）。这正是 compute 链 `raw none` 的最小纯语言复现。
- **报错的是“声明值已定”的情形**（`{.a 1}` 对给定 2）：两边已定且不等 → Unify 报错。

诊断：`DiagKind::Runtime` 原来用 `printer.node(d.a/d.b)`，两侧都是 `Int` 类型常量时消息成为
“expected Int, found Int”，不指名冲突的值。已改为**两侧渲染相同时取记录值**（`d.value_a`/`d.value_b`），
现在输出 `expected 1, found 2` / `expected 1, found 7`。

## `7 unify @any` 的实测（round 24）

在 wildcard 分支里加探针测到（prelude 自身的 unify 也会经过它）：

```
[any] a=1  b=47 va=TypeInt   vb=Any
[any] a=2  b=47 va=TypeFloat vb=Any
[any] a=156 b=147 va=Array    vb=Any   ← 我们的 .x 值槽
```

- 同一个 `Any` 槽（`47`）在 prelude 里分别与 `Int`、`Float` 两个类型 unify——如果按“左已定就写进右侧
  类”实现，就会把 Int 写进该多态类，随后 Float 的 unify 冲突（实测得到 `expected Int, found Float`，
  `span=None`）。所以**不能写右侧的类**；
- 左已定时“node obtain the left side value”指的是**左侧节点自己保留值**（7 仍是 7）——当前实现
  正是如此（wildcard 分支返回成功、不并类）。

那 `none` 从哪来：`e3 = v[k]` 之后，槽 `v[k]` 的类仍是 `e1`（模板的 `Inner`，`.x` 是 `@any`），
而 7 留在 `e3` 自己的节点上；读 `d.a.x` 读的是槽 `v[k]` → `@any` → undecided。

所以问题变成：SPEC 的 `e3 = @raw v[k]` 的**方向**——是“槽保留 e1、e3 保留自己的值”（当前实现），
还是“槽取 e3 的值（v[k] = e3）”？前者给出 `none`，后者给出 7。

## 按“v 的槽是 @raw None”改的实测（round 25）

按用户指正（`v := @raw[name_table, nominal_id, @raw None, @raw None]`，实例的槽是 None，
模板的 `@any` 不可能进 `v`；7 应与 `v` 里的 None unify）：

1. `instantiate_requirement` 的字段值改为 `fresh_cell()`（`v` 的空槽）；
2. `check_instantiate` 的顺序改为先 `e3 = v[k]`、再 `v[k] = e1`。

读数：

- `[inst-after] slot=133 fields=[(78, None)] given_fields=[(78, None)]`——类的字段与给定值
  的字段**相同**（`78` = 形参 `n` 的 cell），即类的值确实取了**给定**的数组（不再是模板的）；
- 但套件从 257/25 掉到 **118/164**：空槽副本破坏了其它读取路径（recorded/graph 等仍依赖模板值）；
- `any-nested-field` 仍是 `none`：给定值的 `.x` 是**形参 cell**（检查期 undecided），运行期要靠
  形参 clone 填 7 —— 剩下的是 clone/side-apply 这一层，不是 struct unify。

已回退这两个实验（保留 `render.rs` 的诊断修复与探针），基线恢复 257 passed / 25 failed。

## 规则澄清与当前进度（round 26）

用户澄清：**不是“模板不能并”，而是 `@any` 在右侧不能并左侧**（§Any 的方向规则）。

对应实现：`v[k] = e1` 是 raw 写入——per-use 副本本身就是 e1 的副本（开放槽是空 cell）；
`e3 = v[k]` 里给定值 unify 的是副本的**空 cell**，不是模板的 `@any`。所以删掉“再对模板节点做一次
unify”是对的（`eac8af7a`），原因是：那次 unify 会把副本的类并回模板侧，使副本离开函数作用域、
按 §Clone 被引用而不是 clone；而不是“模板不能并”。

当前读数：

- 套件 **262 passed / 21 failed**（原 257/25）；
- 最小链式探针答 `[1, 2]`（原 `raw none`）；
- 嵌套 literal 的四种形态（单独 / 字段内读 / 函数内读 / 经局部读）全部答 7；
- 声明值已定（`{.a 1}` 对 2）仍按 Unify 拒绝。

剩余 21 个失败的样例：`parallel_read_input_buffer` 答 0（应 22）。实测两个 launch 都执行
（fill: leaves=[3,0]；sum: leaves=[3] inputs=1），解码解析出的 payload 是
`ComputeValue(Buffer(…))`（open=false），所以 buffer 找到了但内容为空——下一步是确认它解析的是
**生产者写入的那个 buffer** 还是另一个（如声明侧/输出侧）。

## stamp 假设被排除（round 27）

给 `stamp_node_ran` 加探针，在“有 e1 unify”的失败态跑 `any-nested-field`：

```
[stamp] node=107 op=Some(Index) …
[stamp] node=209 op=Some(Index) value=Array …
[stamp] node=259 op=Some(TypeOperator(Add)) …
```

- **所有 stamp 都落在有 operation 的节点上**（Index / TypeOperator），没有裸 cell 被 stamp；
- 所以“检查期 unify 把形参 cell 标记成已答、挡掉运行期写入”的假设不成立。

当前结论：`v[k] = e1`（模板字段节点，按 SPEC 是 e1）一加入就回到 `none`（两种顺序都试过），
但机制尚未定位；已回退到最好状态 **262 passed / 21 failed**、`any-nested-field: 7`、最小链 `[1, 2]`。
下一步需要从“e1 unify 的 `(Some, Some)` 下钻对该数组类的实际影响”入手（谁的类/哪个成员被改动），
而不是 stamp 或 scope。

## e1 unify 的实测机制（round 28）

对比有无 `v[k] = e1` 两次运行中，形参节点 `83` 的 clone 判定（`DBG_CLONE` 探针）：

| 状态 | `[param]` image | `[gate] node=83 class_inside` |
|---|---|---|
| 无 e1 unify | **215**（clone） | true（所有访问） |
| 有 e1 unify | **83**（引用模板自己的 cell） | **false**（部分访问） |

- `class_root(83) = 83`——**类没有合并**，所以 `class_inside_scope` 的 summary 就是节点自己的 block（`BlockId(2)`）；
- `class_inside` 翻转说明 **同一个节点在不同 apply 里被判成 outside**（即 `ctx.anchor` 不同）；
- 引用发生后，`apply` 把 7 写进模板 cell(83) 的类，而函数体内的读用的是另一个 cell(78)，两者不在一个类 → 读到 None → `none`。

所以 `none` 不是“模板并进实例的类”，而是：**e1 unify 之后，形参在某些 apply 中以其自身的 block(2) 被判为 outside，于是被引用而不是 clone**。
下一步要查的是：为什么 e1 unify 会让同一形参在某个 apply 的 anchor 下变成 outside（anchor/block 的作用域比较），以及引用路径与函数体内读的 cell 为什么不在同一类。

## 形参 gate 的 anchor 实测（round 29）

`DBG_CLONE` 打印 node 83 的每次判定：

```
[gate] node=83 block=BlockId(2) anchor=FunctionId(1) anchor_block=BlockId(2) class_inside=false value=Array
[gate] ... class_inside=true value=None
```

- anchor 与 anchor_block 完全相同，差异只在 `value`：值已成 Array 的访问 → false；值为 None 的访问 → true；
- `node_class_inside_scope` 用的是 **ctx.scope_root**（不是 anchor）：`let Some(scope) = ctx.scope_root else { return false }`；
- 所以 false 来自 **某次 apply 的 `scope_root` 为 None**（无作用域的 pass），而 e1 unify 恰好让形参多进了这样一次 pass。

这解释了引用路径：形参在该 pass 被判 outside → 返回模板自己的 cell(83) → 实参写到 83 的类；
函数体内的读用另一个 cell(78)，不在同一类 → None → `none`。

下一步：查为何 e1 unify 会触发一次 `scope_root=None` 的 pass（以及该 pass 是否做了参数绑定），
而不是继续在 scope/anchor 上找。

已恢复最好状态（无 e1 unify，262 passed / 21 failed）。

## 类作用域摘要的 stale None（round 30）

`[gate]` 探针打印 node 83 的类成员与摘要（有 e1 unify 的失败态）：

```
[gate] node=83 class_inside=false summary=None members=[
  (83,  Some(FunctionId(1)), None),
  (106, Some(FunctionId(1)), Some(FunctionId(1))),
  (119, Some(FunctionId(1)), Some(FunctionId(1))),
  ...]
```

- **所有成员 owner 都是 FunctionId(1)**，按 `refresh_class_summary` 的规则摘要应为 `Some(1)`；
- 但 83（代表）自己存着 **None**（虚拟根，代表“类跨出所有作用域”）→ `class_inside=false` → 引用而不是 clone；
- 之后的 pass 里同类却读到 `Some(1)`（且成员只剩 83），说明摘要被刷新过/类被重建过。

所以 `none` 的直接原因是**类作用域摘要的 stale None**：某个成员在 owner 未设定（None）时先被并进类、而代表就取了这个 None；
`set_node_owner` 会 refresh，但若合并发生在其后且代表是已污染的那个，None 就留在代表上。

尝试过「merge 前刷新两侧」（`merge_class_summaries` 内 refresh），**无效**（仍 none，256/27）——
说明污染点不在该合并路径，或刷新用的成员 owner 当时仍是 None。

下一步应找：哪个成员在 owner=None 时进入了该类（克隆创建时 function 是否为 None），
以及代表为何还是 83 而不是被刷新过的成员。已恢复基线（262/21）。

## e1 unify 合并了哪些节点（round 31）

`[eq]` 探针（`DBG_EQ`，只保留涉及 83/78/53 的合并）：

```
[eq] a=53 b=105 root_a=53  root_b=105 b_owner=Some(1)     ← 模板侧(53, owner=None) 与某使用副本合并
[eq] a=78 b=105 root_a=78  root_b=105 b_owner=Some(1)
[eq] a=83 b=106 root_a=83  root_b=106 b_owner=Some(1)
[eq] a=78 b=118 root_a=78  root_b=118 b_owner=Some(1)
[eq] a=83 b=119 root_a=83  root_b=119 b_owner=Some(1)
[eq] a=134 b=53 root_a=83  root_b=53  b_owner=None        ← 83 的类并进了 owner=None 的 53
```

- `83` = 形参 `n` 的值 cell（`e3` 里给定值的 `.x`）；`78` = **读** `d.a.x` 用的 `.x` 值 cell；
- 给定值 83 合并的是 **106/119/134**（它们随后与 owner=None 的 53 合并 → 83 的类摘要变 None → outside → 引用）；
- **78 与 83 从未合并**（78 分别与 105/118/133 合并）——所以读不到给定值；
- 结论：把 `e1` 取成**模板的字段节点**时，unify 把给定值并到了**别的使用副本**（106/119/134）上，而读用的本次副本（78）没被合并。

也就是说：SPEC 的 `e1` 若指模板字段节点，则实现必须保证每次使用先把自己那份副本（78 所在的）与 e1 关联；
现在 e1 是共享模板节点，unify 走的是模板的类，落不到本次副本上。

已恢复基线（262/21）。

## “别的副本”是什么（round 32）

每个 struct 实例化都会经 `instantiate_requirement_value` 为模板的开放槽生成一份 per-use 副本；
同一个模板在程序里被多处实例化就有多份副本。探针读数（涉及 83/78/53 的合并）：

```
[eq] a=83  b=106   ← 给定值 83（形参 n 的 cell）合并进某份副本的 .x
[eq] a=78  b=105   ← 读 d.a.x 用的 .x 是另一份副本的 .x
[eq] a=78  b=118
[eq] a=83  b=119
[eq] a=78  b=133
[eq] a=83  b=134
[eq] a=134 b=53    ← 之后 83 的类并进 owner=None 的 53（模块级）→ 摘要 None → outside → 引用
```

- `83` = `e3` 里给定值的 `.x`（形参 `n` 的 cell）；`78` = 读 `d.a.x` 解析到的 `.x`；
- 83 与 78 **从未合并**：83 合并的是 106/119/134，78 合并的是 105/118/133；
- 所以“为什么没和本次副本 unify”的直接答案是：**读用的容器（123）与 check_instantiate 里被 unify 的那份副本不是同一个实例化结果**——
  同一模板被实例化了不只一次，读解析到的是另一份。

尚未定论的是：字面量为何产生/解析到两份结果（是 `check_instantiate` 的结果节点与 `value_of(container)` 不同，还是字面量被检查了两次）。
下一步测量：`check_instantiate` 里打印结果节点及其 `.x`，与读的 container(123)/`.x`(78) 对比。

## 读写节点一致、差异在运行期（round 33）

在**有/无 e1 unify** 两种状态下打印同一个例子（`any-nested-field`）的 check 期读数：

```
[inst-result] e=26 value_node=123 items=[57, 54, 83]      ← .x 字段值 = 83（形参 cell）
[read-x]      container=123 third=83 value_node=78        ← 读走 lazy Index(123,2) 的 value
```

**两态完全相同**（container 123、third 83、read 的 value_node 78）。但答案一个 7、一个 `none`。

所以差异不在 check 期的图/节点选择，而在**运行期 83 的类的值**：
有 e1 unify 时 83 的类并入了 owner=None 的 53（模块级），类摘要 None → clone gate 判 outside → 引用；
运行期形参写入落到 template cell 的类上，而 lazy 读最终解析到的 83 的类没有 7 → `none`。

结论：`v[k] = e1`（把模板字段节点与副本做 check_unify）在实现里会下钻到字段 pair 的 **type/attribute 项**，
把字面量侧（函数作用域）的项节点与模板侧（模块级）的项节点并类——这一步才是污染源，而不是“e1 该不该 unify”。
按 §Any 的方向规则，值项不受影响；受影响的是 pair 的 type/attr 项。

下一步（已定方向）：让 e1 的 unify 只比较/填充**字段的值项**，不要把 pair 的 type/attribute 项并类
（类型/属性是常量，判等即可）。这需要在 struct 值的递归 unify 里区分值项与类型/属性项。

## 目标节点选择实验（round 34）

尝试把 e3 的 unify 目标从字段 pair 改为 pair 的 value item（`value_item(field, 0)`），
两种写法都让探针与套件大幅回退（`any-nested-field: expected Table, found raw[...]`，套件 146/137）。

原因：pair 的 value item 与给定值的形状不同（给定值是 4 项 struct 值，value item 是单槽），
当两侧都已定时 Unify 会按位置比较 → 表/结构错配。当前已回退。

这说明：e3 必须与字段 pair 本身 unify（双方都未定时按类合并），
而 pair 的类型/属性项造成的跨作用域并类只能在 **unify 的下钻层**区分值项与类型/属性项，
不能在 checker 的调用点改目标。

## 类型/属性项不能只比较（round 35）

在 `unify_array_items` 里给字段 pair 的类型项和属性项加“只判等、不并类”的 compare-only 分支，两种范围都测了：

| 范围 | 探针 | 套件 |
|---|---|---|
| 类型项 + 属性项 | 全部 refused（`the container's type is not decided here`） | 92/191 |
| 仅属性项 | `any-nested-field: 7` ✓ | 92/191 |

- 类型项的类合并是**必须的**：checker 的类型解析依赖它（否则容器类型判成未定）；
- 属性项的类合并同样被别处依赖（92/191）；
- 两种都回退，基线恢复 262/21。

所以“pollution 来自 pair 的 type/attr 并类”这条不能靠取消合并解决。剩下的可行方向只有 §Clone 一侧：
**undecided 节点若其自身位于 applying function 的作用域内，即使它的类横跨模块，也应 clone 并脱离原类**
（SPEC 的“Clone an undecided node will detached from its original class”）。这需要改 clone gate，
而不是 unify。

## “pollution”的准确定义与最小例子（round 36）

**定义**：一个 equality class 同时含有 **模块级节点**（owner = None）和 **函数作用域节点**（owner = Some(f)）。
它的 `scope_summary` 变成 `None`（“reaches outside every scope”），于是 `class_inside_scope` 对其中任何成员都返回 false，
clone gate 就会 **引用** 而不是 clone。`DBG_SPAN` 探针在 `merge_class_summaries` 里打印这类合并。

**当前代码中它是普遍存在的**（纯语言例子 `T := {.a @any : Int}; f := 'n -> (T{.a n}).a; f 2` 就产生多条）：

```
[span] left=53  right=105 rep=53  owners=(None, Some(1), None)   ← 模块级 + 函数级
[span] left=97  right=38  rep=97  owners=(Some(1), None, Some(1))
...
```

这些多是类型/属性节点，**本身不产生错误答案**——例如下面这个最小例子当前答 **7**：

```lichen
Inner := {.x @any : Int}
T := {.a Inner}
f := 'n -> (T{.a Inner{.x n}}).a.x
f 7
```

**只有在额外那次 e1 unify 存在时**，形参值 cell（83）的类才并进 owner=None 的 53 → summary None → 引用 → 答 `none`。
所以“pollution”在当前代码里是一个**普遍但通常无害**的结构；导致 `none` 的是“**值 cell** 的类跨到了模块级”。

## 回退越权改动（round 37）

我把实例字段从实参裸值改成了类型侧的 pair（`value_items.extend(fields)`），**没有 SPEC 依据**，已回退。
基线恢复 262/21。

按 §Struct 的改写重新对照，实现与 SPEC 至少在这几处不一致，需要先确认编码再动手：

1. `v := @raw[name_table, nominal_id, @raw None, @raw None]` —— `v` 的槽是 **raw None**；
   实现里槽来自 per-use 副本（类型侧 pair）。
2. `e3 = @raw v[slot]` —— **给定值 e3 先与 None 槽 unify**；实现里 e3 与 pair unify。
3. `@raw v[slot] = e1` —— 模板字段值 e1 之后再写入；实现里没有这次写入（或只由副本携带）。
4. 结果 `@raw[@raw v, Struct, ...]` —— 实例表达式的值 = v；实现里结果数组直接是 `[name_table, nominal_id, vals...]`。
5. `let _{.n1 e1, .n2 e2} = s` 里 e1/e2 是 **pair 还是声明值**，决定第 3 步写入的是什么。

## 用户的编码规则：非 raw 即 attribute（round 38）

> “谁告诉你是值的？所有非 raw 都是 attributes”

按这条重读 §Struct：

```
s := @raw[@raw[name_table, nominal_id, e1, e2], Struct, ...]
```

- 外层表达式的 **item 0 是 raw**（`@raw[names, id, e1, e2]`，值数组），**其余项都是 attributes**（`Struct`、……）；
- 字段表达式 `{.x @any : Int}` = `@raw[@raw @any, Int]` —— raw 值 `@any` + attribute `Int`；
- 所以实现里的 `[value, type]` **不是**“值 + 类型”，而是 **raw 值 + attribute**（`EXPR_TYPE_SLOT = 1` 就是第一条 attribute）；
- 实例化：`v := @raw[names, id, None, None]`（值数组），`e3 = v[k]`、`v[k] = e1`，结果 = `@raw[@raw v, Struct, ...]`。

据此我此前“结构字段是值、读就是元素本身”的读法**错误**，`7abf86f4` 已 revert（`b6ef5c48`），基线恢复 262/21。
读字段 = 取字段表达式的 **raw 值**（item 0），实现原有的 pair 语义是对的。

## 拒绝诊断的三重复（round 39）

`an_output_position_that_is_not_a_buffer_is_refused` 期望 **1** 条 `expected Struct, found Int`，实际 **3** 条。
本轮定位：

- `DBG_DIAG` 探针显示**日记只有一条**：`seq=0 loc={expr:78, path:One(Type)} kind=Annotation errors=0..1`；
- 在诊断装配处把“每条 entry 只渲染第一个 error”改掉也**无效**（仍 3 条）；
- 在 `record_unify` 里按 `(expr, kind)` 去重同样**无效**。

所以三条消息**不是**来自 checker 的日记渲染；需要继续查 `evaluate_raw` 的诊断装配（可能 store/registry 跨模块积累了同一诊断，
或 prelude 被编译了多次）。两处实验均已回退，基线 262/21。

本轮已提交：`8fa1de17`（raw 实例字段读作自身；纯语言形态与最小链都正确，套件计数不变）。

## 包装器 kernel 的输入：host 对、lowering 错（round 40）

`a_struct_parameter_kernel_runs_through_the_signature_carrying_wrapper`（应是 22，实得 0）的读数：

```
[in]  node=14878 payload=… words=["10", "11", "12"]      ← launch 收集输入时解析出的是生产者 buffer ✓
[out] results=[[0, 0, 0, 0]]                              ← kernel 的输出全 0
```

- **host 侧**（`run_parallel_kernel` 之前的输入收集，compute.rs:1588 的 `recorded_buffer_payload`）解析正确：
  第二个 launch 的输入就是生产者写入的 [10,11,12]；
- **kernel 内部**读 `k.in.b` 时走 lowering（compute.rs:~7168 的
  `recorded_input_node` / `value_at_path`），解析到的是**另一个（空的）buffer** → 读 0 → 写 0*2=0 → 输出 0。

所以这 21 个失败里，这类“包装器 kernel”的根因是 **lowering 对 `k.in.b`（签名结构里的 Buf 字段）的解析**，
而不是 launch 的输入收集。两条解析路径（host 的 `recorded_buffer_payload` 与 lowering 的
`recorded_input_node`/`value_at_path`）用不同的规则，前者对、后者错。

下一步：以 host 侧解析到的节点（14878）为准，让 lowering 的输入路径也走到同一节点
（对比 `recorded_input_node`/`value_at_path` 与 `recorded_buffer_payload` 的解析差异）。

## 两个 launch 的链：outcome buffer ≠ write 的 buffer（round 41）

在 lowering 的 `roles.inputs` 解析处加探针（`DBG_LOW`），跑包装器测试：**没有任何 `[lower-in]` 输出**——
说明 kernel 内部的 `k.in.b` 读**不走**这里（roles.inputs），之前的“lowering 解析错”的定位需要修正。

更早的读数（`DBG_IN`）显示：

```
[out] results=[[10, 0, 0, 0]]   ← fill launch 的 host outcome
[in]  words=["10", "11", "12"]  ← 第二个 launch 的输入（生产者 buffer，三条都在）
```

即：**fill 的 write 把 10/11/12 写进了 `inbuf.z` 的 buffer，而 launch 的 host outcome 是另一个
只含 [10,0,0,0] 的 buffer**。所以问题在 launch 的 **output 绑定**（outcome 收集的 buffer 与 kernel
实际写的 buffer 不一致），而不是输入解析。

下一步：查 `run_parallel_kernel` 的 `outputs` 分配与 ABI 的 write ordinal 绑定，
确认 launch 的输出应绑定到 kernel 实际写入的那个 buffer。

## 第二个 kernel 的读被读了两次，第二次用第一次的值当地址（round 42）

`DBG_W` 探针（`ParallelState` 的 read/write）跑包装器测试：

```
[write] position=0 index=0 value=10      ← fill 的写：0,1,2 -> 10,11,12 ✓
[write] position=0 index=1 value=11
[write] position=0 index=2 value=12
[read]  position=0 index=0 inputs=1
[read]  position=0 index=10 inputs=1     ← 第二次读的地址 = 第一次读到的值(10)
[write] position=0 index=0 value=0       ← 写 0（第二次读越界得 0）
[read]  position=0 index=1 inputs=1
[read]  position=0 index=11 inputs=1
[write] position=0 index=1 value=0
...
```

- fill launch 的写入**正确**（[10,11,12]）；
- 第二个 kernel 每个 lane 读**两次**：先 `read(input, i)`（0/1/2 ✓），再 `read(input, v)`（10/11/12 越界 → 0），
  写出的值是第二次读的结果（0）。正确应为 `v * 2` 用第一次读的 `v`。

所以缺陷在 kernel 内部对 `v * 2` 的 lowering：`v` 被解析成了一个“以 `v` 为地址的 read”，多读了一次并丢掉原值。
下一步：查 `Mul`/write 的 `.value` 操作数解析（`Lower`/`parameter_path`/`supplied`），
让 `v * 2` 使用第一次 read 的结果节点，而不是再造一个 read。

## 两次 BufferReadCall 的发射（round 43）

`DBG_BREAD` 探针（`body.rs` 的 `buffer_read` 发射处）跑包装器测试，得到**两次发射**：

```
[bread] pos=0 index_value=ValueId(1)  index_node=4468  index_node_value=Array
[bread] pos=0 index_value=ValueId(6)  index_node=13580 index_node_value=None
```

- 第一次读的 index 节点 = 4468（值是 Array，`compute.range` 的 range 对象）；
- 第二次读的 index 节点 = 13580（无值），运行期求值成 10（= 第一次读的结果）——即这次读是
  `read(input, v)`，`v` 是上一次读的结果；
- 运行期探针（round 42）证实：每 lane 读两次（先 i、后 v），写出的值是第二次读的结果。

所以缺陷是 **kernel 体内同一个 `compute.read` 被发射了两次**（一次 index=i 正确，一次 index=v）。
下一次要查的是 `value()` 对 `v * 2` 中 `v` 的解析路径：它应复用第一次读的 IR 值，
而不是再发射一次读（`supplied`/`lower_definition` 都可能重新走读节点）。

另：用户澄清 **unify/merge 不触发 operation**，只有 evalu**ation** 才触发——后续分析不要把类合并当作求值。

## 两次发射是**两个不同的 read 节点**（round 44）

`DBG_BREAD`（打印读的 operand 数组）跑包装器测试：

```
[bread] operand=5214  op_items=[5212, 5213, 4723, 4468]  index_node=4468   ← 源 read（4 项）
[bread] operand=13516 op_items=[13517, 13580]            index_node=13580  ← 另一个 read（2 项，index=v）
```

- 第一次：源 read 的 operand 数组有 4 项（buffer、index、…attribute）；
- 第二次：operand 数组只有 2 项 `[buffer, v]`，index = 13580 = 第一次读的结果 `v`。

所以不是“同一节点发射两次”，而是**体内存在两个 read 节点**：源 read（5214）和一个
**index 为 `v` 的副本 read（13516）**——后者是 per-use 拷贝/`field_sources` 解析产生的。
`v * 2` 的 lowering 经 `supplied`/`lower_definition` 解析到 13516，于是又发射了一次读。

下一步：查 13516 的创建处（`instantiate_requirement` 对 read 结果的拷贝？`field_sources` 的边？），
让 `v` 的解析复用它自己的值（第一次读的结果节点），而不是这个自引用的副本 read。

## 副本 read 的 index 来自 prelude 的 `x.at`（round 45）

prelude 的定义：

```lichen
Read := 'p = BufParam -> {.from (Buf p) # warp, .at @any : Int}
read := 'x = (Read _) -> $read(x.from, x.at)
```

两次发射：

```
[bread] operand=5214  op_items=[5212, 5213, 4723, 4468]  ← 模板侧（4 项）
[bread] operand=13516 op_items=[13517, 13580]            index=v  ← 克隆侧（2 项）
```

克隆侧的 `$read(x.from, x.at)` 里 **`x.at` 的值节点就是用户体内的 `v`**（第一次读的结果）：
运行期 index=10（= read 的结果）证实 `.at` 与 `v` 落在了同一个类。

也就是说：用户 `v := read(Read{.from k.in.b, .at i})` 之后，`v` 的节点被并进了 `Read` 实例的
`.at` 槽（可能经 `: Int` 注解的 unify 或 struct 实例化），于是 clone 出的 `x.at` 用的是 `v` 的值。

下一步：查这条合并的来处（`v` 与 `.at` 的 equality 建立点），确认是 `check_unify` 的哪一次
（注解 / `e3` / 类型槽）把读结果并进了 `.at`。

## 首个 read 的 operand 是用户结构值，克隆 read 的 .at = v（round 46）

`DBG_BREAD` 的 operand 数组再看一次：

```
[bread] operand=5214  op_items=[5212, 5213, 4723, 4468]   ← 4 项 = 用户 `(Read _){...}` 的结构值
[bread] operand=13516 op_items=[13517, 13580]             ← 2 项 = prelude 克隆的 [x.from, x.at]
```

- 第一条 read 的 operand 是**用户的结构值** `[names, id, from, at]`（buffer_read 会剥出 from/at）；
- 第二条（prelude `read` 的克隆）的 `.at` = 13580 = 调用结果 `v`。

因为结构值是在调用*之前*构造的，`v` 不可能作为它的字段出现——所以 `.at` 槽是在**调用之后**经类合并拿到 `v` 的
（参数 `x` 的字段类与调用结果的类被并在一起；这也符合运行期“第二次读的 index = 第一次读的值”）。

下一步：在参数绑定（`unify_parameter_slots`/`bind_field_sources`）处确认哪次 unify 把 `x.at` 与 `v` 并类，
并按 SPEC 让参数绑定只复制槽值、不把结果并回参数字段。

## 克隆的 .at 不是 field_sources 的候选（round 47）

`DBG_BIND` 打印 `bind_field_sources` 的所有插入（candidate、supplied、path）：

```
[bind] candidate=12520 supplied=7570  path=[0]    value_projection=true
[bind] candidate=12453 supplied=12692 path=[2, 0] value_projection=true
[bind] candidate=13614 supplied=13435 path=[0]    value_projection=true
...
```

- **13580（克隆的 `.at`）不在任何绑定里**——它不是经 `field_sources` 拿到值的；
- 所以 13580 运行期等于 `v` 只能来自**类合并**：克隆的 `.at` 与调用结果 `v` 落在同一个 class。

结合第一条 read 的 index 是 **range 对象**（4468，Array 值，运行期求值成 lane 0/1/2）而不是 `i` 的 cell，
`i`/`v` 的类合并（或 `.at` 与 `v` 的合并）是下一个要定位的点：
在 `add_equality` 里筛出“经典 read 结果节点”的合并，找出是哪一次 unify（注解 `: Int` / 参数绑定 / 返回类型）建立的。

## alias_read 把 x.at 与 read 结果并类（round 48）

`DBG_ALIAS` 探针抓住了关键合并的调用者：

```
[alias] reader=6018 (LowOperator(Index))  target=13515 (ComputeOperator(Read))
```

- `alias_read`（Index/读的求值别名）把 `x.at`（6018）与 **read 操作节点 13515** 并成一类；
- target 是“容器在当前 index 位置的项”——说明容器（参数 x 的实例）的 `.at` 项所在的类里
  已经有 read 操作节点（13515），即 **`.at` 的 cell 与 read 结果同属一类**。

`[eq2]` 另外两条相关合并：`13491(Array) ↔ 4468(Array, Apply)`（克隆的 index = 模板的 range）、
`5617(Apply) ↔ 13514(Array)`（用户的 read 结果与一个数组）。

所有“先 read 再写”的 kernel（`parallel_multi_output_collects_each_output` 得 [0,0,0] 应 [20,22,24]、
`parallel_read_input_buffer`、包装器等）都共享这一条：**read 的结果与它自己的 `.at` 参数落在同一类**，
于是第二次读用第一次读的值当地址。下一个落点：找出把 `.at` cell 与 read 结果并起来的那次 unify
（不是 alias_read，它只是暴露者）。

## x.at 读到了 read 调用自己的结果（round 49）

`DBG_IDX`（Index 求值）抓到的链：

```
[idx] node=6018  container=5617 (read 的 apply)  index=0 len=5  element=13515 (read 操作)
[idx] node=6061  container=6018 (x.at)           index=0 len=5  element=6019
```

- prelude 的 `x.at`（6018）的容器 = **用户 read 调用自己的 apply 节点 5617**，不是实参结构；
- 5617 的值是一个 len=5 的数组，其 item 0 = read 操作节点（13515）。

即：prelude 的参数 `x` 的 image 落到了 **read 调用自己的结果**上（循环绑定），而不是实参数值结构。
SPEC §Clone 说“unify the argument with the parameter”；现在的实现把参数与 apply 结果并到了一起。
下一步/或需用户指点：参数绑定处（`apply` 的 `unify_parameter_slots`/`parameter_image`/clone）哪里
把参数与 apply 结果建立了 equality。

## 用户裁定：参数只与实参绑定；实测实参解析是对的（round 50）

用户裁定：**参数只与实参绑定，绝不与 apply 结果同类**。

对照实现：apply 的 operand 数组 = `[function_value, value_argument_expr, c, callee_expr]`，
`argument = operands[1]`，而 `value_argument_expr = self.value_of(argument)`（lambda.rs:475）✓。
`[idx] node=6018 container=5617` 的 5617 是**结构字面量的 apply 值**（带 Array 值，len=5）——即实参本身 ✓，
不是 apply 的结果。所以参数绑定这条**没有错**，我上一轮的怀疑不成立。

真正的异常在下一层：`x.at`（6018）的 Index 求值用了 **index=0**（应为 name_table["at"]+2），
读到的元素是 13515（read 操作）。所以下一步是查 `x.at` 这个 Index 的**下标求值**为什么是 0
（名字表 TableGet 的键/位置，或 `+2` 缺失）。

## `.at` 的下标是常量 0（round 51）

`DBG_IDX` 的 subscript 探针：

```
[sub] node=6061 subscript_node=39 (no operation)
[sub] node=6018 subscript_node=39 (no operation)
```

- 两个 Index 的下标都是**同一个常量节点 39**（无 operation），求值 = 0；
- 而按结构编码，`x.at` 的读应是 `name_table["at"] + 2`（非常量），或常量位置 `at + 2`；
- 下标 0 使 `x.at` 读到容器数组的**第 0 项**（名字表/或 read 操作），而不是 `.at` 字段。

所以问题在 checker 生成 `x.at` 读时选了**常量 0**：可能是 `position = Some(0)` 后 `at + STRUCT_VALUE_FIELD_BASE`
未生效，或走了 value-slot 读路径（`Index(container, 0)`）。
下一步：在 `check_named_field`/`value_slot_read` 处确认给 `x.at` 生成的 subscript 常量应是多少（`at + 2`），
以及容器值为何是结构 apply 值。

## `$read` 的 apply 答成了实参结构（round 52）

`[inst-res]` 显示结构实例化正常（用户的 Read 结构 = [names, id, from=4723, at=4468] ✓，e=161）。
而 `[eq2]` 显示 read 调用的 apply（5617）的值是 **Array(len 5)**、`[idx]` 里 `x.at`（6018）的容器是 5617、
其下标是常量 0（节点 39）。

即：**`compute.read` 的 apply 把自己的值答成了实参结构**（而不是读出的元素 10），于是：
- `x.at` 读容器（= apply 值 = 结构数组）的第 0 项 → 读到 read 操作节点；
- 第二次读的地址 = 第一次读的结果。

所以缺陷不在 `x.at` 的编码本身，而在 **apply 的 answer**：`$read`/`read` 的 apply 结果被写成了实参（结构数组）。
下一步：在 apply 的 answer 写入处（`write_node_answer`/`function_apply` 的 result 绑定）查为什么
read 的 apply 拿到了实参的值（很可能是 `c` cell 与参数/实参同类）。

## 参数槽位绑定实验（round 53）

`unify_parameter_slots` 的条件是 `parameter > 2 || supplied > 2`：当实参是结构值（数组长度 > 2）时，
它会把**参数的 value cell 与结构数组的第 0 项**（名字表）并按槽 unify——这看起来正是
“参数 image 落到类型 apply”的候选。

实验：把条件改为只看 `parameter > 2`（结构实参不再触发槽位绑定），
- 包装器测试仍得 0，套件仍 262/21——**无变化**；
- 说明 read 的参数绑定没走这条路径（或不是该路径造成的）。

已回退该实验。另一个读数：`[ans]` 显示多个 Apply 被答成 Array（prelude 的类型函数本就如此），
所以“apply 答成数组”里只有 read 的 *值* 调用是异常的，需要按函数身份区分（值函数 vs 类型函数）再继续查。

## 核心语义套件全绿（round 54）

跑核心 crate 的测试（`target/test_highlevel.bat`，vcvars64 + `cargo test -p <crate>`）：

| 套件 | 结果 |
|---|---|
| **lichen-highlevel** | **136 passed, 0 failed** ✓ |
| lichen-language | 68 passed, **1 failed**（`a_set_and_a_table_value_render_their_own_syntax`） |

- 核心语义（struct/clone/any/类型系统）在 highlevel 的 136 个测试上**全部通过**；
- lichen-language 的唯一失败是**解析**问题（`set(1, 2)` 源码报 unclosed '('），与 compute 无关，
  且该测试文件最后一次改动在本会话之前（`e44849fd`），属既有问题；
- 剩余 21 个失败都在 **lichen-compute** 套件（compute 插件/launch 路径）。

结论：SPEC 的核心实现已由 highlevel 套件验证；compute 插件的 21 个失败是独立的下一阶段工作。

### 该语言测试失败是既有的

`git show 4d2cb04e:crates/lichen-language/src/tests/render_tests.rs` 显示同样的
`set(1, 2)` 测试在**基线**就存在；而 SPEC §Set 的语法是 `set(t)(e1, e2)`，
`set(1, 2)` 与解析器的集合构造语法不一致。所以它是**既有失败**（基线时语言套件未被跑过），
不是本会话引入。修它要么改测试（需用户许可），要么让解析器接受旧写法——待用户定。

## 突破：e3 只对已定的供给值做 unify（round 55）

**bisect 结果**（`git checkout <commit> -- crates/` + 构建 + 跑 compute 套件，代价 ~35s/次）：

| commit | 结果 |
|---|---|
| 基线 `4d2cb04e` | **279 passed, 0 failed**（绿） |
| `670a65e8`（copy fix） | 276 / 3（`buf_element_class_recorded::*`） |
| `566f37e8`（WIP：无条件 e3 unify + supplied 追踪） | 252 / 27 |
| 之后逐步修回到 HEAD 的 262 / 21 |

`566f37e8` 把 `check_instantiate` 的 e3 unify 从“仅当供给值已定”改成**无条件**。
恢复“仅当已定”（`c18ef097`）后：

```
compute 套件：280 passed, 3 failed   ← 从 262/21 恢复
lichen-highlevel：136 passed, 0 failed ✓
lichen-language：68 passed, 1 failed（既有 set 解析测试）
```

原理：无条件 unify 把**未定的供给值**的类并进槽的类，等于把每个 per-use 副本的类挪出函数作用域，
后续 clone 便引用而非克隆，compute 链全断。仅当已定时 unify，副本保持自己的类。

## 剩余 3 个

`buf_element_class_recorded::*`（float 元素类）——正是 copy fix（`670a65e8`）引入的 3 个；
检查期报 `compute.graph: argument 0 of this dispatch is not a buffer`，疑与 copy fix 对
frozen type half 的拷贝有关。

## 剩余 3 个的解析点（round 56）

`DBG_GIN`（graph 输入解析处）跑 `buf_element_class_recorded::*`：

```
[gin] position=0 path=[1, 0] found=Some(Dynamic(NodeId(5100v1)))
```

- 路径 `[1, 0]` = 参数结构第 1 个字段（`.I`）的值槽；
- 找到 5100，但 `recorded_buffer_payload(5100)` 返回 None（`buf_payload` 对 5100 无值）→ 报 “argument 0 is not a buffer”；
- 即 5100 是一个**未定/未连接**的 cell，`recorded_applied_projection` 也没能把它解析到 apply 的投影。

这 3 个正是 copy fix（`670a65e8`）引入（基线绿）。下一步：查该副本 cell 的 `field_sources`/投影链路，
让 graph 的参数解析沿副本边走到实参的 buffer（与 host 侧解析一致）。

## round 57 小结

- 240/3 的 3 个失败仍指向 graph 参数解析到“无 payload 的副本 cell（5100）”；
- 已让 `recorded_buffer_payload` 在**无 payload** 时也沿 `field_source_of` 追供应商（`1b9459aa`），
  但 5100 没有 `field_sources` 条目，仍解析不到——套件保持 280/3；
- 下一步需要查：`Arg{.n a.n, .I a.in}` 的 `.I` 字段副本（5100）为何没有 `field_sources` 边，
  或者 graph 的 `cfg` 值路径（`value_at_path`）取到的 5100 与 check 期写入的 required 节点不一致。
