# Foreword

lichen started as an experiment. Could a type checker be *built on a
runtime* rather than beside it — with types as ordinary values, and
checking as the same unification that the runtime already does? The
answer turned out to be yes, and the experiment grew a language, a
compiler, a package manager, a language server, and a compute layer that
runs kernels on CPUs and GPUs.

That history is why lichen looks the way it does. It has no `main`, no
mutable variables, no comments, and no booleans; it has one universe
that closes in a cycle at `Type`, and a function whose type is the
function itself. Each of those is a consequence of the same idea, not a
list of quirks to memorize, and this book teaches the idea first and the
consequences after.

The book is deliberately written the way the Rust book is written: one
concept at a time, a complete program in every chapter, and real output
rather than sketches of output. Every program in these pages was run
against the compiler in this repository, and the output shown is what
the compiler printed. lichen is still young and still moving; when this
book and the compiler disagree, the compiler wins, and the book is what
needs fixing.

*— the lichen contributors*
