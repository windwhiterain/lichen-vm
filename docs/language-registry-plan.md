# Language 层 registry 支持 / Package Manager 原型 — 实施计划

**状态:** v1 已实现(2026-08-30);**传递依赖已实现**(2026-08-31,见 §8)——包可以 `@import` 其它包,freeze 的 dependency-free 断言放宽为 referenced-keys 检查
**上游依赖:** `docs/static-module-deps-plan.md`（lowlevel registry / static module，已实现并有测试）
**目标:** 在 `crates/lichen-language` 层打通 registry 特性：
1. 提供一个**预处理器**，识别专用 `@import "path" as name` 指令，加载包并解析导入；
2. 预处理器与语言 parser/lexer 互相不可见：parser 永远只看到干净的 lichen 源文件；
3. 通过一个最小的 package store 原型，把多个源文件编译、冻结（freeze）到共享 registry，并在导入方 *in-place* 使用静态模块（不拷贝 payload）。

---

## 1. 现状盘点

已完成（lowlevel，`crates/lichen-lowlevel`）：

- `ModuleKey` / `StaticNodeId` / `AnyNodeId` / `AnyFunctionId` / `AnyHandle` 等 key-carrying refs。
- `Registry<P>`：`new_module` / `freeze` / `get`，`Module::freeze(&source)`。
- `StaticModule::from_module`：动态模块 → 静态模块（`node_map` 在内部生成后丢弃）。
- `Module::static_read` / `node_value` / `materialize_leaf` / `as_dynamic` / `static_function_apply`。
- `evaluate_node`、`unify`、GC、assert 的 static arm 已经具备。

缺口（highlevel / language）：

- `crates/lichen-highlevel/src/ir.rs`：IR 无法表达“一个来自静态模块的引用”。
- `crates/lichen-highlevel/src/checker.rs`：`Checker::build` 总是 `Module::new()`（私有 registry）；`dyn_node` 对 `AnyNodeId::Static` 直接 `unreachable!`，`kind_marker_is` / `is_function_type` / `is_struct_type` / `is_indexable_type` / `check_index` / `check_instantiate` / `wrap_shallow` 均无法处理 static refs。
- `crates/lichen-highlevel/src/diagnostic.rs`：`record_span`、type printer 的 `dyn_node` 同样 panic；`EvalError.index` / `ApplyError.function` 已识别 static 的 fallback，但渲染仍需 read-only 的 static arm。
- `crates/lichen-highlevel/src/program.rs`：`TypeOperator::IndexTypeDispatch` 和二元运算仍用 `module.nodes[dyn_node(...)]` 读操作数，会 panic。
- `crates/lichen-language`：没有预处理器；`compile` / `run` 只接受单源文件；`render.rs` 的 `dyn_node` 同样 panic。

---

## 2. 设计总览

### 2.1 导入方：预处理器 → language parser

```
raw source（含预处理器指令 @import "path" as name）
        │
        ▼
Preprocessor（专用格式，独立模块）
        │  - 逐行扫描，只识别 @import 行
        │  - 通过 PackageStore 加载并 freeze 包
        │  - 生成 ResolvedImport { name, span, value: StaticNodeId, ty: StaticNodeId }
        │  - 删除 @import 行（用空行占位，保持行号/列号不变）
        ▼
cleaned source（纯 lichen 源文件，parser 看不到任何 import 痕迹）
        │
        ▼
lex → parse → compile（现有链路不变）
        │  Compiler 接收 preprocessor 传来的 imports，作为第一帧 scope：
        │  name -> ExprKind::Static { value, ty }
        ▼
IR（含 Static 节点）
        │  Checker::build_in(ir, store.registry.clone())   ← 新入口
        ▼
Build 的 module 绑定共享 registry；run 与现有路径一致
```

要点：

- **`@import` 不属于 language 语法**。lexer/parser/AST 完全不新增 token、关键字、`Stmt` variant。
- **预处理器与 parser 互相不可见**：预处理器只做行级文本处理，不解析 lichen；parser 只解析 cleaned source，不识别 `@import`。
- **预处理器输出两样东西**：`cleaned source` 和 `resolved imports`。后者在编译前端作为初始 scope 帧注入，导入名绑定到 `ExprKind::Static`。
- **包自身的 `@import` 同样解析**（2026-08-31 起支持传递依赖）：包加载时先递归解析它自己的 import——每个依赖先加载、先 freeze 进共享 registry——然后包自身对着共享 registry 编译。加载栈检测循环导入并诊断。

### 2.2 包导出

```
package source (普通 lichen 源文件，可含 @import)
        │  Preprocessor（递归解析包自身的 import；依赖先 freeze 进共享 registry）
        ▼
cleaned source + imports
        │  lex → parse → compile_with_imports_in（绑定共享 registry）
        ▼
IR（含 Static 节点，指向已 freeze 的依赖）
        │  Checker::build_in(ir, 共享 registry)
        ▼
Build { module, root_val, root_ty, ... }
        │  deep-evaluate root_val / root_ty
        ▼
store.registry.write().freeze_mapped(&build.module)  →  { key, node_map }
        │  （值里的依赖 refs 原样保留——绝对 key，见 §8）
        │  package 的 value = StaticNodeId { module: key, index: node_map[root_term] }
        ▼
PackageHandle { key, export: StaticNodeId }
```

- **包的唯一导出 = 包源文件最后一个 expr 的 value**（`Build::root_val` / `Build::root_ty`）。包文件不需要、也不提供 `export` 关键字。
- 包文件内仍然可以有任意 binding，但它们不会被导出，只用于计算最终的 value。

### 2.3 导入绑定在 IR 中的表示

- **导入 = 一个 IR 叶子 `ExprKind::Static`**，checker 将其编译为 `[materialize(value), materialize(ty)]` pair。值/类型 payload 仍是静态模块的共享 arena，不深拷贝。
- **static-aware 的 `dyn_node` 替换**：highlevel/language 的检查、诊断、渲染全部改为 read-only 解析（或 checker 中按需 `materialize_leaf`），不再假设图中只有 `AnyNodeId::Dynamic`。

---

## 3. 分阶段实施

### Phase 1 — lowlevel 补充：暴露 freeze 映射

文件：`crates/lichen-lowlevel/src/lib.rs`、`src/static_module.rs`。

1. 增加 `StaticModule::from_module_mapped(module, key) -> (StaticModule<P>, HashMap<NodeId, LocalNodeId>)`；现有 `from_module` 变成其包装（丢弃 map）。
2. 增加：
   ```rust
   pub struct Freeze<P: Program> {
       pub key: ModuleKey,
       pub node_map: HashMap<NodeId, LocalNodeId>,
   }
   ```
3. `Registry::freeze_mapped(&mut self, module: &Module<P>) -> Freeze<P>`：
   - 逻辑与 `freeze` 相同（`try_insert_with_key` 分配 key → `from_module_mapped`）。
   - `freeze` 变为 `freeze_mapped().key`（保持现有测试兼容）。
4. `Module::freeze_mapped(&mut self, source: &Module<P>) -> Freeze<P>` 便捷方法。

测试：`crates/lichen-lowlevel/tests/basic/static_module.rs` 增加
`freeze_mapped_returns_consistent_node_indices`：freeze 后，对若干已知 `NodeId`，`node_map` 给出的 `LocalNodeId` 与 `module.nodes[index].value` 的 rewrite 一致（例如把 `NodeId` 放进一个数组再 freeze，检查 item ref 变成 `StaticNodeId { key, index: node_map[&node] }`）。

### Phase 2 — highlevel static seam

#### 2a. IR

文件：`crates/lichen-highlevel/src/ir.rs`。

增加 variant：

```rust
pub enum ExprKind<V> {
    // ...
    /// 来自静态模块的导入：`value` / `ty` 是包导出 value/type 节点的 static ref。
    /// checker 把它们 materialize 成叶子后重新组成 pair
    /// `[materialize(value), materialize(ty)]`。
    Static { value: lichen_lowlevel::StaticNodeId, ty: lichen_lowlevel::StaticNodeId },
}
```

（或命名为 `Import`；`Static` 更直接。）

#### 2b. Checker 构建入口

文件：`crates/lichen-highlevel/src/checker.rs`。

1. 抽出 `Checker::build_with(ir, module)` 或 `Checker::build_in(ir, registry: Arc<RwLock<Registry<HighProgram<V>>>>)`：
   - `build` 保持原签名，内部创建 `Module::new()` 后调用新入口。
   - 新入口只替换 module 的来源；`apply_depth_limit` / `apply_total_limit` 等初始化逻辑保持不变。
2. 增加 `check_term` 对 `ExprKind::Static` 的分支：
   - `let value_node = self.module.materialize_leaf(value, self.current_block);`
   - `let ty_node = self.module.materialize_leaf(ty, self.current_block);`
   - `let pair = self.pair_of(value_node, ty_node);`
   - 填充 `term[e]` / `val[e]` / `ty[e]`。

#### 2c. checker 中的 static-aware 解析

目标：**所有从数组 item 读 `AnyNodeId` 再 `dyn_node` 的地方，要么按需 materialize，要么用只读 helper**。

具体替换点（`grep dyn_node crates/lichen-highlevel/src/checker.rs`）：

| 位置 | 处理 |
|---|---|
| `kind_marker_is` | 用 `module.node_value(items[0].node)` 读 head；`is_universe` 需要识别 materialized 静态 universe（见下） |
| `is_function_type` / `is_struct_type` / `is_indexable_type` | 改为接受/解析 `AnyNodeId`，内部走 `module.node_value` |
| `check_index` 的 tuple/struct 形状提取 | `module.array_items` 读 static 数组后，`items[0].node` 可能是 `Static`；按需 `materialize_leaf` 得到动态 NodeId 供后续 `Index` 构造 |
| `check_instantiate` 的 `shape = dyn_node(items[0].node)` | 同样按需 materialize |
| `wrap_shallow` 的 pair 下降 | 对 static item 停止下降（静态子树不可变，shallow 包装无意义）或 materialize 后处理 |
| `install_constants` / 数组构造 | 继续用 `AnyNodeId::Dynamic`，不涉及 static |

`is_universe` 的 static 兼容：
- 动态节点保持现有 `equality_representative` 比较。
- 对于 materialize 出来的静态 universe 叶子，直接比较内容：`array_items` 为 2 元、head 为 `type_marker`、tail 与自身成环（`tail` 是 `AnyNodeId::Static`，其 `static_read` 的值与当前数组 handle 相等）。
- 更稳妥的做法是给 lowlevel 增加 `Module::value_is_universe(&self, value: P::Value) -> bool`，checker/language 共用。

#### 2d. `program.rs` 操作符读值

文件：`crates/lichen-highlevel/src/program.rs`。

- 将 `module.nodes[dyn_node(kind_items[0].node)].value` 之类的读取全部改为 `module.node_value(any_id)`。
- `IndexTypeDispatch` 的 struct 内层 tag 同样用 `node_value` 只读解析。
- 删除 free `dyn_node`（或保留仅供动态路径的 helper，但操作符必须不再假设 dynamic）。

#### 2e. 诊断与渲染（highlevel + language）

文件：
- `crates/lichen-highlevel/src/diagnostic.rs`
- `crates/lichen-language/src/render.rs`

原则：诊断/渲染只读，不 materialize。

1. 增加 read-only helper：
   ```rust
   fn any_value<V: ValueType>(module: &Module<HighProgram<V>>, id: AnyNodeId) -> Option<V> {
       module.node_value(id)
   }
   fn any_items<V: ValueType>(module: &Module<HighProgram<V>>, id: AnyNodeId) -> Option<&'static [ArrayItem]> {
       any_value(...).and_then(...as_enum...LowValue::Array...)
   }
   ```
2. `diagnostic.rs`：
   - `record_span`：`AnyNodeId::Static` 直接跳过（static ref 无 importer span）。
   - type printer 的 `dyn_node(items[..].node)` 改为 `any_node` 递归：`Static(sref)` 读取 `module.static_read(sref)` 并按相同结构
打印，动态则走原 `node()`。
   - `EvalError.index` / `ApplyError.function` 的 static arm 已有 fallback，补充测试确认不 panic。
3. `render.rs`：
   - `TypePrinter` / `ValuePrinter` 中所有 `dyn_node(item.node)` 替换为 `any_node(item.node)`。
   - `fields`、`elements`、`instance` 等函数同步改为能打印 static 值（同一套递归打印逻辑，入口是 `AnyNodeId`）。
   - `is_universe` / `kind_is_struct` 等 helper 使用只读 `node_value` 并兼容 static。

### Phase 3 — 预处理器（import 专用格式）

新文件：`crates/lichen-language/src/preprocess.rs`。

**lexer / parser / AST 完全不改**：没有 `import` 关键字，没有字符串 token，没有 `Stmt::Import`。

#### 3a. 专用格式

预处理器只识别一种行指令：

```text
@import "path.lichen" as name
```

规则：
- `@import` 必须在行首（允许前导空格/tab）。
- 路径为双引号字符串，转义从简（v1 足够）。
- `as name` 必须有，`name` 是普通 lichen name 的字符集合，但由预处理器校验。
- 每行一条指令；其余文本一律原样透传。
- 指令行在 cleaned source 中替换为等行数的空行，保证后续 parser 报错的行号/列号与 raw source 一致。

#### 3b. 预处理器接口

```rust
pub struct Preprocessed {
    /// 删除 @import 行后的 lichen 源文件（行号保持）。
    pub source: String,
    /// 已解析的导入绑定，交给 compile 作为初始 scope。
    pub imports: Vec<ResolvedImport>,
}

pub struct ResolvedImport {
    pub name: String,
    pub span: Span,               // 原始指令的 (line, col)
    pub value: StaticNodeId,      // package root_val 的 static ref
    pub ty: StaticNodeId,         // package root_ty 的 static ref
}
```

- `preprocess(raw: &str, base: Option<&Path>, store: &mut PackageStore) -> (Preprocessed, Vec<Diag>)`。
- 错误累积：包路径不存在、包编译失败、`as` 缺失、`as` 后不是合法 name、循环导入（2026-08-31 起传递依赖已支持，环由加载栈诊断；见 §8）。
- 诊断建议新增 `Stage::Preprocess`（或暂时并入 `Stage::Resolve`）。

#### 3c. 预处理器与 parser 的边界

- 预处理器**不调用** lexer/parser；它只做行级文本扫描。
- lexer/parser**不识别** `@import`；如果 cleaned source 中仍残留 `@import`，lexer 会按非法字符报错，这属于预处理器 bug，测试明确覆盖“预处理器输出中不存在 `@import` 行”。
- `docs/language-spec.md` 只写 language 语法；`@import` 写在预处理器/包管理器的独立文档段落，不进入 language grammar。

### Phase 4 — compile 前端与 Package store

#### 4a. compile 前端接收 imports

文件：`crates/lichen-language/src/compile.rs`。

1. `Compiler` 增加一个 `initial_imports: Vec<ResolvedImport>` 输入。
2. 在现有 block-wide binding pre-pass **之前**，为每个 `ResolvedImport` 分配一个 `ExprKind::Static { value, ty }`，插入第一帧 scope（`name -> ExprId`）。
3. 后续流程完全不变：block-wide bindings 是第二帧，允许本地 binding shadow import 名。
4. 普通 `compile(source)` 保持兼容：`initial_imports` 为空，行为与现在一致。

#### 4b. PackageStore

新文件：`crates/lichen-language/src/package.rs`。

```rust
pub struct PackageStore {
    registry: Arc<RwLock<Registry<HighProgram<HighProgramValue>>>>,
    packages: HashMap<PathBuf, PackageHandle>,   // 按 canonical path 缓存
}

pub struct PackageHandle {
    pub path: PathBuf,
    pub key: ModuleKey,
    /// 包导出的 value：包源文件最后一个 expr 的 value 节点的 static ref。
    pub value: StaticNodeId,
    /// 包导出 value 的 type 节点的 static ref。
    pub ty: StaticNodeId,
}
```

方法：

- `PackageStore::new()` — 创建一个共享 registry。
- `load_package(&mut self, path: &Path) -> Result<PackageHandle, Vec<Diag>>`：
  1. 读文件；调用 `preprocess`（递归解析包自身的 import——依赖先加载先 freeze；见 §8）。
  2. `lex` / `parse` / `compile`（cleaned source，无 imports）。
  3. `Checker::build(ir)`（package 私有 registry，保证 `from_module` 的 dependency-free 断言）。
  4. `module.evaluate_node_deep(root_val)` / `module.evaluate_node_deep(root_ty)`。
  5. `self.registry.write().freeze_mapped(&build.module)` 得 `Freeze { key, node_map }`。
  6. 生成 `PackageHandle { key, value: StaticNodeId { module: key, index: node_map[&root_val] }, ty: StaticNodeId { module: key, index: node_map[&root_ty] } }`。
  7. 缓存并返回。
- `resolve_import(&mut self, base: &Path, import_path: &str) -> Result<PackageHandle, Diag>`：
  - v1 解析规则：`import_path` 相对当前文件目录；`base` 为当前源文件路径或调用方显式传入的目录。

#### 4c. 运行入口

文件：`crates/lichen-language/src/run.rs`。

- 新增 `evaluate_raw(raw_source, base: Option<&Path>, store: &mut PackageStore) -> Result<String, Vec<Diag>>`：
  1. `preprocess(raw_source, base, store)` → cleaned source + imports + diags。
  2. `compile_with_imports(cleaned_source, imports)` → Report（或直接走 build）。
  3. `Checker::build_in` 绑定 store registry。
  4. 运行与现有 `evaluate` 一致。
- 现有 `evaluate(source)` 保持行为不变（无 import 的纯 lichen 源文件）；含 `@import` 的 raw source 走 `evaluate_raw`。

`crates/lichen-language/src/lib.rs`：

- 导出 `preprocess` 模块、`package` 模块、`evaluate_raw`。
- `compile` 增加 `compile_with_imports`，旧 `compile` 保持兼容。

### Phase 5 — CLI / 包管理器原型

文件：`crates/lichen-language/src/main.rs`。

在现有 `lichen <file.lichen | directory>` 基础上增加子命令（同时保持无子命令的旧用法）：

```text
lichen run <file.lichen | directory>   # 与旧用法相同，但 raw source 先过预处理器
lichen build <file.lichen>             # 校验包：preprocess（含传递依赖）→ compile → freeze 到内存 registry，打印包 value 的 type
```

- `run`：对单文件创建 `PackageStore`，`evaluate_raw(source, Some(path), &mut store)`；对目录每个文件创建 store 后运行（import 相对该文件解析）。
- `build`：调用 package 编译路径，打印 `built <file>` 以及 `print_type` 的结果；不做磁盘持久化（持久化是 future work，见 §6）。
- 旧 `path_arg` 分支继续兼容：等同 `run path_arg`（自动先过预处理器）。

---

## 4. 关键设计决策与风险

1. **Import 在预处理器，不在 language parser（用户决策）**。
   - lexer/parser/AST 保持纯语言；`@import` 是预处理器专用格式。
   - 预处理器只做行级文本处理；parser 只处理 cleaned source；两者互不可见。
   - 好处：语言语法保持稳定；预处理器格式可以独立演进（例如未来换 `#import`、加条件编译）。
   - 代价：导入信息不能通过 AST 传递，必须由预处理器显式输出 `ResolvedImport`，compile 前端增加一个初始 scope 注入点。

2. **Import = IR `ExprKind::Static`，而不是复制 AST/IR**。
   好处：复用现有 name resolution（scope 映射到 `ExprId`），checker 无需新的 scope 概念；坏处：`ExprKind` 增加 variant 会触碰所有 match（编译器会穷尽检查，机械）。

3. **Static seam 采用“只读优先，checker 按需 materialize”**。
   - 渲染/诊断绝不修改 module（保持 `&Module` API）。
   - checker 中需要 `NodeId` 参与构造/统一时，用 `materialize_leaf` 生成动态叶子；其值仍是静态共享 payload。
   - 注意 `is_universe` 不能靠 `equality_representative` 比较 materialized 静态 universe，需要内容级判断（见 2c）。

4. **包编译绑定共享 registry（2026-08-31 起，含传递依赖）**。
   - `Checker::build` 保持旧行为（dependency-free 程序仍是一个 module 一个私有 registry）。
   - 包加载走 `compile_with_imports_in(..., Some(store.registry()))`：依赖先 freeze（键已注册），包自身的 import leaves 原地解析，freeze 时依赖 refs 原样保留。
   - 循环导入由 `PackageStore` 的加载栈检测，在预处理器层诊断（消息携带链路，caret 落在闭合环的 `@import` 行）。

5. **包导出 = 最后一个 expr 的 value（用户决策）**。
   - 包源文件因此就是普通 lichen 程序，没有额外的导出语法。
   - 冻结的是 `root_term`（最终 `[value, type]` pair）；导入名与它直接绑定。

6. **传递依赖 = 绝对 key refs + verbatim freeze（2026-08-31 实现，见 §8）**。
   - lowlevel `from_module` 的 dependency-free 断言删除；`Registry::freeze_mapped` 改为检查"源值里引用的每个模块 key 都已在本 registry 注册"。
   - refs 是 key-carrying 的，freeze 一个含依赖 refs 的模块时把它们原样写入（不重定向、不拷贝 payload）。

7. **无持久化**。
   - 本阶段 package store 是进程内的；跨进程缓存/安装需 `DistributeModule` 序列化格式 + 加载时 re-key，lowlevel 计划已预留，不是本 plan 的目标。

---

## 5. 测试计划

### lowlevel

- `freeze_mapped` 的 `node_map` 与静态 refs 一致性。
- 现有 `freeze` 行为不回归。

### highlevel

- `ExprKind::Static`：checker 编译 `Static` 节点为 `[materialize(value), materialize(ty)]`，值/类型可被后续 `Index` / `Apply` 使用。
- static-aware `kind_marker_is` / `is_function_type` / `is_indexable_type`：导入函数类型能通过 `check_app` 的 function-ness guard；导入 struct/array/tuple 类型能通过 `check_index` 的 index-target guard。
- `Checker::build_in`：绑定共享 registry 后，`module.static_read` 能解析 static refs。

### preprocessor

1. `@import "pkg.lichen" as x` 被识别并从 cleaned source 删除；cleaned source 行号与 raw 一致。
2. 非 `@import` 文本逐字节透传（含 `--` 注释、空行、`@` 出现在非行首等）。
3. 缺少 `as`、`as` 后不是 name、路径不是字符串 → 预处理器诊断，parser 不参与。
4. 循环导入（a→b→a）→ 预处理器诊断，caret 落在闭合环的指令行（传递依赖测试见 §8）。
5. cleaned source 交给 lexer 后不会产生任何 `@` 非法字符错误（回归测试）。

### language / package store

1. 包文件 `42` + 导入方 `@import "pkg.lichen" as x; x` → 输出 `42: Int`。
2. 包文件 `x => x + 1` + `@import "pkg.lichen" as f; f 41` → `42: Int`。
3. 多态函数：包文件 `x => x`；导入后 `(f 1, f Int)` → `(1, Int): <Int, Type>`。
4. struct 类型：包文件 `struct<Int>`；导入后 `s(5)` 实例化并渲染 `struct<Int>`。
5. 两个不同 import 方共享同一包：同一进程连续 `evaluate_raw` 两个文件，包只 freeze 一次（`PackageStore` 缓存），两个 module 通过同一 key 解析。
6. `@import ... as name` 名字可被本地 binding shadow；import 在文件中前后可见。
7. 诊断：包路径不存在、`@import` 格式错误、循环导入、包内编译失败（定位到导入方指令行）。
8. CLI：`run` 支持 `@import`；`build` 打印 `built <file>` 与包 value 的 type。

---

## 6. 明确的非目标

- `DistributeModule` 磁盘序列化 / 跨进程安装 / lockfile / 版本解析。
- language parser 中的 `import` 语法、`export` 关键字（保持 language 纯净）。
- 预处理器宏 / 条件编译（`@import` 只做包导入）。
- 跨进程 registry 锁、文件系统 backing store。
- 传递依赖（~~v1 非目标~~ → **2026-08-31 已实现**，见 §8）。

这些留给后续：`DistributeModule` 加载时 re-key，registry 成为真正的翻译点。

---

## 7. 建议实现顺序

1. Phase 1（lowlevel `freeze_mapped`）—— 小且独立。
2. Phase 2（highlevel static seam）—— 最大风险，先行。
3. Phase 3（预处理器 + `ResolvedImport`）—— 可与 2 并行。
4. Phase 4（compile 前端注入 imports / PackageStore / run）—— 依赖 1、2、3。
5. Phase 5（CLI）—— 薄封装。
6. Phase 6（测试、README、`docs/language-spec.md` 增加预处理器/包管理器独立章节，language grammar 保持不含 import）。

---

## 8. 传递依赖的实现（2026-08-31）

包可以 `@import` 其它包。整体形状就是 §4.6 预留的路径：**包的预处理器也解析
`@import`，包编译走共享 registry，freeze 断言放宽到"已冻结依赖的 key 已注册"**。
关键点：

### lowlevel（`static_module.rs` / `lib.rs`）

1. **`from_module_mapped` 的 dependency-free 断言删除**。取而代之，
   `Registry::freeze_mapped` 在 build 之前做 **referenced-keys 检查**：遍历源模块
   所有节点的值，收集 static ref 命名的模块 key（数组 item refs、函数值 refs、
   static handles 的 key），断言每个都已在目标 registry 注册。检查放在
   `freeze_mapped`（而非 `from_module` 内）同时避免了死锁：包模块绑定的是共享
   registry，freeze 已持有它的写锁，`from_module` 内再读锁同一个 registry 会自锁。
2. **rewrite 保留跨模块 refs verbatim**：
   - 动态数组的 item 里出现的 `AnyNodeId::Static`（指向依赖模块）原样写入——
     绝对 key 从出生起就是最终 key，无需重定向（v1 里是 `unreachable!`）。
   - 值本身已是 static 的（static 数组 payload、static 函数值、static ext
     handle）原样返回——payload 留在依赖的共享 arena，不拷贝、不 re-key。
     Phase-2 收集也只收集 dynamic payload。
3. **`static_remap_value` 的 key 守卫**：应用静态函数时，值 item 的 remap /
   parameterized 查询只允许针对 `sref.module == ctx.module.key` 的引用——local
   index 是 per-module 的，用外模块的 local index 索引本模块节点表会读错节点甚至
   越界。外模块引用按构造即 concrete（产生它的那次 apply 只对 concrete item 保留
   verbatim ref），原样保留。

### language（`package.rs` / `preprocess.rs`）

4. **`PackageStore` 的加载栈**：`loading: Vec<PathBuf>`。`load_package` 先查栈
   （同 canonical path 在栈中 = 循环导入 → 诊断），再查缓存，然后入栈 →
   `build_package` → 出栈。环的诊断消息携带完整链路（a → b → a）。
5. **`build_package` 对共享 registry 编译**：递归 `preprocess`（依赖先加载先
   freeze）→ `compile_with_imports_in(cleaned, imports, Some(shared_registry))` →
   deep-evaluate → `freeze_mapped` → export = `node_map[root_term]`。
6. **失败依赖的诊断定位**：包加载失败（路径不存在 / 环 / 包内编译错误）时，
   预处理器把诊断重定位到**导入方的 `@import` 行**（caret 属于当前文件），消息前缀
   `cannot load package '<path>':`，逐层嵌套即完整 import 链。
7. **`packages` 缓存公开**：宿主/测试可观察"同包只 freeze 一次"。

### 示例（多文件互相引用的展示）

`examples/programs/packages/` 存放可导入的包，顶层 `imports.lichen` 导入它们：

- `packages/math.lichen` —— 导出 `succ`（普通 lichen 文件，最后表达式即导出）。
- `packages/geometry.lichen` —— `@import "math.lichen"`，导出基于 math 构建的
  `double`（传递依赖）。
- `imports.lichen` —— 同时直接导入 math 和传递导入 geometry，`(math 41, geo 5)`
  → `(42, 12): <Int, Int>`。

`readme.rs` 渲染 `packages/` 子目录（标题 `packages/math.lichen`），所有示例
（含包文件）经 `evaluate_raw`（每文件一个 store、以文件自身为 base）运行；
README 的示例区由 `tests/readme.rs` 自动同步。CLI：`run`/无子命令跑含 import 的
文件或目录，`build` 对含传递依赖的包工作（合成的自导入改用文件名相对解析）。

### 测试

- lowlevel：`freezing_keeps_dependency_refs_verbatim`（refs/items/handles 原样、
  跨两模块读取）、`freeze_rejects_an_unregistered_dependency_key`（跨 registry
  freeze 拒绝）、`static_apply_keeps_foreign_items_in_place`（remap 守卫回归，
  外模块 local index 特意超出本模块节点数）。
- language：传递 apply 链（inner→middle→main，跨模块模板 apply）、传递 struct
  类型（内层定义、中层实例化、外层索引）、菱形导入每包一次、循环导入诊断、
  失败依赖定位到指令行、双导入方共享一个包。
