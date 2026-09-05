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
(type_of) @keyword

; operators
"->" @operator
"=>" @operator
"::" @operator
":" @operator
"#" @operator
"?" @operator
"!" @operator
"$" @operator
"==" @operator
"<=" @operator
"+" @operator
"-" @operator
"=" @operator
"." @operator
