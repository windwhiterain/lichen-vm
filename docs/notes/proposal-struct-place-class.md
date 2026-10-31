# Proposal (v2): a unify with a declaration checks, it does not carry a value

> Status: **proposal — for review, not applied.** Supersedes v1 (`5856f190`) after the
> review's four points. Written in the spec's own form (`:=`, `@raw`, `sytax`,
> `a = b` => `a <: b`). The spec file itself is untouched.

## The rule: text proposed for §Subtype

After the existing line `a = b` => `a <: b`, one paragraph:

```sytax
`e : Type` => `e` is a declaration
`a = b` where `b` is a declaration => `{a <: b; a}`
```

> A unify with a declaration **checks** subtype and carries **no value**. A declaration
> states a type; a type is not an answer. When neither side is a declaration, the unify
> carries the class's value, as it does today.

An expression whose type is `Type` is already defined (§Type: "expression with unit
variant lowlevel value has type `Type`"), so "declaration" is not a new category of
value — it is the one the language already names.

## §Struct: the reduction stays as today

Today's text is already a unify in the right direction (`a = b` => `a <: b`; the place
is the left, the declaration the right), so the reduction is kept. What changes is one
sentence after it:

> `raw v[...] = d` is a **requirement**: it checks the supplied value against `d` and
> carries no value. The field's only value is `e`'s.

```sytax
`s{.n2 e3, .n1 e4}` 
:= 
`{
  let {.n1 d1, .n2 d2} = s;
  v := raw[name_table, nominal_id, @raw None, @raw None];
  raw v[name_table["n1"]+2] = d1;
  raw v[name_table["n2"]+2] = d2;
  e4 = raw v[name_table["n1"]+2];
  e3 = raw v[name_table["n2"]+2];
  raw[raw v, Struct, ...]
}`

`s.<name>` := `{type_of s = Struct; @raw s.value[s.value.name_table[<name>]+2]}`
```

The two lines are ordered requirement-first only for reading; the value relations are
`e4 = place` and `e3 = place` (`e <: place`): the supplied expression is the field's
value.

## How the four review points are answered

1. **Direction and "write → unify".** Correct: the grammar has only unify
   (`unify := expr = expr`), and `a = b` is directed `a <: b`. This proposal claims no
   write→unify change and reverses nothing: the declaration is the right side (the
   requirement), the supplied value the left.
2. **A decided declaration would still be the class value.** It cannot be, under the
   rule: the declaration side is a check, so its value never joins the place's class.
   `fill_class_holes` fills a place only from the actual's value; the declaration is not
   in that class to be filled. This is the separation the notes require — the
   requirement is *checked*, not *stored*.
3. **This is not the measured class-merge design.** The class merge is exactly what the
   rule disallows on a declaration side; the place's class carries only the actual. No
   per-call freshness problem follows: the declaration participates in no instance's
   class, so two calls cannot share cells through it (the cross-call `GraphValue`
   collisions measured before came from the declaration value entering the class).
4. **The implementation mapping, verified against the code.** Four sites, named by the
   functions they are:
   - `check_instantiate` (`checker/structs.rs`): the instance's fields are the argument
     terms (`value_items.extend(ordered.iter().map(|&argument| vals[argument]))`); the
     **decided** branch's `check_unify(vals[*argument], field)` is the declaration
     unify that must become the check-only form (a subtype check with the existing error
     suppression, no class merge). The undecided branch already checks type and
     attributes only.
   - The lowlevel unify (`equality.rs`'s merge and `fill_class_holes`): a declaration
     side must not carry its value; the checker routes those unifies through the
     check-only form, so no lowlevel rule changes for other values.
   - The reads are **unchanged**: `value_slot_read` (`checker/structs.rs`), the compute
     module's free function `slot_value_of` (`compute.rs`), and the lowlevel `Index`
     read. Tuple and array reads are unaffected — their positions are values, not
     declarations; only a struct field whose declaration is a type expression takes the
     requirement path.
   - `record_template_field_sources` keeps its role for provenance (the decode's
     `field_source_of`); it no longer stands in for a value.

## The 22 affected tests and the path each exercises

All read a field whose declaration is a type expression before the supplied value is
decided; the expected path after the rule is: the declared requirement is checked, the
place holds the supplied value (or stays undecided until it runs), and the read answers
that value.

| test | the read it exercises |
|---|---|
| `chain_merge::a_chain_of_three_merges_into_one_node_under_the_floor` | `first.r` feeding the next dispatch's `In2.p` |
| `chain_merge::a_consumer_that_reads_two_producers_is_a_fan_in_and_merges` | two producers' fields into one consumer |
| `chain_merge::a_returned_middle_output_is_refused_by_name_and_nothing_merges` | a middle output declared by the codomain |
| `chain_merge::the_merged_chain_answers_the_un_passed_program_and_a_moved_bias_moves_it` | the same chain's record |
| `runtime_scalar_leaf::a_normalise_reading_a_runtime_scalar_merges_into_one_node` | a scalar leaf declared by the config |
| `runtime_scalar_leaf::a_producer_declaring_a_runtime_scalar_is_refused_by_name` | the config's declared scalar |
| `runtime_scalar_leaf::the_merged_diamond_answers_the_body_and_a_moved_bias_moves_it` | the diamond's declared leaf |
| `nested_config::a_nested_config_records_each_leaf_for_what_it_is` | a nested group's declared leaves |
| `nested_config::a_nested_config_runs_on_the_device_through_the_graph` | the same group's record |
| `nested_config::a_two_scalar_config_carries_both_and_runs_on_the_device` | two declared scalars |
| `cooperative_product::the_cpu_product_matches_the_host` | `Mma`'s declared buffers |
| `cooperative_product::the_gpu_product_matches_the_host` | the same on the device |
| `cooperative_product::the_product_is_warp_uniform_not_wider` | `Mma`'s declared buffer attributes |
| `float_leaf::a_float_leaf_and_an_integer_value_are_refused_in_both_directions` | a float leaf declared by the config |
| `float_leaf::a_float_scale_beside_two_extents_records_and_answers_on_the_device` | the same leaf's record |
| `perspective_requirements::a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it` | `plrun`'s result field read (`b.b`) |
| `perspective_requirements::a_write_returns_the_input_buffer_perspective` | the write's declared result |
| `sibling_diamond::a_sibling_diamond_reading_the_launch_extent_is_one_region_and_merges` | a sibling's declared output |
| `fan_out_merge::under_a_pack_the_device_answers_the_program_and_the_counterfactual_moves` | a fan-out's declared fields |
| `graph_argument::the_two_buffer_chain_runs_on_the_device_and_answers_over_both` | two recorded buffers' fields |
| `graph_runtime_scalar::what_makes_a_second_extent_admissible_does_not_depend_on_the_reads` | a declared extent |
| `graph_jit::what_a_recorded_body_may_not_reach_for_is_refused_by_name` | a recorded collect's declared buffer |

Excluded: the two documented parks
(`compute::a_float_domain_is_permitted_at_every_position_the_walk_reaches`,
`compute::jit_inline_nested_function`) and `fuse_licence::a_lane_dependent_value_read_at_a_computed_address_is_recomputed`,
which the notes record as a different defect.

## What does not change

- Surface syntax; annotations (§Annotation) keep their replacement rule.
- A decided instance: its field already holds the supplied value, so the read is
  unchanged.
- Clones and classes of ordinary values: the rule touches only the declaration side.
- Tuple/array reads: their positions are values, not declarations.
