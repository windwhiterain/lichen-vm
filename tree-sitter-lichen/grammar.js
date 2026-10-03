// tree-sitter grammar for the Lichen language.
//
// Design goal: *simple and permissive*.  Lichen's real frontend is a
// strict, correctness-first type-checked parser with whitespace-sensitive
// postfix forms ("Glue").  A tree-sitter grammar only needs to (a) highlight
// and (b) expose a little structure for outline / bracket-matching, so this
// grammar intentionally:
//   - does NOT model the Glue (adjacency) distinction for `( ... )`,
//     `[ ... ]` and `{ ... }` — the same delimiter may be read as postfix or
//     as a fresh atom; GLR resolves it without rejecting valid code;
//   - treats a single `( ... )`/`[ ... ]`/`< ... >`/`{ ... }` as an atom
//     everywhere, so it accepts both meanings;
//   - keeps the operator precedence ladder but is happy to accept any
//     atom-juxtaposition as "application".
//
// The one Glue rule it *does* model is the `<` that opens an angle form, and it
// has to: the real language reads a glued `X<e>` as an index and a spaced
// `a < b` as a comparison, and the comparison operators are new here.  See
// `angle_tuple` — it is a second, glued `<` token, so the lexer settles it.
//
// The preprocessor block `--- name = "value" | name = import "path" ... ---`
// is the only "comment-like" construct; it is parsed as its own node so doc
// strings can be highlighted.
//
// Run `tree-sitter generate` in this directory after editing.

// The binary ladder, loosest to tightest — the same order `parse.rs` builds
// and language-spec §2 states.  The bitwise trio keeps C's nesting (`&` in
// `^` in `|`) and sits *tighter* than a comparison, so `a & b == c` is
// `(a & b) == c`.  Every level is left-associative, so `1 < 2 == 1` is
// `(1 < 2) == 1` and `8 / 4 / 2` is `(8 / 4) / 2`.
const PREC = {
  lambda: 1,
  annotation: 2,
  arrow: 3,
  comparison: 4,
  bitwise_or: 5,
  bitwise_xor: 6,
  bitwise_and: 7,
  addition: 8,
  product: 9,
  assertion: 10,
  application: 11,
};

// One ladder level -> one rule, so the tree says which level a node came from
// and each level carries its own precedence.  `left`/`right` and `operator` are
// the fields the highlight query and an editor's cursor motions read.
const binaryLevel = (level, operators) => $ => prec.left(PREC[level], seq(
  field('left', $.expression),
  field('operator', operators.length === 1 ? operators[0] : choice(...operators)),
  field('right', $.expression),
));

module.exports = grammar({
  name: 'lichen',

  // Whitespace (space/tab/cr) is trivia.  Newlines are *not* trivia: the
  // language uses a newline/comma/semicolon as a uniform statement boundary,
  // so newline is a real `separator` token.
  extras: $ => [/[ \t\r]+/],

  word: $ => $.identifier,

  rules: {
    // -- top level ---------------------------------------------------------
    source_file: $ => seq(
      optional($.preprocess_block),
      optional($.separator),
      optional($.statements),
    ),

    // -- the --- ... --- preprocessor block ----------------------------------
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
      optional('cache'),
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
      $.binary_comparison,
      $.binary_or,
      $.binary_xor,
      $.binary_and,
      $.binary_addition,
      $.binary_product,
      $.assert_expression,
      $.application,
    ),

    // Every form but a comparison — what goes *inside* an angle form.  The `>`
    // that closes an angle form is also the comparison operator, and an LR
    // table has to decide at that one token: the real parser decides by
    // re-reading the element it has just parsed and finding no operand to
    // continue the comparison with.  A comparison is never a type and never an
    // index, so leaving it out of the element leaves the `>` unambiguously the
    // closing bracket, and `<a, b, c>` stays a three-element tuple type.
    //
    // The `prec` is not part of the binding ladder: an element and a plain
    // `expression` are otherwise the same reduction, so the closing `>` would
    // have two readings.  Weighting the element is what picks the one the
    // source means.
    _angle_element: $ => prec(1, choice(
      $.lambda,
      $.annotation,
      $.arrow,
      $.binary_or,
      $.binary_xor,
      $.binary_and,
      $.binary_addition,
      $.binary_product,
      $.assert_expression,
      $.application,
    )),

    // `param => body` (right-assoc).  The parameter is a full expression, so
    // a typed param `x : T => e` reads the `: T` annotation as the parameter.
    lambda: $ => prec.right(PREC.lambda, seq(
      field('parameter', $.expression),
      '=>',
      field('body', $.expression),
    )),

    annotation: $ => prec.right(PREC.annotation, seq(
      field('value', $.expression),
      field('operator', choice(':', '#', '?')),
      field('annotation', $.expression),
    )),

    arrow: $ => prec.right(PREC.arrow, seq(
      field('parameter', $.expression),
      '->',
      field('return_type', $.expression),
    )),

    // The computational operators (see docs/notes/operators.md).  A comparison
    // yields the `0`/`1` scalar — there is no `Bool` — and `&`/`|`/`^` over
    // two of those are the language's and/or/xor.
    binary_comparison: binaryLevel('comparison', ['<', '>', '<=', '>=', '==', '!=']),
    binary_or: binaryLevel('bitwise_or', ['|']),
    binary_xor: binaryLevel('bitwise_xor', ['^']),
    binary_and: binaryLevel('bitwise_and', ['&']),
    binary_addition: binaryLevel('addition', ['+', '-']),
    binary_product: binaryLevel('product', ['*', '/', '%']),

    // `!` is a prefix assert over an application (juxtaposed atoms).  Keeping
    // its operand at the application level (rather than a full expression)
    // avoids the `! expr =>` ambiguity with lambdas; `!(x => e)` still works
    // because the lambda sits inside a parenthesized atom.
    assert_expression: $ => prec(PREC.assertion, seq('!', field('value', $.application))),

    // Juxtaposition: one or more atoms, left-associative.  The left
    // associativity is what settles the one decision an LR table cannot
    // postpone: after a complete application, a *spaced* `<` is the
    // comparison, so the fresh-atom reading of an angle form is only reachable
    // from a *glued* `<` (`angle_tuple`'s second opening).  A tuple type
    // applied to a bare atom therefore has to be written glued (`f<A, B>`), as
    // every occurrence in this repository already writes it.
    application: $ => prec.left(PREC.application, repeat1($._atom)),

    // An atom is a base form followed by postfix forms.  Only the
    // unambiguous `.name` field read and the `::name` raw named read are
    // postfixes; every `[`/`(`/`{` — and a glued `<` — is read as a *fresh*
    // atom (and handled by application juxtaposition), so the real parser's
    // whitespace-sensitive "Glue" distinction stays deliberately glossed over
    // everywhere but the one place it decides an operator (see `angle_tuple`).
    _atom: $ => prec.left(seq($._base, repeat($._postfix))),

    _base: $ => choice(
      $.identifier,
      $.integer,
      $.string_literal,
      $.placeholder,
      $.type_constant,
      $.parenthesized,
      $.array,
      $.angle_tuple,
      $.struct_type,
      $.table_literal,
      $.block,
      $.if_expression,
      $.native_call,
    ),

    _postfix: $ => field('field', choice($.field_read, $.raw_field_read)),

    field_read: $ => seq('.', field('name', $.identifier)),

    // `X::a` — the raw named read over a TypeStruct value.
    raw_field_read: $ => seq('::', field('name', $.identifier)),

    // -- atoms & literals --------------------------------------------------
    identifier: $ => /[A-Za-z_][A-Za-z0-9_]*/,
    integer: $ => /[0-9]+/,
    string_literal: $ => /"[^"]*"?/,
    placeholder: $ => '_',

    type_constant: $ => choice('Int', 'string', 'Type'),

    // `( e )` grouping / `(e1, e2, ...)` tuple.  An element may be a named
    // struct-field argument `.field value` (struct instantiation / field
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

    _paren_element: $ => seq(
      optional(seq('.', field('name', $.identifier))),
      field('value', $.expression),
    ),

    // `[e1, e2, ...]` array literal.  Elements may be `~`-marked; a bare
    // `expression` is the no-`~` case of `tilde_element`.
    array: $ => seq(
      '[',
      optional(seq(
        $.tilde_element,
        repeat(seq($.separator, $.tilde_element)),
        optional($.separator),
      )),
      ']'
    ),

    tilde_element: $ => seq(optional(/\~[0-9]*/), field('element', $.expression)),

    // `<e1, e2, ...>` — the tuple type, the same lenient one-or-more shape the
    // grammar has always had, so `X<e>`, `array<Int, 3>` and `x : <Int, Int>`
    // all stay an application of an atom to an angle form.
    //
    // It takes two openings, and that is the whole of the `<` disambiguation:
    // the *glued* one (`token.immediate`) is a different token from the spaced
    // one, so the lexer alone tells them apart — no lookahead, no GLR fork.  A
    // glued `<` can only be an angle form and a spaced one only a comparison
    // (see `application`), which is the real language's own rule.  The price
    // is the one form this reads as a comparison and the real parser reads as
    // an application: a *spaced* tuple type after a bare atom, as in
    // `f <Int, Type>`.
    angle_tuple: $ => seq(
      choice(token.immediate('<'), '<'),
      $._angle_element,
      repeat(seq($.separator, $._angle_element)),
      optional($.separator),
      '>',
    ),

    // `struct<T1, ..., Tn>`.
    struct_type: $ => seq(
      'struct',
      '<',
      $.struct_field,
      repeat(seq($.separator, $.struct_field)),
      optional($.separator),
      '>'
    ),

    struct_field: $ => seq(
      optional(seq('.', $.identifier)),
      field('type', $._angle_element),
    ),

    // `table { k1 ==> v1, ... }`.
    table_literal: $ => seq(
      'table',
      optional($.separator),
      '{',
      optional(seq(
        $.table_entry,
        repeat(seq($.separator, $.table_entry)),
        optional($.separator),
      )),
      '}'
    ),

    table_entry: $ => seq(
      field('key', $.expression),
      '==>',
      field('value', $.expression),
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
