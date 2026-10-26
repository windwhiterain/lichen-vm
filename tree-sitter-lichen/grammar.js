// tree-sitter grammar for the Lichen language.
//
// The compiler's frontend is strict (see `crates/lichen-language-parser`);
// this grammar is the *editor* face and is deliberately simple and permissive:
// it exposes structure for highlighting, outline and bracket matching, and it
// accepts anything the frontend accepts.  Grammar: `docs/language-spec.md` §2,
// the precedence ladder `docs/notes/operators.md` §3.
//
// Postfix forms (`(k)`, `[i]`, `{…}`) exist only when *glued* to the
// preceding token (`docs/language-spec.md` §2.1), so the grammar spells the
// glued delimiter `token.immediate`; a spaced delimiter is a fresh atom and so
// an argument of `application`.
//
// The value templates (`docs/notes/value-templates.md`): `Tuple`, `Array` and
// `Struct` are the builtin templates, `[template, length]` is an array
// template, `template[…]` is an array instance, `{.a e}` a struct value and
// `S{.a e}` a struct instance.  The retired lowercase type forms
// (`tuple{…}`, `struct{…}`, `array{T, n}`) and the `C(…)` instantiation no
// longer parse.
//
// Run `tree-sitter generate` in this directory after editing.

// One ladder level -> one rule, so the tree says which level a node came from
// and each level carries its own precedence.  `left`/`right`/`operator` are
// the fields the highlight query reads.
const binaryLevel = (precedence, operators) => $ => prec.left(precedence, seq(
  field('left', $.expression),
  field('operator', operators.length === 1 ? operators[0] : choice(...operators)),
  field('right', $.expression),
));

// The binary ladder, loosest to tightest — the same order `parse.rs` builds:
// `docs/notes/operators.md` §3.  The bitwise trio keeps C's nesting (`&` in `^`
// in `|`) and sits tighter than a comparison, so `a & b == c` is
// `(a & b) == c`.  Every level is left-associative.
const PREC = {
  lambda: 1,
  annotation: 2,
  unify: 3,
  table_find: 5,
  comparison: 6,
  bitwise_or: 7,
  bitwise_xor: 8,
  bitwise_and: 9,
  addition: 10,
  product: 11,
  prefix: 12,
  application: 13,
};

module.exports = grammar({
  name: 'lichen',

  // Whitespace (space/tab/cr) is trivia.  Newlines are *not* trivia: the
  // language uses a newline/comma/semicolon as a uniform statement boundary,
  // so a newline is a real `separator` token.
  extras: $ => [/[ \t\r]+/],

  word: $ => $.identifier,

  rules: {
    // -- top level ---------------------------------------------------------
    source_file: $ => seq(
      optional($.preprocess_block),
      optional($.separator),
      optional($.statements),
    ),

    // -- the --- ... --- preprocessor block ---------------------------------
    // The delimiter is symmetric, so the two ends are the same token and are
    // told apart by alias rather than by spelling.
    preprocess_block: $ => seq(
      alias('---', 'block_open'),
      optional($.separator),
      repeat(seq($.pp_entry, optional($.separator))),
      alias('---', 'block_close'),
    ),

    pp_entry: $ => choice(
      seq(field('name', $.identifier), '=', 'import', field('path', $.string_literal)),
      seq(field('name', $.identifier), '=', field('value', $.string_literal)),
    ),

    // -- statements --------------------------------------------------------
    statements: $ => seq(
      $.statement,
      repeat(seq(repeat1($.separator), $.statement)),
      repeat($.separator),
    ),

    statement: $ => choice($.binding, $.expression),

    // `name := expr` — the binding.  The `=` of a unify is an expression, not
    // a binding (`docs/notes/value-templates.md`).
    binding: $ => seq(
      optional(token(prec(1, '@loop'))),
      optional(token(prec(1, '@cache'))),
      optional('let'),
      field('name', $.identifier),
      ':=',
      field('value', $.expression),
    ),

    separator: $ => choice(',', ';', '\n'),

    // -- expressions -------------------------------------------------------
    // The ladder, loosest to tightest: `expression` is every form.
    expression: $ => choice(
      $.lambda,
      $.annotation,
      $.unify,
      $.table_read,
      $.binary_comparison,
      $.binary_or,
      $.binary_xor,
      $.binary_and,
      $.binary_addition,
      $.binary_product,
      $.prefix,
      $.application,
    ),

    // `param => body` (right-assoc).  The parameter is a full expression, so
    // a typed param `x : T => e` reads the `: T` annotation as the parameter.
    lambda: $ => prec.right(PREC.lambda, seq(
      field('parameter', $.expression),
      '=>',
      field('body', $.expression),
    )),

    // `e : T`, `e # p`, `e ! r`, `e ? d` and `e ~ "payload"` — the annotation
    // chain, nested right, so `e : T # p` annotates the annotation.  A chain
    // with no value (`: T # a`) annotates the wildcard `@any`, which is how a
    // compound slot leaves its value position open
    // (`docs/notes/compound-element-slots.md`).  A mark's payload is text,
    // never an expression (`docs/language-spec.md` §2, marks).
    //
    // The value and the pieces' right sides sit at the unify level, so the
    // pieces chain flat (`a : b : c` is two pieces of one annotation) and an
    // annotation is never the value of another.
    annotation: $ => prec.right(PREC.annotation, seq(
      field('value', $.expression),
      repeat1($._annotation_piece),
    )),

    _annotation_piece: $ => choice(
      seq(field('operator', choice(':', '#', '?', '!')), field('annotation', $.expression)),
      seq(field('operator', '~'), field('payload', $.string_literal)),
    ),

    // `actual = expected` — a plain unify; `actual <: template` — a template
    // match.  One level, left-associative, and it binds looser than the
    // annotation (`docs/notes/value-templates.md`).
    unify: $ => prec.left(PREC.unify, seq(
      field('left', $.expression),
      field('operator', choice('=', '<:')),
      field('right', $.expression),
    )),

    // `t <== k` — the table read: the loosest binary level, left-associative,
    // so its key is a whole comparison expression (`docs/notes/operators.md` §4).
    table_read: $ => prec.left(PREC.table_find, seq(
      field('container', $.expression),
      field('operator', '<=='),
      field('key', $.expression),
    )),

    // The computational operators (see docs/notes/operators.md).  A comparison
    // yields the `0`/`1` scalar — there is no `Bool` — and `&`/`|`/`^` over
    // two of those are the language's and/or/xor.  `@in` is membership and
    // sits on the comparison rung: it yields the same `0`/`1`.
    binary_comparison: binaryLevel(PREC.comparison, ['<', '>', '<=', '>=', '==', '!=', '@in']),
    binary_or: binaryLevel(PREC.bitwise_or, ['|']),
    binary_xor: binaryLevel(PREC.bitwise_xor, ['^']),
    binary_and: binaryLevel(PREC.bitwise_and, ['&']),
    binary_addition: binaryLevel(PREC.addition, ['+', '-']),
    binary_product: binaryLevel(PREC.product, ['*', '/', '%']),

    // The prefix level: `@assert e`, `@gc e`, the one-operand keywords
    // and the two two-operand ones.  A one-operand keyword takes an application
    // (`int2float f x` converts `f x`); a two-operand one takes one atom each
    // (`docs/notes/operators.md` §3, §11).
    prefix: $ => prec(PREC.prefix, choice(
      seq(choice('@assert', '@gc'), field('value', $.application)),
      seq(
        field('operator', choice('int2float', 'float2int', 'exp', 'sqrt', 'floor')),
        field('value', $.application),
      ),
      seq(
        field('operator', choice('max', 'min')),
        field('left', $._atom),
        field('right', $._atom),
      ),
    )),

    // Juxtaposition: one or more atoms, left-associative.  The left
    // associativity is what lets a spaced delimiter open a fresh atom (an
    // argument) where a glued one is a postfix.
    application: $ => prec.left(PREC.application, repeat1($._atom)),

    // An atom is a base form followed by glued postfix forms
    // (`docs/language-spec.md` §2.1).
    _atom: $ => prec.left(seq($._base, repeat($._postfix))),

    _base: $ => choice(
      $.identifier,
      $.integer,
      $.float,
      $.string_literal,
      $.placeholder,
      $.wildcard,
      $.type_constant,
      $.parenthesized,
      $.array_template,
      $.struct_value,
      $.block,
      $.if_expression,
      $.native_call,
      $.set_type,
      $.table_type,
      $.table_value,
      $.set_value,
    ),

    _postfix: $ => choice(
      $.field_read,
      $.index,
      $.instance,
      $.call,
      $.struct_instance,
    ),

    // `a.name` — a named field read.
    field_read: $ => seq('.', field('name', $.identifier)),

    // `a[i]` — an element read; the `[` must be glued, else it opens an array
    // template.
    index: $ => seq(token.immediate('['), field('index', $.expression), ']'),

    // `T[e1, e2]` — an instance of the array template `T`: the elements are
    // written out, so a separator follows the first one.  The separator-free
    // one-element form is the read above.
    instance: $ => seq(
      token.immediate('['),
      optional(seq(
        $.shallow_element,
        $.separator,
        optional(seq(
          $.shallow_element,
          repeat(seq($.separator, $.shallow_element)),
          optional($.separator),
        )),
      )),
      ']',
    ),

    // `a(k)` — a positional slot read: one expression in a glued paren.  The
    // retired `C(…)` instantiation is gone (`docs/notes/value-templates.md`).
    call: $ => seq(
      token.immediate('('),
      field('key', $.expression),
      ')',
    ),

    // -- atoms & literals --------------------------------------------------
    identifier: $ => /[A-Za-z_][A-Za-z0-9_]*/,
    integer: $ => /[0-9]+/,
    float: $ => /[0-9]+\.[0-9]+/,
    string_literal: $ => /"[^"]*"?/,
    placeholder: $ => '_',
    // `@any` — the wildcard literal, which unifies with everything.  The `@`
    // sigil is also a bare-word token (an unknown `@word` is a lex error), so
    // each sigil keyword outranks it by token precedence.
    wildcard: $ => token(prec(1, '@any')),

    // The type constants, the builtin templates among them: `Tuple`, `Array`
    // and `Struct` are values, not spellings of a type
    // (`docs/notes/value-templates.md`).
    type_constant: $ => choice('Int', 'Float', 'string', 'Type', 'Tuple', 'Array', 'Struct'),

    // `( e )` grouping / `(e1, e2, ...)` tuple value.
    parenthesized: $ => seq(
      '(',
      optional(seq(
        $.expression,
        repeat(seq($.separator, $.expression)),
        optional($.separator),
      )),
      ')'
    ),

    // `[template, length]` — the array template: the element template and the
    // array's length.  An array *value* is an instance of a template, written
    // `template[…]`.
    array_template: $ => seq(
      '[',
      $.shallow_element,
      $.separator,
      $.shallow_element,
      optional($.separator),
      ']',
    ),

    // `@shallow e` marks the whole element shallow, `@shallow(2) e` the first
    // two levels.  The depth's parens are **glued**, so a spaced `(` is the
    // element itself and never a depth (`docs/language-spec.md` §2, shallow).
    shallow_element: $ => seq(
      optional(seq('@shallow', optional(seq(token.immediate('('), $.integer, ')')))),
      field('element', $.expression),
    ),

    // `{.a e, …}` — a struct value: the fields carry their own names.
    struct_value: $ => seq(
      '{',
      $.struct_field,
      repeat(seq($.separator, $.struct_field)),
      optional($.separator),
      '}',
    ),

    // `S{.a e, …}` — a struct instance: the callee is the struct's template,
    // and the brace is glued to it.
    struct_instance: $ => seq(
      token.immediate('{'),
      optional(seq(
        $.struct_field,
        repeat(seq($.separator, $.struct_field)),
        optional($.separator),
      )),
      '}',
    ),

    // `.name e` — one field of a struct value or a struct instance.
    struct_field: $ => seq(
      '.',
      field('name', $.identifier),
      field('value', $.expression),
    ),

    // `set{E}` — the set type, whose one element is the element **slot**.
    set_type: $ => seq(
      'set',
      '{',
      field('element', $.expression),
      optional($.separator),
      '}',
    ),

    // `table{K, V}` — the table type: the key slot, then the value slot.
    table_type: $ => seq(
      'table',
      '{',
      field('key', $.expression),
      $.separator,
      field('value', $.expression),
      optional($.separator),
      '}',
    ),

    // `table(k ==> v, ...)` — a constant table value.
    table_value: $ => seq(
      'table',
      '(',
      optional(seq(
        $.table_entry,
        repeat(seq($.separator, $.table_entry)),
        optional($.separator),
      )),
      ')',
    ),

    table_entry: $ => seq(
      field('key', $.expression),
      '==>',
      field('value', $.expression),
    ),

    // `set(a, b, ...)` — a set of ordinary values, parenthesised like the
    // table value (the braced `set{…}` is the type spelling).
    set_value: $ => seq(
      'set',
      '(',
      optional(seq(
        field('member', $.expression),
        repeat(seq($.separator, field('member', $.expression))),
        optional($.separator),
      )),
      ')',
    ),

    // `{ stmt; ...; expr }` block.  A statement may be `pub`-marked (a block
    // struct field); an explicit `return expr` anywhere is the block's tail.
    block: $ => seq(
      '{',
      repeat($.separator),
      optional(seq(
        $.block_stmt,
        repeat(seq(repeat1($.separator), $.block_stmt)),
        repeat($.separator),
      )),
      '}',
    ),

    // A block-body item.  The language only permits `pub`/`return` inside a
    // block, so they are reserved here (not at the top level).
    block_stmt: $ => choice(
      seq('return', field('value', $.expression)),
      seq(optional('pub'), $.statement),
    ),

    // `if <cond> then <then> else <else>`.
    if_expression: $ => seq(
      'if',
      field('condition', $.expression),
      'then',
      field('then_branch', $.expression),
      'else',
      field('else_branch', $.expression),
    ),

    // `$name(args...)` native call.
    native_call: $ => seq(
      '$',
      field('operator', $.identifier),
      '(',
      optional(seq($.expression, repeat(seq($.separator, $.expression)), optional($.separator))),
      ')'
    ),
  },
});
