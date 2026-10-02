; Tree-sitter highlights for Lichen (mirrors tree-sitter-lichen/queries/highlights.scm).
;
; Zed reads queries from the extension's `languages/<lang>/` directory, not from
; the grammar repo — so this file must stay in sync with the grammar's queries.

; The `@{ ... @}` preprocessor block is Lichen's only "prose" home (doc strings,
; metadata); treat the whole block as a comment.
(preprocess_block) @comment

; metadata keys / names inside the preprocessor block
(pp_entry name: (identifier) @property)
(pp_entry path: (string_literal) @string)
(pp_entry value: (string_literal) @string)

; literals
(identifier) @variable
(integer) @number
(string_literal) @string
(placeholder) @variable
(type_constant) @type.builtin

; named fields (`.name` reads, struct field / argument names)
(field_read name: (identifier) @property)
(struct_field (identifier) @property)
(parenthesized name: (identifier) @property)

; bindings / definitions
(binding name: (identifier) @variable)

; keyword-like tokens
"if" @keyword
"then" @keyword
"else" @keyword
"let" @keyword
"struct" @keyword
"table" @keyword
"import" @keyword
"return" @keyword
"pub" @keyword
"cache" @keyword
(type_of) @keyword

; operators.  The full set is the language's, docs/notes/operators.md §1:
; `+ - * / %`, the six comparisons, and `& | ^`.
"->" @operator
"=>" @operator
"::" @operator
":" @operator
"#" @operator
"?" @operator
"!" @operator
"$" @operator
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

; `<` and `>` are the one pair this grammar cannot list by name: the same two
; characters open and close every angle form (`<a, b>`, `struct<…>`, `X<e>`)
; and are the order comparisons.  Capturing them through the comparison's
; `operator` field is what keeps a type's own brackets out of the operator
; colour — `x : <Int, Int>` has no operator in it.
(binary_comparison operator: "<" @operator)
(binary_comparison operator: ">" @operator)
