; highlight queries for Lichen (tree-sitter-lichen)
;
; Note: the same rules are mirrored in the Zed extension at
; crates/lichen-language-zed/languages/lichen/highlights.scm because Zed reads
; queries from the extension's language directory, not from the grammar repo.
; Keep the two in sync.

; The `@{ ... @}` preprocessor block is Lichen's only "prose" home (doc
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

; operators (anonymous)
"->" @operator
"=>" @operator
"::" @operator
":" @operator
"#" @operator
"!" @operator
"$" @operator
"==" @operator
"<=" @operator
"+" @operator
"-" @operator
"=" @operator
"." @operator
