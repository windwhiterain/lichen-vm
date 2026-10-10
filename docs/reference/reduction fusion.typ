= PLDI'26：自动 Reduction Fusion [论文详解]

原文：#link("https://arxiv.org/abs/2510.08726")[Neptune: Advanced ML Operator Fusion for Locality and Parallelism on GPUs]

Flash Attention v1 就是在优化 safe-softmax。张量算子包含 element-wise，reduction 等类型，safe softmax中就包含了两个 reduction 算子 max 和 sum，在后续的注意力计算中还有一个 sum reduction。为了减少数据运输的开销，优化上需要合并（fuse）算子以避免同储存大量中间计算结果。element-wise 算子的合并早研究，然而 reduction 算子的合并在 Flash Attention v1 时并不存在，这导致原始的 Attention 计算有三个 reduction 算子，需要存储两个中间计算数据。Flash Attention 手动合并了三个 reduction 算子为一个，不再需要储存中间数据。那么如何自动合并 reduction 算子呢？

原文中数学公式有些绕口，笔者已按自己的理解修改。

- $"op"$: 匿名函数
== Repair Function
首先给出需要 fusion 的两个 reduction （分别命名为 producer 和 consumer）的定义：
$ 
i &:= 0..N\
r_(i+1) &:= "op"(r_i,d_(i+1))\
t_(i+1,N) &:= f(t_(i,N),g(d_(i+1),r_(i+1),r_N))\
$
- $d_i$: 输入
- $r_i$: producer reduction sequence
- $g(d_i,r_i,r_N)$: element-wise operation 依赖 producer reduction
- $f$: 满足结合律的 reduction operator
- $t_(i,N)$: consumer reduction sequence
理想的 fusion 结果应当是：
$
  t_(i+1) = "op" (t_i,i,i+1)
$
显然 $g(d_i,r_i,r_N)$ 中的 $N$ 阻碍了 fusion，对此的解决方法是假设 $N$ 在增长，把 reduction sequence 建立在 $N$ 上而非 $i$ 上：
$
N &:= 0..K\
h(t_(N,N),r_N,r_(N+1)) &:= t_(N,N+1)\
=> t_(N+1,N+1) &= f(h(t_(N,N),r_N,r_(N+1)),g(d_(N+1),r_(N+1),r_(N+1)))\
t'_N &:= t_(N,N)\
=> t'_N &= f(h(t'_N,r_N,r_(N+1)),g(d_(N+1),r_(N+1),r_(N+1)))
$
- $h$：repair function
这样 $t'_N$ 就成了 fusion 后的 reduction sequence。其中 $h$ 被称为 repair function，取 $N$ 变为 $N+1$ 后要将 $t_(i,N)$ 修复为 $t_(i,N+1)$ 的意思。$N$ 增长后所有的 $g(d_i, r_N)$ 需要被修复为 $g(d_i, r_(N+1))$，而 $t$ 又是对$g$ 关于 $f$ 的 reduction，那么只要 $h$ 能修复每个 $g$，并关于 $f$ 满足分配律，就能修复 $t$ 了：
$
h(g(d_i,r_i,r_N),r_N,r_(N+1)) &= g(d_i,r_i,r_(N+1))\ 
h(f(x,y),r_N,r_(N+1)) &= f(h(x,r_N,r_(N+1)),h(y,r_N,r_(N+1))) 
$
首先考虑如何找到能修复 $g$ 的 $h$。我们希望得到类似于 $h(x) = "op"(x)$ 的表达式，但方程中给出的是 $h(g(x)) = "op"(x)$，这需要使用反函数法：
$
y &:= g(x)\
=> x &= g^(-1)(y)\
=> h(y) &= "op"(g^(-1)(y))
$
$N$ 是 $h$ 的参数之一，所以 $h$ 中的 $g(d_i,r_i,r_N)$ 本质上是 $g(d,r,r_N)$ 函数在 $i$ 处的值，然而 $g(d,r,r_N)$ 的参数 $i$ 的定义域是离散的，求它的反函数将会陷入困难，因此引入一个假想的连续参数 $c$：
$
g'(r_N)(c_i) &:= g(d_i,r_i,r_N)\
=> h(g'(r_N)(c_i),r_N,r_(N+1)) &= g'(r_(N+1))(c_i)\ 
<== h(g'(r_N)(c),r_N,r_(N+1)) &= g'(r_(N+1))(c)\ 
$
原来该等式需要对任意离散的 $d_i,r_i$ 成立，改写后则需要对任意连续的 $c$ 成立，这当中自然包含了离散 $d_i,r_i$ 的全部情况。

应用反函数法：
$
x &:= g'(r_N)(c)\
=> c &= g'(r_N)^(-1)(x)\
=> h(x,r_N,r_(N+1)) &= g'(r_(N+1))(g'(r_N)^(-1)(x))\
$
只要 $h$ 满足关于 $f$ 的分配律我们就找到了 repair function。

== Rolling Update
repair function 的分析是在单 producer reduction 的情况下的，实际上这可以扩展到多个 producer reduction，它们的输出拼接为输入 $d$。

repair function 的分析只考虑了一个 element-wise operation，实际上 reduction 之间的 element-wise operation 全部可以被 inline 到 $g$。

=== 对于不可逆的 $g'(r_N)$：
例如：
$
g(d_i,r_i,r_N) := exp("relu"^2(d_i) - r_N)
$
目标是用 $c$ 改写之使得：
$
g'(r_N)(c_i) := g(d_i,r_i,r_N)
$
从 $i$ 的表达式出发，向更外层表达式迭代，可以得到如下改写：
$
c_i:=d_i &=> g'(r_N)=exp("relu"^2(c_i) - r_N)\
c_i:="relu"^2(d_i) &=> g'(r_N)=exp(c_i - r_N)\
c_i:="relu"^2(d_i) - r_N &=> g'(r_N)=exp(c_i)\
&..
$
可以发现第一种改写不可逆，第二种可逆，第三种可逆但 $c_i$ 的表达式与高阶函数 $g'$ 的参数 $r_N$ 有关，这种改写就丢失了函数的信息。

可以总结出规律即找到最外层与 $r_N$ 无关的 $i$ 的表达式改写为 $c$ 可最大程度避免不可逆 $g'(r_N)$。

== Split-K Update
就是在把 fuse 过后的 reduction 并行化，以 global，local 两层为例：
$
j &:= 0..N\
r^"local"_(i,j+1) &:= "op"(r^"local"_(i,j),d_(i,j+1))\
t^"local"_(i,j+1) &:= f(h(t^"local"_(i,j),r_(i,j),r_(i,j+1)) , g(d_(i,j+1),r_(i,j+1),r_(i,j+1)))\
i &:= 0..M\
r^"global"_(i+1) &:= "op"(r^"global"_i,r^"local"_(i+1,N))\
t^"global"_(i+1) &:= f(h(t^"global"_i,r^"global"_i,r^"global"_(i+1)) , h(t^"local"_(i+1,N),r^"local"_(i+1,N),r^"global"_(i+1)))
$

