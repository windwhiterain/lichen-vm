; Tree-sitter highlights for Lichen (mirrors tree-sitter-lichen/queries/highlights.scm).
;
; Zed reads queries from the extension's `languages/<lang>/` directory, not from
; the grammar repo — so this file must stay in sync with the grammar's queries.

; The `--- ... ---` preprocessor block is Lichen's only "prose" home (doc
; strings, metadata); treat the whole block as a comment.
(preprocess_block) @comment

; The metadata keys / names inside the preprocessor block.
(pp_entry name: (identifier) @property)
(pp_entry path: (string_literal) @string)
(pp_entry value: (string_literal) @string)

; literals
(identifier) @variable
(integer) @number
(float) @number
(string_literal) @string
(placeholder) @variable
(type_constant) @type.builtin

; named fields (`.name` reads, struct field / argument names)
(field_read name: (identifier) @property)
(struct_field name: (identifier) @property)
(call name: (identifier) @property)
(parenthesized name: (identifier) @property)

; bindings / definitions
(binding name: (identifier) @variable)

; keyword-like tokens (anonymous)
"if" @keyword
"then" @keyword
"else" @keyword
"let" @keyword
"pub" @keyword
"@cache" @keyword
"import" @keyword
"return" @keyword
"@loop" @keyword
"@assert" @keyword
"@in" @keyword
"@shallow" @keyword

; `@any` is the `wildcard` node: the query syntax reads a `"@…"` string as a
; capture name, so the literal is reached by its node rather than by its text.
(wildcard) @constant.builtin

; the type-led forms (`docs/language-spec.md` §2.1): `tuple{…}`, `struct{…}`,
; `array{…}`, `set{…}`, `table{…, …}` are types, `set(…)` and `table(…)` values
"tuple" @keyword.type
"struct" @keyword.type
"array" @keyword.type
"set" @keyword.type
"table" @keyword.type

; the prefix keyword operators (`docs/notes/operators.md` §2, §11)
"int2float" @keyword.operator
"float2int" @keyword.operator
"max" @keyword.operator
"min" @keyword.operator
"exp" @keyword.operator
"sqrt" @keyword.operator
"floor" @keyword.operator

; operators (anonymous).  The full set is the language's, docs/notes/operators.md
; §1: `+ - * / %`, the six comparisons, the bitwise trio and `@in`.
"->" @operator
"=>" @operator
"==>" @operator
"<==" @operator
"::" @operator
":" @operator
"#" @operator
"?" @operator
"!" @operator
"~" @operator
"$" @operator
"<" @operator
">" @operator
"<=" @operator
">=" @operator
"==" @operator
"!=" @operator
"|" @operator
"^" @operator
"&" @operator
"+" @operator
"-" @operator
"*" @operator
"/" @operator
"%" @operator
"=" @operator
"." @operator
