# LichenVM

*一个基于惰性求值与合一（unification）的编程语言与类型检查虚拟机 —— 在这里，类型只是一种值。*

[English](README.md) · [简体中文](README.zh-CN.md)

LichenVM 是一个围绕同一个想法构建的最小编程语言：**类型检查与求值是同一个过程**。程序被编译成一张节点图；每个节点的值和它的类型都由同一个解释器计算，而结构化合一（structural unification）是唯一的类型检查规则。`Type : Type` 在单一宇宙中成立——这里没有另一套独立的类型系统需要学习，因为类型就是普通的值。

这个项目同时也是*自定义类型检查器的基础设施*：虚拟机是一个通用的解释器，检查器的运行时行为（合一、按应用实例化）就是语言运行时本身。

## 特性

- **类型即值（Type = value）**。没有类型/值的区分。`Int`、`Type`、`Int -> Int`、`Int<3>` 都是普通的值，可以被绑定、传递、放进元组。
- **自动 let 多态（let-polymorphism）**。每个 lambda 天生多态：每次应用都会用全新的单元格实例化参数，所以同一个绑定可以在 `Int` 和 `Type` 上同时使用。不需要也不存在 generalize/instantiate 特殊形式。
- **惰性的依赖数组类型**。`Int<n>` 的长度可以是任意表达式。被绑定的长度在检查时惰性解析并固定（pinning）——`((n => ([1, 2, 3] : Int<n>)) 3)` 可以通过检查，而传入其他长度会失败。
- **一等类型（First-class types）**。通过部分函数应用实现类型实例化。
- **完整的前端**。手写的词法、语法、名字解析与 IR 生成器，每个阶段都有规范的诊断信息（源码位置 + 脱字符标注），支持语句、绑定和索引。
- **精简的虚拟机**。惰性求值、块级垃圾回收、最小化内存分配。

## 快速开始

需要 Rust 工具链。

```bash
cargo build
cargo test
```

运行全部示例程序（每个示例输出一行 `文件: 输出`）：

```bash
cargo run -p lichen-language -- crates/lichen-language/examples/programs
```

运行单个程序：

```bash
cargo run -p lichen-language -- crates/lichen-language/examples/programs/bindings.lichen
# 1
```

安装 CLI（可执行文件名为 `lichen`）：

```bash
cargo install --path crates/lichen-language  # 在本仓库的检出中执行
cargo install --git git@github.com:windwhiterain/lichen-vm.git lichen-language
```

然后直接运行：

```bash
lichen crates/lichen-language/examples/programs/bindings.lichen
# 1
```

## 尝鲜

```text
5 : Int                             -- 5
(x => x) 5 : Int                    -- 5
a = [1, 2]; b = 0; a[b]             -- 语句与绑定：1
((id => ((id 5 : Int), (id Type : Type))) (x => x)) : <Int, Type>
                                    -- let 多态：[5, Type]
((n => ([1, 2, 3] : Int<n>)) 3) : Int<3>
                                    -- 依赖数组长度：[1, 2, 3]
([1, 2, 3])[1]                      -- 索引：2
(i => [10, 20][i]) 1 : Int          -- 用索引做分支：20
```

## 工作原理

一个程序就是一个表达式。每个表达式都被编译成一对 **[value, type]**（值、类型对），类型脊线的终点是规范宇宙 `K = [Type, ↺]`（通过自环实现 `Type : Type`）。检查器本身就是一个解释器：它编译 IR，运行定义阶段（definition pass）让应用期的类型检查生效，然后求值根节点——这就是程序的返回值。

- **合一就是检查**。`Module::unify` 是一种结构化的惰性合一。纯净的未绑定单元格会被绑定；带有未求值计算的类属于"待计算"（pending computation），必须先被解析，任何具体值都不能覆盖它。
- **应用就是绑定**。`f x` 会把 `f` 的参数克隆一份全新的实例，再与 `x` 合一——这正是每个 lambda 自动 let 多态的原因，也是"运行时即类型检查器"的含义。
- **语句是图的共享**。`a = e; …` 只编译一次 `e`，之后对 `a` 的每一处使用都预先解析到同一个节点——IR 仍然是纯粹的表达式图，不需要 `let` 节点。
- **块级垃圾回收**。节点按块（block）组织；求值只压实返回可达的那棵子树，并释放腾空的块。

## 子 crate

| crate | 作用 |
|---|---|
| [`lichen-lowlevel`](crates/lichen-lowlevel) | 虚拟机：节点、块、惰性求值、结构化合一、垃圾回收。 |
| [`lichen-highlevel`](crates/lichen-highlevel) | IR 与检查器：每个表达式都是 [value, type] 对；检查与运行是同一个过程。 |
| [`lichen-language`](crates/lichen-language) | 前端：源代码 → IR（带诊断信息），并提供命令行与示例程序。 |
| [`lichen-utils`](crates/lichen-utils) | 共享工具。 |

## 文档

- [`docs/language.md`](docs/language.md) — 语言规范（语法、语义、诊断信息）
- [`docs/highlevel.md`](docs/highlevel.md) — 检查器设计
- [`docs/hm-loc.md`](docs/hm-loc.md) — 面向初学者的 HM-loc 推断方法讲解
- [`README.old.md`](README.old.md) — 最初的设计笔记

## 状态

v1 已形成完整流水线——文本 → 词法 → 语法 → 解析 → IR → 检查 → 运行——每个阶段都有诊断信息，并有 190+ 个测试。尚未实现：算术与条件、递归、按应用进行的依赖检查、参数注解、错误恢复、JIT 编译。

## 设计哲学

- 最小化内存占用，最小化内存分配。
- 平凡程序追求高速度（复杂程序未来交给 JIT 编译）。
