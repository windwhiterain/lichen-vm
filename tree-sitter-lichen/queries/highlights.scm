; highlight queries for Lichen (tree-sitter-lichen)
;
; Note: the same rules are mirrored in the Zed extension at
; lichen-language-zed/languages/lichen/highlights.scm because Zed reads
; queries from the extension's language directory, not from the grammar repo.
; Keep the two in sync.

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
(string_literal) @string
(placeholder) @variable
(type_constant) @type.builtin

; named fields (`.name` reads, struct field / argument names)
(field_read name: (identifier) @property)
(struct_field (identifier) @property)
(parenthesized name: (identifier) @property)

; bindings / definitions
(binding name: (identifier) @variable)

; keyword-like tokens (anonymous)
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

; operators (anonymous).  The full set is the language's, docs/notes/operators.md
; §1: `+ - * / %`, the six comparisons, and `& | ^`.
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
