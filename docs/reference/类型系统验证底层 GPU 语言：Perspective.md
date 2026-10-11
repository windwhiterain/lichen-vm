# PLDI'26：类型系统验证的底层 GPU 语言 [论文详解]
原文：[Modular GPU Programming with Typed
Perspectives](https://arxiv.org/abs/2511.11939)

背景：[PLDI'26：类型系统验证的底层GPU语言 [论文Intro]](https://zhuanlan.zhihu.com/p/2033200226341737579)

原文设计的语言存在表达力上的缺陷，笔者已加以扩展。

该语言以 python embedded 的方式实现。

## Contract in Cuda
- uniform parameter：cooperative 的函数可能需要每个参与的thread 提供一些相同的参数，例如 `reduce(group, val, op)` 中的 `op`
- disjoint mutation：并行的 threads 需要避免同时写内存中的同一个位置
- synchronization：并行的 threads 写内存后需要同步给其他 threads，例如 `__syncthreads`
- convergence：cooperative 的函数需要一段连续的线程同时执行该函数，例如 `wmma`

## Perspective
作者提出了一种关于 expression 和 scope 的属性 perspective：
``` python
  perspective := level(n)
```
- `level`：GPU compute hierarchy 中的某一个层级：
  - `grid`
  - （cluster，作者忽略了）
  - `block`
  - `thread`
- `n`：表示连续的 `n` 个 `level`（其 local index（相对于上一层 `level` 的 index）必须 align 于 `n`）

perspective 表示以连续的 `n` 个 `level` 构成的一组 threads 的形式。

读者可能会疑惑 `level` 中为什么不包含 `warp`。这是因为一个 warp 内 thread 的数量是固定的 32，warp 完全可以视作语法糖：
``` python
warp(n) ≔ thread(32 * n)
```
相对的一个 grid 内 block
的数量，一个 block 内 thread 的数量是可变的： 
``` python
grid(1) ≔ block(B)
block(n) ≔ thread(T * n)
```
- `B`：一个 grid 内 block 的数量
- `T`：一个 block 内 thread 的数量 
  
不引入 grid 和 block 就得引入变量 `B` 和 `T`。

读者可能还会疑惑在 AMD GPU 中对应 nvidia GPU warp 概念的 wave-front 一个内的 thread 数量是 64，如果该语言要适配不同厂商的 GPU 的话应该需要 warp 抽象才对。

但实际上 cuda warp level 的 API 已经完全耦合了 warp 内 thread 的数量，因此即使要适配 wave-front 也会直接在 tile API 上区别它和 warp。

相对的，由于 `B` 和 `T` 是变量，cuda API 并没有耦合它们，这才能够抽象出来。
- `align(A, B)`：perepective A align 于 B：threads 数量 A align 于 B
- `A/B`：perspective A 的 threads 数除以 B
- `divisor(A, B)`：返回 threads 数是 perspective A，B threads 数的公因数的 perspective

## Expression Perspective
在 expression 上定义两种 perspective 属性 （作者将这两种看作一种）：
``` python
expr: type(DP) @ UP
```
- `UP`（uniform perspective）：该 expression 的值在该 prespective 形式下任意一组 threads 内一致，这是对 cuda uniform parameter contract 的保证
- `DP`（disjoint perspective）：该 expression 可变引用的内存区域在该 perspective 形式下任意任意两组 threads 间都不重叠，这是对 cuda disjoint mutation contract的保证

`UP` 跟随着 `type` 一起参与类型检查。 

`DP` 由 `type` 给出，在类型标注中可以省略。

例子：
``` python
@prism("global")
@requires(grid[1], block[1], thread[32])
def kernel:
  a: int @ grid[1] = 1
  # ok:
  # literal has grid[1] UP
  b: int @ thread[32] = a
  # ok:
  # grid[1] can lower to thread[32]
  c: int @ thread[24] = a
  # error:
  # grid[1] can't lower to thread[24]
  d: int @ thread[16] = a
  # ok:
  # grid[1] can lower to thread[16]
  e: int @ thread[16] = b
  # ok:
  # thread[32] can lower to thread[16]
```
- `@prism("global")`：定义 kernel 函数
- `@requires(block[b], thread[t])`: 约束对该 kernel 的 launch `grid[1]` align 于 `block[b]`，`block[1]` align 于 `thread[t]`
- lower：`UP` 可以隐式地 lower 到它 align 于的 perspective
``` python
@prism("global")
@requires(grid[1], block[1], thread[96])
def kenerl
  a: int @ thread[48] = 1
  b: int @ thread[32] = warp_id()
  c: int @ thread[16] = add(a, b)
```
- `b`，`c`：uniform perspective 像类型一样通过函数签名推断出来
  
`warp_id`、 `add` 的函数签名：
``` python
def warp_id() -> int @ thread[32]: ...
def add[P1, P2](l: int @ P1, r: int @ P2) 
  -> int @ divisor(P1,P2): ... 
```

## Data Memory Heirarchy
memory heirarchy 被编码在 data type 中。他们的 `DP` 由 data type 给出：
|memory hierarchy|data type|DP|
|--|--|--|
|thread local|`int`, ...|`thread[1]`|
|shared|`shared(T, N)`|`block[1]`|
|global|`ptr(T, N)`|`grid[1]`|
|view of global/shared|`view(T, N, DP)`|`DP`|
|anything immutable|`const(T)`|`0`|

一个 thread local 变量只是对每个 thread 中的寄存器或 local memory 的可变引用，只能被本线程访问，因此它在 thread 间永远不会重叠。

`shared(T, N)`、`ptr(T, N)`、`view(T, N, DP)` 都属于 `thread_shared`，它们可变引用的内存区域可以被多个 thread 访问。

一个 `shared(T, N)` 对 shared memory 的可变引用能被一个 block 内的 threads 访问；一个 `ptr(T, N)` 对 global memory 的可变引用可以被 grid 内的 threads 访问；`view(T, N, DP)` 对 shared memory 或 global memory 的引用是经过一定逻辑的划分得到的，能访问它的 thread 与由该逻辑决定（`DP`）。

它们的类型签名：
``` python
class view:
  T: type
  N: int
  DP: Perspective
  def __getitem__[UP, UP2](
    self: Self @ UP, 
    key: int @ UP2,
  ) -> T @ divisor(UP, UP2): ...
  def __setitem__(
    self, 
    key: int, 
    value: T,
  ):
    assert(DP == thread[1]) 
class shared(view):
  DP = block[1]
class ptr(view):
  DP = grid[1]
```
直接写 `view`，每个 thread 都会执行写指令，无法保证 disjoint mutation，因此 `__setitem__` 需要 `DP == thread[1]`，即要求它可变引用的内存在每个 thread 之间都不重叠。

要想从大 `DP` 的 `view` 获取小 `DP` 的 `view`，需要使用内置函数 `partition`：
``` python
data: ptr(T, 1024) @ grid[1]
offset = lambda x: 1024 / B * x
with partition(
  data, 
  block[1], 
  offset,
) as data:
  data: view(T, 1024/B, block[1]) @ block[1]
```
`partition(data, P, offset)` 返回用自定义的寻址函数 `offset` 将 `data` 划分为 disjoint 的具有其 align 于的 perspective 的子 data 分配给 perspective `P` 代表的 threads 组。原 `data` 将被 shadow。

当 `with` 作用域结束后，编译器将在下一个对 `data` 的访问指令前插入数据同步指令，这是对 cuda synchronization contract 的保证。

至于 `offset` 的 disjoint 性需要用户负责或在未来引入证明器。

`partition` 的函数签名：
``` python
def partition[T, N, DP](
  data: view(T, N, DP),
  P: Perspective,
  offset: Callable[[int], int] 
) -> view(T, N/(DP/P), P) @ P:
  assert(aligned(DP, P))
```
例子：
``` python
a: shared(int, 1024) @ block[1]
b: int @ block[1] = a[0] 
c: int @ thread[32] = a[warp_id()]
a[0] = warp_id()
# error:
# `__setitem__` requires
# DP of `a` == thread[1]
offset = lambda x: 1024 / B * x
with partition(a, thread[1], offset) as a:
  a: view(int, 1024/B, thread[1]) @ thread[1]
  a[0] = warp_id()
  # ok
```

## Scope Perspective
在 scope 上定义一种  perspective 属性：表示该 scope 中的 statement 以 perspective 表示的 threads 组一起执行，是对 cuda convergence contract 的保证。

在 scope 和 `view` 上定义 skip 集合：多个 Perspective 区间的并：
$$
  skip := \sum_i [P_h^i, P_l^i]
$$
- scope skip：表示 scope perspective 代表的 threads 组中因为控制流而缺失的 threads 组

- `view.skip`：表示 `view` 在向其 `DP` 表示的 threads 组的分配过程中跳过的划分

函数可以约束调用它的 scope：
``` python
@requires(thread[32])
def wmma(..): ...
```
- `requires(P0, P1, ..)`：要求该函数只能在 perspective align 于所有 P0, P1, .. 的 scope 中调用

kernel 函数的 scope 总是有 perspective `grid[1]`：
``` python
@prism("global")
def kernel():
  ... # grid[1]
```
内置函数 `split` 是静态控制流函数：
``` python
# grid[1]
match split(thread[32]):
  case 16: 
    ... # thread[16], thread[32->16]
  case 8: 
    ... # thread[8], thread[32->8]
  case 8: 
    ... # thread[8], thread[32->8]
```
`split(P)` 将每个 `P` 表示的 threads 组按照顺序连续地分为其 align 于的若干个子 threads 组，分别执行不同的代码。

每个 case 的 scope 拥有对应的 perspective，并且对 skip 集合并上一个对应的 skip 区间。

例子：
``` python
@prism("global")
@requires(thread[32])
def kernel():
  wmma(..)
  # ok
  match split(thread[32])
    case 16: 
      wmma(..)
      # error:
      # `wmma` requires scope
      # perspective thread[32]
    case _: ...  
```

scope 的 perspective 和 skip 集合从 kernel 的 scope 开始，通过`split`构成 hiearchy：
``` python
# grid[1]
match split(block[4]):
  case 2:
    # block[2], block[4->2]
    match split(thread[32]):
      case 8:
        # thread[8], sum(
        #   block[4->2],
        #   thread[32->8],
        # )
      case _: ...
  case _: ...
```
相对于 `split` 将 `view` 划分后分配给多个 threads 组访问，内置函数 `claim` 将 `view` 全部划给一个 threads 组访问，该层级下的其他的 threads 组则不得访问。与此同时向其 skip 集合中并入相应 skip 区间：
``` python
a: view(int, 1024, thread[32]) @ thread[32]
# a.skip = sum(
#   thread[64->32],
# )
with claim(a, thread[1]) as a:
  # only 1 thread in 32 thread can access `a`
  a: view(int, 1024, thread[1]) @ thread[1]
  # a.skip = sum(
  #   thread[64->32],
  #   thread[32->1],
  # )
  # = thread[64->1]
```
scope 中的 statement 能访问某个 `view` 的条件是：`view` 的 skip 集合包含于 scope 的 skip 集合中。可直观理解数据跳过的区域必须包含于执行跳过的区域，否则执行时就取到被跳过的数据了。

例子：
``` python
a: view(int, 1024, thread[32]) @ thread[32]
with claim(a, thread[1]) as a:
  a: view(int, 1024, thread[1]) @ thread[1]
  a[0] = 1
  # error:
  # thread[32->1] is not include
  # in scope skip None
  match split(thread[16]):
    case 1:
      a[0] = 1
      # error:
      # sum(thread[32->1]) is not include
      # in scope skip thread[16->1]
    case _: ...
  match split(thread[64]):
    case 1:
      a[0] = 1
      # ok
      # sum(thread[32->1]) is include
      # in scope skip thread[64->1]
    case _: ...
  match split(thread[32]):
    case 1:
      a[0] = 1
      # ok
    case _: ...
```
这是对 cuda disjoint mutation contract 的保证（作者通过强制要求 `claim` 必须紧跟一个与之相应的 `split`，忽略了 skip 集合）

> 笔者准备实现一个该文章描述的类型系统 + 函数式的语言