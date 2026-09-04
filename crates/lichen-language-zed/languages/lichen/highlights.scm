; Tree-sitter highlights for Lichen.
;
; DRAFT — the `tree-sitter-lichen` grammar does not exist yet, so these node
; names are provisional. Adjust the node names to match the grammar once it is
; authored. This file is only used when `[grammars.lichen]` is registered in
; `extension.toml`.

; Literals
(number) @number
(string) @string
; (boolean) @boolean

; Names
(identifier) @variable
(parameter) @variable.parameter
(type) @type

; Comments / directives
(comment) @comment
(preprocessor) @preproc

; Keywords / operators
(keyword) @keyword
(operator) @operator
