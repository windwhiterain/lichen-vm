<div align = "center">
<img src="logo.png" width="200">

# LichenVM

*infrastructure for tye checkers, static analyzers and language intelligence tools*

![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg) ![Contributions](https://img.shields.io/badge/contributions-welcome-brightgreen) ![status](https://img.shields.io/badge/status-prototype-red)

</div>

## Why LichenVM
While countless DSL is invented for ML kernel these years, static analysis on them are almostly unimplemented, or not integrated into language services.

Some embeded-DSL tries relying on host-language's type system to encode its static analysis, which usually brings much slower analysis speed, and completely different runtime integration.

All this leads to an idea: we have LLVM for compilers, now we need "LLVM" for static analyzers: "LichenVM", that is modular, layerd and composable, gradually add more checking property to original runtime program.


## Features
- **Unified Runtime**
  
  Value computing, type checking and more, all encoded into a unified runtime compute graph: type can be computed, value can depend on type.  

- **Modular analysis property**
  
  Value is a property of expression, so do types, they are defined by plugins, can be extended by down-stream plugins, and more property like visibility, can be defined.

- **zero cost plugin system**
  
  The plugin system use enum dispach for extendable concept like value, operator, which is implemented via code generation.

## Start Developing
Still in prototyping, check the tests.