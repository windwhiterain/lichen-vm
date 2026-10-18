// tree-sitter grammar for the Lichen language.
//
// The compiler's frontend is strict (see `crates/lichen-language-parser`);
// this grammar is the *editor* face and is deliberately simple and permissive:
// it exposes structure for highlighting, outline and bracket matching, and it
// accepts anything the frontend accepts.  Grammar: `docs/language-spec.md` §2,
// the precedence ladder `docs/notes/operators.md` §3.
//
// Postfix forms (`(k)`, `[i]`, `{n}`, `::a`) exist only when *glued* to the
// preceding token (`docs/language-spec.md` §2.1), so the grammar spells the
// glued delimiter `token.immediate`; a spaced delimiter is a fresh atom and so
// an argument of `application`.
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
  arrow: 3,
  table_find: 4,
  comparison: 5,
  bitwise_or: 6,
  bitwise_xor: 7,
  bitwise_and: 8,
  addition: 9,
  product: 10,
  prefix: 11,
  application: 12,
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
      repeat(seq($.separator, $.statement)),
      optional($.separator),
    ),

    statement: $ => choice($.binding, $.expression),

    binding: $ => seq(
      optional(token(prec(1, '@loop'))),
      optional(token(prec(1, '@cache'))),
      optional('let'),
      field('name', $.identifier),
      '=',
      field('value', $.expression),
    ),

    separator: $ => choice(',', ';', '\n'),

    // -- expressions -------------------------------------------------------
    // The ladder, loosest to tightest: `expression` is every form.
    expression: $ => choice(
      $.lambda,
      $.annotation,
      $.arrow,
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
    annotation: $ => prec.right(PREC.annotation, choice(
      seq(
        field('value', $.expression),
        repeat1($._annotation_piece),
      ),
      repeat1($._annotation_piece),
    )),

    _annotation_piece: $ => choice(
      seq(field('operator', choice(':', '#', '?', '!')), field('annotation', $.expression)),
      seq(field('operator', '~'), field('payload', $.string_literal)),
    ),

    // `A -> B` — a function type, right-associative.
    arrow: $ => prec.right(PREC.arrow, seq(
      field('parameter', $.expression),
      '->',
      field('return_type', $.expression),
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

    // The prefix level: `@assert e`, the four one-operand keywords and the two
    // two-operand ones.  A one-operand keyword takes an application
    // (`int2float f x` converts `f x`); a two-operand one takes one atom each
    // (`docs/notes/operators.md` §3, §11).
    prefix: $ => prec(PREC.prefix, choice(
      seq('@assert', field('value', $.application)),
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
      $.array,
      $.block,
      $.if_expression,
      $.native_call,
      $.tuple_type,
      $.struct_type,
      $.array_type,
      $.set_type,
      $.table_type,
      $.table_value,
      $.set_value,
    ),

    _postfix: $ => choice(
      $.field_read,
      $.index,
      $.type_access,
      $.named_type_access,
      $.call,
    ),

    // `a.name` — a named field read.
    field_read: $ => seq('.', field('name', $.identifier)),

    // `a[i]` — an index; the `[` must be glued, else it opens an array atom.
    index: $ => seq(token.immediate('['), field('index', $.expression), ']'),

    // `X{n}` — a type access: component `n` of a tuple type value's value.
    type_access: $ => seq(token.immediate('{'), field('index', $.expression), '}'),

    // `X::a` — a named type access over a TypeStruct value.
    named_type_access: $ => seq(token.immediate('::'), field('name', $.identifier)),

    // `a(k)` — a positional slot read, or `C(.x 1, …)` / `C(1,)` — an
    // instantiation; a single bare expression in a glued paren is the read.
    call: $ => seq(
      token.immediate('('),
      optional(seq(
        $._argument,
        repeat(seq($.separator, $._argument)),
        optional($.separator),
      )),
      ')',
    ),

    _argument: $ => choice(
      seq('.', field('name', $.identifier), field('value', $.expression)),
      field('value', $.expression),
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

    type_constant: $ => choice('Int', 'Float', 'string', 'Type'),

    // `( e )` grouping / `(e1, e2, ...)` tuple value.  An element may be a
    // named struct-field argument `.field value` (struct instantiation / field
    // read) or a bare expression; the grammar is lenient and accepts either
    // everywhere.
    parenthesized: $ => seq(
      '(',
      optional(seq(
        $._paren_element,
        repeat(seq($.separator, $._paren_element)),
        optional($.separator),
      )),
      ')'
    ),

    _paren_element: $ => choice(
      seq('.', field('name', $.identifier), field('value', $.expression)),
      field('value', $.expression),
    ),

    // `[e1, e2, ...]` array literal.  A position may carry the `@shallow`
    // marker; a bare `expression` is the unmarked case of `shallow_element`.
    array: $ => seq(
      '[',
      optional(seq(
        $.shallow_element,
        repeat(seq($.separator, $.shallow_element)),
        optional($.separator),
      )),
      ']'
    ),

    // `@shallow e` marks the whole element shallow, `@shallow(2) e` the first
    // two levels.  The depth's parens are **glued**, so a spaced `(` is the
    // element itself and never a depth (`docs/language-spec.md` §2, shallow).
    shallow_element: $ => seq(
      optional(seq('@shallow', optional(seq(token.immediate('('), $.integer, ')')))),
      field('element', $.expression),
    ),

    // `tuple{slot, ...}` — the tuple type: one slot per element, keyword-led
    // because the braced form is a *type* and `(e, …)` is the tuple value.
    tuple_type: $ => seq(
      'tuple',
      '{',
      $.tuple_slot,
      repeat(seq($.separator, $.tuple_slot)),
      optional($.separator),
      '}',
    ),

    // `value : type` — a tuple type's slot: the value position pinned by a
    // top-level `:`, or a bare type whose value is the wildcard.
    tuple_slot: $ => prec(1, choice(
      seq(
        field('value', $._atom),
        ':',
        field('type', $.expression),
      ),
      field('type', $.expression),
    )),

    // `struct{.name T, ...}` — a nominal struct type (the field list is
    // optional, `struct{}`).
    struct_type: $ => seq(
      'struct',
      '{',
      optional(seq(
        $.struct_field,
        repeat(seq($.separator, $.struct_field)),
        optional($.separator),
      )),
      '}',
    ),

    struct_field: $ => seq(
      optional(seq('.', field('name', $.identifier))),
      field('type', $.expression),
    ),

    // `array{T, n}` — the array type: the element type, then the length.
    array_type: $ => seq(
      'array',
      '{',
      field('element', $.expression),
      $.separator,
      field('length', $.expression),
      optional($.separator),
      '}',
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
      optional($.separator),
      optional(seq(
        $.block_stmt,
        repeat(seq($.separator, $.block_stmt)),
        optional($.separator),
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
