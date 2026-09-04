// tree-sitter grammar for the Lichen language.
//
// Design goal: *simple and permissive*.  Lichen's real frontend is a
// strict, correctness-first type-checked parser with whitespace-sensitive
// postfix forms ("Glue").  A tree-sitter grammar only needs to (a) highlight
// and (b) expose a little structure for outline / bracket-matching, so this
// grammar intentionally:
//   - does NOT model the Glue (adjacency) distinction between postfix and
//     spacing — the same delimiter may be read as postfix or as a fresh
//     atom; GLR resolves it without rejecting valid code;
//   - treats a single `( ... )`/`[ ... ]`/`< ... >`/`{ ... }` as an atom
//     everywhere, so it accepts both meanings;
//   - keeps the operator precedence ladder but is happy to accept any
//     atom-juxtaposition as "application".
//
// The preprocessor block `@{ name = "value" | name = import "path" ... @}`
// is the only "comment-like" construct; it is parsed as its own node so doc
// strings can be highlighted.
//
// Run `tree-sitter generate` in this directory after editing.

const PREC = {
  lambda: 1,
  annotation: 2,
  arrow: 3,
  comparison: 4,
  addition: 5,
  assertion: 6,
  application: 7,
};

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

    // -- the @{ ... @} preprocessor block ----------------------------------
    preprocess_block: $ => seq(
      '@{',
      optional($.separator),
      repeat(seq($.pp_entry, optional($.separator))),
      '@}',
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
      optional('let'),
      field('name', $.identifier),
      '=',
      field('value', $.expression),
    ),

    separator: $ => choice(',', ';', '\n'),

    // -- expressions -------------------------------------------------------
    expression: $ => choice(
      $.lambda,
      $.annotation,
      $.arrow,
      $.binary_comparison,
      $.binary_addition,
      $.assert_expression,
      $.application,
    ),

    // `param => body` (right-assoc).  The parameter is a full expression, so
    // a typed param `x : T => e` reads the `: T` annotation as the parameter.
    lambda: $ => prec.right(PREC.lambda, seq(
      field('parameter', $.expression),
      '=>',
      field('body', $.expression),
    )),

    annotation: $ => prec.right(PREC.annotation, seq(
      field('value', $.expression),
      field('operator', choice(':', '#')),
      field('annotation', $.expression),
    )),

    arrow: $ => prec.right(PREC.arrow, seq(
      field('parameter', $.expression),
      '->',
      field('return_type', $.expression),
    )),

    binary_comparison: $ => prec.left(PREC.comparison, seq(
      field('left', $.expression),
      field('operator', choice('==', '<=')),
      field('right', $.expression),
    )),

    binary_addition: $ => prec.left(PREC.addition, seq(
      field('left', $.expression),
      field('operator', choice('+', '-')),
      field('right', $.expression),
    )),

    // `!` is a prefix assert over an application (juxtaposed atoms).  Keeping
    // its operand at the application level (rather than a full expression)
    // avoids the `! expr =>` ambiguity with lambdas; `!(x => e)` still works
    // because the lambda sits inside a parenthesized atom.
    assert_expression: $ => prec(PREC.assertion, seq('!', field('value', $.application))),

    // Juxtaposition: one or more atoms, left-associative.
    application: $ => prec.left(PREC.application, repeat1($._atom)),

    // An atom is a base form followed by postfix forms.  For simplicity and
    // permissiveness we keep only the unambiguous `.name` field read as a
    // postfix; every `[`/`<`/`{`/`(` is read as a *fresh* atom (and handled
    // by application juxtaposition), so the whitespace-sensitive "Glue"
    // distinction of the real parser is deliberately glossed over.
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

    _postfix: $ => field('field', $.field_read),

    field_read: $ => seq('.', field('name', $.identifier)),

    // -- atoms & literals --------------------------------------------------
    identifier: $ => /[A-Za-z_][A-Za-z0-9_]*/,
    integer: $ => /[0-9]+/,
    string_literal: $ => /"[^"]*"?/,
    placeholder: $ => '_',

    type_constant: $ => choice('Int', 'string', 'Type'),

    // `( e )` grouping / `(e1, e2, ...)` tuple.
    parenthesized: $ => seq(
      '(',
      optional(seq($.expression, repeat(seq($.separator, $.expression)), optional($.separator))),
      ')'
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

    // `<e1, e2, ...>` type tuple (the grammar is lenient: one-or-more).
    angle_tuple: $ => seq('<', $.expression, repeat(seq($.separator, $.expression)), optional($.separator), '>'),

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
      field('type', $.expression),
    ),

    // `table { k1 :: v1, ... }`.
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
      '::',
      field('value', $.expression),
    ),

    // `{ stmt; ...; expr }` block.
    block: $ => seq(
      '{',
      optional($.separator),
      optional($.statements),
      '}',
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
