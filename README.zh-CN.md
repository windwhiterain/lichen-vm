<div align = "center">
<img src="logo.png" width="200">

# LichenVM

*面向类型检查器、静态分析器和语言智能工具的基础设施*

![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg) ![Contributions](https://img.shields.io/badge/contributions-welcome-brightgreen) ![status](https://img.shields.io/badge/status-prototype-red)

[English](README.md) | [简体中文](README.zh-CN.md)

</div>

## 为什么是 LichenVM

近几年，面向机器学习 kernel 的 DSL 层出不穷，但这些 DSL 上的静态分析往往几乎没有实现，或者没有很好地集成到语言服务中。

一些 embedded DSL 会尝试依赖宿主语言的类型系统来编码静态分析逻辑，但这通常会带来更慢的分析速度，也会让分析逻辑和运行时集成方式完全不同。

于是有了这个想法：编译器有 LLVM，那么静态分析器也需要一个类似的“LLVM”，也就是 LichenVM。它应该是模块化、分层、可组合的，并且能够在原始运行时程序之上逐步增加更多检查属性。

## 特性

- **统一运行时**

  值计算、类型检查以及更多分析能力，都被编码到统一的运行时计算图中：类型可以被计算，值也可以依赖类型。

- **模块化分析属性**

  值是表达式的一种属性，类型也是如此。它们由插件定义，可以被下游插件继续扩展，也可以定义更多属性，比如可见性。

- **零成本插件系统**

  插件系统通过代码生成实现，并使用 enum dispatch 表达值、算子等可扩展概念。

- **推断任意属性**

  统一运行时可以为节点添加等式约束，这些节点的值会以结构化、递归的方式被推断：值可以从值或类型中推断，类型也可以从类型或值中推断。

## 开始开发

项目仍处于原型阶段，可以先从测试用例开始了解。
