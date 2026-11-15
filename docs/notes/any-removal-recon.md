# Any-removal migration — reconnaissance report

Session: recon worker, worktree `.worktrees/any-migration-recon`, branch `scratch/any-migration-recon` at dev `174379fb` (working tree clean). Remote `lichen-vm`, 2026-11-14. Read-only slice: this is the only file written; supporting captures live in `target/recon/` (untracked build directory).

## Build-environment facts (needed by every later slice)

- The runner's PATH has no MSVC `link.exe` and no coreutils. Two traps, both hit in this slice:
  1. Prepending Git's `usr/bin` to PATH makes rustc find **`C:\software\Git\Git\usr\bin\link.exe` (GNU coreutils link)** and fail every build script with `link: extra operand`. Never run cargo with that PATH.
  2. Without vcvars, rustc reports `error: linker `link.exe` not found` and `cargo check` exits 101 even though VS Community 2022 is installed (`C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\link.exe`).
- Working recipe used here: PowerShell, import `vcvars64.bat` environment (`cmd /c "\"...vcvars64.bat\" >NUL 2>&1 && set"` piped through `^([^=]+)=(.*)$` into `[Environment]::SetEnvironmentVariable(...,'Process')`), then `CARGO_TARGET_DIR=C:\resource\lichen-vm\.worktrees\any-migration-recon\target` (own target dir, AGENTS.md rule), cargo from `C:\Users\Admin\.cargo\bin`. Free disk was 107G (df on `C:`).
- Verification per AGENTS.md: every `Compiling`/`Checking` line in `target/recon/check.log` names `.worktrees\any-migration-recon`; a grep for the parent checkout path in the log returns nothing.

## 1. Baseline suite

Commands and their exit codes (recorded in `target/recon/*.exit`):

- `cargo check --workspace` → `CHECK_EXIT=0` (log `target/recon/check.log`, 597 lines).
- `cargo test --workspace --no-run` → `NOBUILD_EXIT=0` (log `target/recon/test_build.log`).
- `cargo test --workspace -- --test-threads=8` → `TEST_EXIT=101`. This aborted at the first failing binary (`tests\lichen_compute.rs`), so it reports five result lines only; it is kept because those five are verbatim.
- `cargo test --workspace --no-fail-fast -- --test-threads=8` → `TEST2_EXIT=101` (log `target/recon/test_run2.log`, 3768 lines). Every binary reported; this is the baseline table.

`cargo check` warnings, verbatim summary lines from `check.log`:

- `lichen-lowlevel` (lib) generated 1 warning — unused variable `x`
- `lichen-highlevel` (lib) generated 3 warnings — unused imports `ArrayItem`, `LowValue`; unused import `lichen_utils::extend::AsEnum` (x2)
- `lichen-graph-ir` (lib) generated 2 warnings — unreachable `else` clause; irrefutable `if let` pattern
- `lichen-language` (lib) generated 1 warning — unused import `ValueExt`
- `lichen-runtime` (lib) generated 3 warnings — unused imports `ExportKind`, `Export`, `FuncDecl`, `SignatureData`; unused variable `state`; function `countdown_body` is never used
- `lichen-compute` (lib) generated 15 warnings — unused doc comment; unused imports `SEQUENTIAL_PARALLEL_ELEMENTS`, `buffer_import_name`; unreachable statement; unreachable `else` clause (x2); dead code for `BUFFER_DIAGNOSTIC`, `BUF_ELEMENT_FIELD`, `is_loop_position`, `is_declared_position`, `buf_value`, `buffer_words`, `class_name`, `not_an_array`, field `entry`, method `current_loop`
- `lichen-compiler` (lib) generated 1 warning — variable does not need to be mutable
- `lichen-serve` (lib) generated 9 warnings — unused variables `index`, `session` (x3); unreachable pattern; never-read `rotation`, `capacity`; never-used `rotated`/`mixed`, `filled`

Test-binary build adds (`test_build.log`): `lichen-compute` (test "lichen_compute") generated 78 warnings; `lichen-compiler` (test "pack_install") generated 17 warnings; plus duplicate lib warnings.

### Per-binary results, verbatim (cargo test --workspace --no-fail-fast -- --test-threads=8)

\`\`\`
| ﻿Running tests\analyze.rs (target\debug\deps\analyze-8de466cae66cf7db.exe)  ||  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\main.rs (target\debug\deps\lichen_compiler-0dffc077dd5019aa.exe)  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\pack_install.rs (target\debug\deps\pack_install-eb3677834bfc10e4.exe)  ||  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_compute-6d02546d99126717.exe)  ||  test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\lichen_compute.rs (target\debug\deps\lichen_compute-db58823b985ce5ea.exe)  ||  test result: FAILED. 135 passed; 148 failed; 1 ignored; 0 measured; 0 filtered out |
| Running tests\compute_gpu.rs (target\debug\deps\compute_gpu-74c2b0ce503a674a.exe)  ||  test result: FAILED. 90 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\normalize_infinity.rs (target\debug\deps\normalize_infinity-94ad5be5dfad1d8a.ex  ||  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\fusion_bridge.rs (target\debug\deps\fusion_bridge-80a3d3ea476171cd.exe)  ||  test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\graph_runs.rs (target\debug\deps\graph_runs-200df2c2cd883d08.exe)  ||  test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\highlevel.rs (target\debug\deps\highlevel-c5b75b23fab96c9e.exe)  ||  test result: ok. 136 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\kernel_ir.rs (target\debug\deps\kernel_ir-1a5eca1bd8b32993.exe)  ||  test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_language-5df70193df2870dc.exe)  ||  test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\language.rs (target\debug\deps\language-a315862600147ce0.exe)  ||  test result: FAILED. 278 passed; 11 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_language_lex-a389b0f9922fcb7f.exe)  ||  test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_language_parser-bfa518dc947a3a12.exe)  ||  test result: ok. 62 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_language_server-9532c631a6dee65b.exe)  ||  test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\bin\lichen-language-server.rs (target\debug\deps\lichen_language_server  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\language_server.rs (target\debug\deps\language_server-9eb03ca45bc82680.exe)  ||  test result: FAILED. 21 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_language_zed-deddb75e6d2ae99d.exe)  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\grammar_consistency.rs (target\debug\deps\grammar_consistency-9bc6738c873eb4bb.  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\basic\main.rs (target\debug\deps\basic-ce9223a2303c73dc.exe)  ||  test result: ok. 167 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_package-15d6d62e2ccff403.exe)  ||  test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\main.rs (target\debug\deps\lichen-bc1b5076ccd37daf.exe)  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_preprocess-e083ac01ba88083e.exe)  ||  test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\directive_span.rs (target\debug\deps\directive_span-743d318e62278f70.exe)  ||  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\registry.rs (target\debug\deps\registry-62c70c02e700a02f.exe)  ||  test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_runtime-763bf4af3c8c7561.exe)  ||  test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\serve.rs (target\debug\deps\serve-c50ce94ae6f53669.exe)  ||  test result: FAILED. 10 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\line_model.rs (target\debug\deps\line_model-b920a2be7c8ac139.exe)  ||  test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_std_native-83f901d9ac0be0ee.exe)  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\lib.rs (target\debug\deps\lichen_tools-44bfa62bdd2dd860.exe)  ||  test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running unittests src\bin\sync-readme.rs (target\debug\deps\sync_readme-ca01b41a245281fd.exe)  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\tools.rs (target\debug\deps\tools-a76a5dc5918cd08a.exe)  ||  test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out |
| Running tests\utils.rs (target\debug\deps\utils-689a193187e48860.exe)  ||  test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_analyze  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_compiler  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_compute  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_compute_gpu  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_doc  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_fusion  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_fusion_bridge  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_graph_ir  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_highlevel  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_kernel_ir  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_language  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_language_lex  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_language_parser  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_language_server  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_language_zed  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_lowlevel  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_package  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_perspective  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_preprocess  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_registry  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_render  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_runtime  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_serve  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_span  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_std_native  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_testkit  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_tools  ||  test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
| Doc-tests lichen_utils  ||  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out |
\`\`\`

Only six binaries are red at baseline: \`tests\lichen_compute.rs\` (135/148/1 ignored), \`tests\compute_gpu.rs\` (90/4), \`tests\language.rs\` (278/11), \`tests\language_server.rs\` (21/1), \`tests\serve.rs\` (10/1), \`tests\tools.rs\` (2/2). Everything else passes, including \`tests\highlevel.rs\` 136, \`tests\basic (lowlevel)\` 167 with 1 ignored, language lib 69, parser lib 62.

### Full failing test names per red binary

148 in tests/lichen_compute.rs, 4 in compute_gpu, 11 in language, 1 in language_server, 1 in serve, 2 in tools. Source: target/recon/test_run2.log.
﻿--- lichen_compute (tests/lichen_compute.rs) ---
lichen_compute::automatic_tiling::a_fold_whose_bound_is_a_literal_tiles_with_the_seed_its_marks_name
lichen_compute::automatic_tiling::the_floor_tiles_a_declared_reduction_the_merge_stage_declines
lichen_compute::automatic_tiling::the_pipeline_tiles_a_declared_reduction_and_the_tiled_body_answers
lichen_compute::buf_element_class_recorded::the_compiled_fragment_names_the_declared_element_class
lichen_compute::buf_element_class_recorded::the_module_holds_a_decided_float_element_type
lichen_compute::buf_element_class_recorded::the_recorded_node_carries_the_declared_element_class
lichen_compute::chain_merge::a_chain_of_three_merges_into_one_node_under_the_floor
lichen_compute::chain_merge::a_consumer_that_reads_two_producers_is_a_fan_in_and_merges
lichen_compute::chain_merge::a_returned_middle_output_is_refused_by_name_and_nothing_merges
lichen_compute::chain_merge::the_merged_chain_answers_the_un_passed_program_and_a_moved_bias_moves_it
lichen_compute::combine_fold::a_sum_of_squares_tiles_with_the_datum_its_claim_names_and_answers_the_squares
lichen_compute::compile_order_class::a_kernels_classes_do_not_depend_on_which_kernel_compiled_first
lichen_compute::compile_order_class::the_recorded_consumer_carries_the_programs_float_class
lichen_compute::composed_chain::the_composed_chain_records_the_merged_body
lichen_compute::compute::a_float_argument_to_an_int_parameter_is_refused_by_class
lichen_compute::compute::a_float_domain_is_permitted_at_every_position_the_walk_reaches
lichen_compute::compute::a_jit_kernel_crosses_the_two_classes_both_ways
lichen_compute::compute::a_kernel_reads_the_input_buffer_it_names
lichen_compute::compute::a_multi_output_parallel_run_is_identical_sequential_and_parallel
lichen_compute::compute::a_parallel_run_over_the_threshold_covers_every_index
lichen_compute::compute::a_refused_call_argument_element_says_why
lichen_compute::compute::a_refused_call_argument_shape_says_why
lichen_compute::compute::a_refused_call_run_says_why
lichen_compute::compute::a_refused_plrun_count_says_why
lichen_compute::compute::a_runtime_scalar_reaches_the_body_beside_the_extent
lichen_compute::compute::a_scalar_leaf_of_the_wrong_class_is_refused_by_name
lichen_compute::compute::a_statement_write_is_dropped_and_the_codowns_write_fills_the_output
lichen_compute::compute::a_struct_parameter_kernel_runs_through_the_signature_carrying_wrapper
lichen_compute::compute::a_write_to_another_output_than_its_position_exchanges_the_outputs
lichen_compute::compute::jit_an_operator_inside_a_cross_kernel_argument_is_emitted
lichen_compute::compute::jit_closes_over_constant
lichen_compute::compute::jit_conditional_else
lichen_compute::compute::jit_conditional_then
lichen_compute::compute::jit_cross_kernel_call
lichen_compute::compute::jit_cross_kernel_passes_a_sub_tuple_through
lichen_compute::compute::jit_cross_kernel_passes_the_parameter_through
lichen_compute::compute::jit_cross_kernel_subexpr
lichen_compute::compute::jit_cross_kernel_tuple_argument
lichen_compute::compute::jit_cross_kernel_tuple_argument_through_the_wrapper
lichen_compute::compute::jit_cross_kernel_wrapper
lichen_compute::compute::jit_inline_lichen_function
lichen_compute::compute::jit_inline_nested_function
lichen_compute::compute::jit_lowers_the_arithmetic_comparison_and_bitwise_operators
lichen_compute::compute::jit_multi_arg_all_ops
lichen_compute::compute::jit_multi_arg_sub
lichen_compute::compute::jit_multi_arg_ternary
lichen_compute::compute::jit_multi_arg_tuple
lichen_compute::compute::jit_multi_op_signature
lichen_compute::compute::jit_nested_tuple_domain
lichen_compute::compute::jit_refuses_a_cross_kernel_call_to_a_multi_value_kernel
lichen_compute::compute::jit_then_launch_scalar
lichen_compute::compute::jit_three_value_codomain_returns_three_values
lichen_compute::compute::jit_tuple_codomain_computes_each_leaf
lichen_compute::compute::jit_tuple_codomain_elements_are_indexable
lichen_compute::compute::jit_tuple_codomain_launches_through_the_cross_kernel_wrapper
lichen_compute::compute::jit_tuple_codomain_returns_several_values
lichen_compute::compute::launch_rejects_wrong_arity
lichen_compute::compute::parallel_multi_output_collects_each_output
lichen_compute::compute::parallel_multi_output_writes_every_output_in_one_pass
lichen_compute::compute::parallel_range_write_is_map
lichen_compute::compute::parallel_read_input_buffer
lichen_compute::compute::parallel_write_only_collects_whole_buffer
lichen_compute::consumer_guard::a_combine_only_consumer_is_refused_by_the_edge_and_the_differential_holds
lichen_compute::consumer_guard::a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched
lichen_compute::consumer_guard::the_program_answers_the_same_with_the_pass_and_without_it
lichen_compute::cooperative_product::the_cpu_product_matches_the_host
lichen_compute::cooperative_product::the_gpu_product_matches_the_host
lichen_compute::elementwise_chain::an_elementwise_chain_derives_a_schema_the_floor_proves
lichen_compute::elementwise_chain::two_elementwise_kernels_derive_a_region_the_floor_proves
lichen_compute::fan_in_merge::a_consumer_of_two_producers_merges_into_one_node_under_the_floor
lichen_compute::fan_in_merge::a_fan_in_whose_element_does_not_name_its_second_producer_is_refused_by_name
lichen_compute::fan_out_merge::a_fan_out_derives_a_region_a_pack_can_name
lichen_compute::fan_out_merge::a_pack_naming_the_fan_out_merges_all_three_nodes_into_one
lichen_compute::fan_out_merge::under_a_pack_the_device_answers_the_program_and_the_counterfactual_moves
lichen_compute::fan_out_merge::without_a_pack_the_floor_proves_the_fan_out
lichen_compute::float_leaf::a_float_leaf_and_an_integer_value_are_refused_in_both_directions
lichen_compute::float_leaf::a_float_scale_beside_two_extents_records_and_answers_on_the_device
lichen_compute::fold_datum::a_datum_no_body_computes_is_refused_by_name_and_the_body_answers
lichen_compute::fold_datum::a_maximum_tiles_under_the_same_proof_and_answers_the_untiled_body
lichen_compute::frozen_buffer_reuse::a_frozen_buffer_cell_still_feeds_the_next_launch
lichen_compute::fuse_licence::a_lane_dependent_value_read_at_a_computed_address_is_recomputed
lichen_compute::fuse_licence::a_pack_naming_another_region_is_inert
lichen_compute::fuse_licence::a_pack_naming_the_region_merges_it_and_the_report_names_the_pack
lichen_compute::fuse_licence::a_read_at_a_computed_address_runs_on_the_device_and_the_counterfactual_moves
lichen_compute::fuse_licence::a_read_at_a_conditional_address_runs_on_the_device_and_the_counterfactual_moves
lichen_compute::fuse_licence::an_elementwise_producer_feeding_a_reduction_consumer_merges_and_the_device_agrees
lichen_compute::fuse_licence::the_floor_still_merges_a_reduction_chain_with_no_pack_installed
lichen_compute::fuse_licence::the_fuse_directive_is_read_as_the_format_spells_it
lichen_compute::fuse_licence::with_the_pack_the_device_answers_the_program_and_the_counterfactual_moves
lichen_compute::fuse_licence::without_a_pack_the_floor_proves_the_elementwise_region
lichen_compute::fused_region::a_composed_node_is_named_by_a_second_merge_pass_and_left_alone
lichen_compute::fused_region::a_fan_out_region_with_no_pack_is_refused_by_name
lichen_compute::fused_region::a_returned_producer_output_is_refused_by_name_and_nothing_merges
lichen_compute::fused_region::an_address_the_consumer_computes_is_answered_by_the_producer_s_own_cone
lichen_compute::fused_region::extents_that_do_not_correspond_are_refused_by_name_and_nothing_merges
lichen_compute::fused_region::the_floor_reports_the_merge_and_then_the_tiling_pass_on_the_merged_node
lichen_compute::fused_region::the_floor_run_reports_both_stages_and_answers_the_un_passed_program
lichen_compute::fused_region::the_region_merges_into_one_node_and_the_surviving_node_holds_the_composed_fragment
lichen_compute::graph_argument::a_declared_buffer_beside_the_in_group_fails_the_build_and_never_runs
lichen_compute::graph_argument::a_second_buffer_records_under_the_in_group_and_does_not_beside_it
lichen_compute::graph_argument::a_step_whose_argument_names_two_buffers_records_three_inputs
lichen_compute::graph_argument::the_two_buffer_chain_runs_on_the_device_and_answers_over_both
lichen_compute::graph_jit::a_buffer_the_body_closed_over_is_refused_by_the_capture
lichen_compute::graph_jit::a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one
lichen_compute::graph_jit::a_dispatch_the_body_closed_over_is_refused_rather_than_recorded
lichen_compute::graph_jit::a_graph_dispatches_what_the_program_dispatches_with_repeats_collapsed
lichen_compute::graph_jit::a_graph_recorded_for_one_backend_runs_on_another
lichen_compute::graph_jit::one_backend_for_the_whole_graph_is_checked_while_it_is_built
lichen_compute::graph_jit::what_a_recorded_body_may_not_reach_for_is_refused_by_name
lichen_compute::graph_runtime_scalar::a_second_runtime_scalar_is_carried_beside_the_extent
lichen_compute::graph_runtime_scalar::the_second_extent_is_recorded_as_the_scalars_own_edge
lichen_compute::graph_runtime_scalar::the_two_extent_launch_runs_on_the_device_through_the_graph
lichen_compute::graph_runtime_scalar::what_makes_a_second_extent_admissible_does_not_depend_on_the_reads
lichen_compute::graph_structure::the_kernel_slot_is_the_captured_artifact_and_the_cfg_an_index_chain
lichen_compute::identity_from_body::a_maximum_over_data_below_the_old_seed_answers_what_the_untiled_body_answers
lichen_compute::identity_from_body::a_minimum_over_a_row_of_positives_answers_what_the_untiled_body_answers
lichen_compute::identity_from_body::a_seed_that_is_no_literal_and_a_missing_role_are_refused_by_name
lichen_compute::loop_write::a_body_that_is_a_fold_writes_inside_the_folds_step
lichen_compute::loop_write::one_plain_write_fills_the_output
lichen_compute::mark_anchor::a_mark_inside_a_loop_still_names_its_whole_expression
lichen_compute::mark_anchor::a_mark_on_a_top_level_operator_names_its_whole_expression
lichen_compute::nested_config::a_nested_config_records_each_leaf_for_what_it_is
lichen_compute::nested_config::a_nested_config_runs_on_the_device_through_the_graph
lichen_compute::nested_config::a_two_scalar_config_carries_both_and_runs_on_the_device
lichen_compute::pack_placement::a_matching_pack_fires_and_the_plan_it_states_is_what_lands
lichen_compute::pack_placement::a_pack_asking_for_a_width_the_target_lacks_is_refused
lichen_compute::pack_placement::a_pack_whose_constraints_do_not_hold_is_inert
lichen_compute::pack_placement::an_unmatched_pack_is_inert
lichen_compute::pack_placement::the_landed_fragment_reads_the_class_the_pack_states
lichen_compute::recompute_reads::the_merge_copies_the_producer_cone_once_per_read_at_that_read_s_address
lichen_compute::recompute_reads::the_recording_reads_one_buffer_at_two_addresses
lichen_compute::recompute_reads::two_reads_merge_and_the_device_answers_the_program
lichen_compute::reduction_chain::the_declared_algebra_licenses_tiling_and_the_artifact_lands_in_a_graph
lichen_compute::rmsnorm_denominator::a_fixed_size_and_a_runtime_extent_both_fuse
lichen_compute::runtime_only_package::an_imported_package_that_jits_at_its_top_level_still_runs
lichen_compute::runtime_scalar_leaf::a_normalise_reading_a_runtime_scalar_merges_into_one_node
lichen_compute::runtime_scalar_leaf::a_producer_declaring_a_runtime_scalar_is_refused_by_name
lichen_compute::runtime_scalar_leaf::the_merged_diamond_answers_the_body_and_a_moved_bias_moves_it
lichen_compute::sibling_diamond::a_sibling_diamond_merges_into_one_node_under_the_floor
lichen_compute::sibling_diamond::a_sibling_diamond_reading_the_launch_extent_is_one_region_and_merges
lichen_compute::sibling_diamond::a_sibling_diamond_whose_element_names_one_producer_is_refused_by_name
lichen_compute::sibling_diamond::an_element_reading_the_invocation_index_is_one_region_and_merges
lichen_compute::sparse_input_classes::the_consumer_declares_a_float_for_each_of_its_two_inputs
lichen_compute::tiling_visibility::a_composed_node_is_reported_by_name_and_nothing_lands_in_it
lichen_compute::tiling_visibility::an_ordinary_graph_reports_one_line_per_node
lichen_compute::union_ports::a_consumer_naming_only_the_absorbed_buffer_merges
lichen_compute::union_ports::a_node_whose_ordinals_disagree_with_its_body_is_refused_by_name
lichen_compute::zz_store_loop_probe::a_store_registration_and_a_module_drop_repeat_where_the_peel_breaks_them
--- compute_gpu (tests/compute_gpu.rs) ---
compute_gpu::materialised_integer::a_float_loop_materialising_a_boolean_into_the_integer_class_validates
compute_gpu::materialised_integer::a_float_module_naming_the_integer_zero_as_a_condition_validates
compute_gpu::mma_shader::the_cooperative_matrix_module_validates
compute_gpu::spirv_validation::a_folded_reduction_validates
--- language (tests/language.rs) ---
language::callee_inference::a_named_type_function_parameter_and_its_field_reads
language::callee_inference::an_instantiation_whose_callee_is_inferred
language::field_read_kinds::a_formal_field_read_is_checked_against_the_argument_at_the_call
language::field_read_kinds::a_named_read_of_a_deferred_array_is_refused_at_the_read
language::field_read_kinds::a_named_read_of_a_deferred_non_struct_is_refused
language::field_read_kinds::a_named_read_of_a_deferred_struct_is_accepted
language::merge_carry::a_merge_carries_the_decided_type_to_a_member_added_after_the_commit
language::pipeline::a_field_reads_class_is_decided_where_the_container_type_is
language::pipeline::a_struct_instance_reads_its_fields_by_name
language::template_instances::a_struct_types_wildcard_fields_stay_open_across_instances
language::template_instances::an_inline_literal_domain_is_fresh_per_application
--- language_server (tests/language_server.rs) ---
language_server::statement_values::compute_kernel_bindings_render_by_name_not_raw_layout
--- serve (tests/serve.rs) ---
serve::schema_probe::a_recorded_graph_reports_the_schema_a_pack_must_target
--- tools (tests/tools.rs) ---
tools::examples::every_example_runs_and_prints_what_it_declares
tools::zz_kernel_body_probe::matmul_kernel_body

## 2. Any census

Scope: every file under `crates/`, `examples/`, `lichen-std/`, `lichen-std-native/`, `tree-sitter-lichen/`, `lichen-language-zed/`, `helix/` mentioning the Rust `LowValue::Any` / `TokenKind::Any` / `Expr::Any` / `AnyLit` variants or the source spelling `@any`. Raw lines are embedded verbatim below; the role key follows the brief: **unify rule / clone rule / evaluation / codec / internal mechanism / surface lowering (AnyLit) / printer / test / example / grammar / editor**.

### Rust `Any` tokens — role per site (file:line; see the verbatim dump after this table)

- `crates/lichen-lowlevel/src/lib.rs:88-93` — **unify rule + internal mechanism**: the `LowValue::Any` variant itself, with the `# Invariant` (directional: left `Any` joins an undecided right class; readers treat it as undecided; only the concreteness verdict takes it as decided).
- `crates/lichen-lowlevel/src/lib.rs:113` — **internal mechanism**: `PartialEq` arm `(Any, Any) => true`.
- `crates/lichen-lowlevel/src/equality.rs:386-407` — **unify rule**: `clear_wildcard` (a lone `Any` slot may be emptied so a statement can give it a value); `:459-490` `value_holds_open_cell` (open = no value **or** `Any`); `:1122-1143` `function_has_wildcard` (signature carries `Any` in parameter value or return type); `:1164-1177` directional wildcard arm of `unify_inner`; `:1245-1251` `_ = @any` is `_` (right `Any` never decides); `:1382-1394` `alias_read` refuses to join a reader to a wildcard target; `:1448-1451` `is_wildcard`.
- `crates/lichen-lowlevel/src/evaluation.rs:171-190` — **evaluation**: `Any` is never a computed answer to hand on (twice); `:272-277` the wildcard answers no reader; `:884-889` the deep concreteness verdict treats `Any` as **decided content**; `:1013-1030` `evaluate_block` decides scope-release on "not `Any`".
- `crates/lichen-lowlevel/src/apply.rs:305` — **evaluation**: decided-projection scan skips `Any`; `:404-412` — **clone rule**: an apply whose result is `Any` is undecided; `:464-477` — **clone rule**: `answer_elements_are_undecided` counts `Any` open for the run policy.
- `crates/lichen-lowlevel/src/function.rs:322-331` — **clone rule**: a cloned `Any` codomain reads the template requirement; `:445-452` — **internal**: replay suppression requires open (none-or-`Any`) cells; `:605-612` — **clone rule**: `Any` is not an answer; `:866-872` — **clone rule**: SPEC §Clone Any→fresh empty node in `node_apply`.
- `crates/lichen-lowlevel/src/static_module/apply.rs:474-478, 565-570` — **clone rule**: the frozen walks' Any→hole arms; `:755-765` — **clone rule**: array items that are undecided or `Any` force the clone walk.
- `crates/lichen-lowlevel/src/static_module.rs:330-346` — **internal**: frozen operator search skips wildcard class members; `:495-508` — **internal**: frozen parameter/return openness test.
- `crates/lichen-lowlevel/src/template.rs:297, 402, 539, 746` — **clone rule** (template walk): openness/closure tests count `Any` as open; the wildcard becomes a fresh empty cell.
- `crates/lichen-lowlevel/src/table.rs:251-255` — **evaluation/codec**: key hashing — `Any` is `Undecided` in Key mode, `UNDECIDED_TOKEN` in Template mode (decision D5 territory).
- `crates/lichen-lowlevel/src/codec.rs:255, 301` — **codec**: `LowValue::Any` is artifact tag `9` (round-trips through the frozen artifact format).
- `crates/lichen-lowlevel/src/loop_run.rs:222, 256` — **internal mechanism**: the loop-run replay's `slot_expr`/`slot_array` mint `Any` cells for every attribute position of a `[value, type, wildcards…]` expr ("an attribute no one named"); line 128 is prose (see §3d).
- `crates/lichen-highlevel/src/program.rs:433-455` — **surface lowering (AnyLit)**: `AnyLit::build` = fresh value node `LowValue::Any` + fresh own type cell; `# Invariant` — fresh per occurrence; `:584, 604` — **surface lowering**: `HighProgramLiteral::Any` in the literal registry; `:1515` — **surface lowering**: `placeholder_expr` mints `Any` for a bare `_` callee's value slot.
- `crates/lichen-highlevel/src/checker.rs:1031` — **internal mechanism**: the one shared `any_value` ("Default positions share this wildcard; source literals and function sugar are fresh"); `:1136-1143` `any()`; `:1440-1451` — **evaluation/attribute**: a stated element attribute excludes `Any`; `:1965-1985` — **printer-adjacent**: an attribute slot holding `Any` states nothing.
- `crates/lichen-highlevel/src/checker/lambda.rs:126-142` — **unify rule (the old auto-`@any` remnant)**: an unasserted identity lambda's parameter class is written `LowValue::Any` at definition; see §3a.
- `crates/lichen-highlevel/src/checker/annotations.rs:240-263` — **unify rule**: `stated_slot` vs `carried_wildcard` split on `Any`; `:264-275` `adopt_stated_attributes` `# Invariant` ("`Any` unifies with everything and merges with nothing"); `:548`, `:613` `# Invariant` (element slots hold `@any`, so the lowlevel compare is vacuous and reconciliation happens here).
- `crates/lichen-highlevel/src/checker/indexing.rs:639-680` — **unify rule**: `wildcard_slot` (is `Any`, or empty whose origin reaches `Any` through ≤8 hops) and `wildcard_requirement` `# Invariant`.
- `crates/lichen-highlevel/src/checker/tuples.rs:94-97` — **internal**: `element_slot` `# Invariant` ("an unconstrained position holds `Any`, replaced by an open cell in each apply clone"); `:105, 111, 137` fill with `self.any()`.
- `crates/lichen-highlevel/src/shape.rs:688-696` — **internal**: a wildcard value position seeds no class low-shape.
- `crates/lichen-language/src/compile.rs:17, 495-496` — **surface lowering**: `Expr::Any` → `AnyLit` literal.
- `crates/lichen-language/src/resolve.rs:611`, `dirty.rs:299`, `spans.rs:29`, `session.rs:944`, `resolve/content_key.rs:226` — **internal mechanism**: `Expr::Any` pattern arms; content-key code `36` (incremental digest of the spelling).
- `crates/lichen-language/src/program.rs:706`, `testing.rs:180, 189` — **internal**: module-level `Any` cell for the default attribute array; template-slot placeholder value in the render test harness.
- `crates/lichen-language-lex/src/lib.rs:83, 239, 808` — **grammar/lexer**: `TokenKind::Any` (`@any`), its display name, and the `"any"` keyword map.
- `crates/lichen-language-parser/src/ast.rs:123, 411`, `parse.rs:801, 1095, 1194`, `path.rs:68, 97`, `parse/error_blocks.rs:26` — **grammar/parser**: `Expr::Any` atom; **`parse.rs:793-801`: a value-less annotation chain desugars with the `@any` head**; **`parse.rs:1174-1202`: the primed-field sugar `.'name …` replaces that `Expr::Any` head with the formal** (§3a).
- `crates/lichen-language-server/src/analysis.rs:1499, 1639, 1923, 2268` — **editor**: token-classification and AST-walk arms for `@any`.
- `crates/lichen-render/src/render/type_printer.rs:130, 164, 471, 510` + `# Invariant` at `:390, 451` — **printer**: renders `@any`; the slot printer suppresses both the shared wildcard and its per-call hole; `value_printer.rs:104, 274, 636` — **printer**: `@any` spellings and the `?` fallback.
- `crates/lichen-compute/src/compute.rs:1513, 4107, 4585, 4626, 6618, 6802-6841, 7496, 8144` — **evaluation/internal (compute)**: openness tests (`Any` counts open) at the graph-argument read, `decided_buffer_value`, element-slot minting, `recorded_leaf`/buffer-payload walks following `field_sources` while the cell is empty-or-`Any`, `GraphInput` placeholder attributes; `compute/graph.rs:490` — **printer**: refusal text "a value this run has not decided"; `compute/body.rs:1070` — comment citing SPEC §Any; `std` docs `compute.rs:1155`-area comments.
- `lichen-std-native/src/lib.rs:95, 134, 155` — **evaluation/internal**: `std.sort` treats an `Any` element as undecided; result slots mint `Any` attributes; doc "Evaluate even with open `@any` type cells".
- Tests (`crates/lichen-highlevel/tests/highlevel/{checker,lowering,table}.rs`), (`crates/lichen-compute/tests/lichen_compute/{zz_*probe,runtime_only_package}.rs`) — **test** role, embedded verbatim below.

### Verbatim dump — Rust `Any` token sites (file:line:content, 128 lines)

```text
crates/lichen-compute/src/compute/body.rs:1070:    /// the requirement the Any rule left open (`docs/language-spec.md` §Any).
crates/lichen-compute/src/compute/body.rs:1124:    /// `$write`/`$write_atomic` (`docs/notes/kernel-buffer-declarations.md`). Any other
crates/lichen-compute/src/compute/graph.rs:490:        LowValue::Any => "a value this run has not decided".to_string(),
crates/lichen-compute/src/compute.rs:1513:                        if value.is_none_or(|value| matches!(value, LowValue::Any))
crates/lichen-compute/src/compute.rs:4107:    (!matches!(value.as_enum(), Some(LowValue::Any))).then_some(value)
crates/lichen-compute/src/compute.rs:4585:            Some(<P::Value as From<LowValue>>::from(LowValue::Any)),
crates/lichen-compute/src/compute.rs:4626:            Some(<P::Value as From<LowValue>>::from(LowValue::Any)),
crates/lichen-compute/src/compute.rs:6618:            .is_none_or(|value| matches!(value.as_enum(), Some(LowValue::Any)));
crates/lichen-compute/src/compute.rs:6802:        .is_none_or(|value| matches!(value.as_enum(), Some(LowValue::Any)));
crates/lichen-compute/src/compute.rs:6807:                .is_none_or(|value| matches!(value.as_enum(), Some(LowValue::Any)))
crates/lichen-compute/src/compute.rs:6839:            .is_none_or(|value| matches!(value.as_enum(), Some(LowValue::Any)));
crates/lichen-compute/src/compute.rs:7496:            let wildcard = <P::Value as From<LowValue>>::from(LowValue::Any);
crates/lichen-compute/src/compute.rs:8144:        let width = <P::Value as From<LowValue>>::from(LowValue::Any);
crates/lichen-compute/tests/lichen_compute/runtime_only_package.rs:59:    // Any cached artifact means the refusal went unexercised: `kernels.lichen` is
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1691:/// The Any-typed field's own reading, beside the decided field's refusal.
crates/lichen-compute/tests/lichen_compute/zz_gc_panic_probe.rs:12:fn message(payload: &(dyn std::any::Any + Send)) -> String {
crates/lichen-compute/tests/lichen_compute/zz_gc_panic_probe.rs:22:fn report(label: &str, run: Result<String, Box<dyn std::any::Any + Send>>) {
crates/lichen-highlevel/src/checker/annotations.rs:246:        (!matches!(value, Some(lichen_lowlevel::LowValue::Any))).then_some(slot)
crates/lichen-highlevel/src/checker/annotations.rs:258:        matches!(value, Some(lichen_lowlevel::LowValue::Any)).then_some(slot)
crates/lichen-highlevel/src/checker/annotations.rs:266:    /// its own. `Any` unifies with everything and merges with nothing, so a
crates/lichen-highlevel/src/checker/indexing.rs:639:    /// Whether a slot is `Any` or an empty clone whose origin reaches `Any`.
crates/lichen-highlevel/src/checker/indexing.rs:645:            Some(LowValue::Any)
crates/lichen-highlevel/src/checker/indexing.rs:663:                Some(LowValue::Any)
crates/lichen-highlevel/src/checker/indexing.rs:675:    /// Empty alone is not wildcard: the node's origin must reach `Any`, so an ordinary
crates/lichen-highlevel/src/checker/lambda.rs:140:                    .write_node_value(cell, Some(P::Value::from(LowValue::Any)));
crates/lichen-highlevel/src/checker/tuples.rs:97:    /// unconstrained position holds `Any`, replaced by an open cell in each apply clone.
crates/lichen-highlevel/src/checker.rs:1031:        self.any_value = self.alloc_node(root, None, Some(P::Value::from(LowValue::Any)));
crates/lichen-highlevel/src/checker.rs:1141:            Some(P::Value::from(LowValue::Any)),
crates/lichen-highlevel/src/checker.rs:1447:                .is_none_or(|value| value != LowValue::Any);
crates/lichen-highlevel/src/checker.rs:1977:                if matches!(wildcard, Some(LowValue::Any)) {
crates/lichen-highlevel/src/program.rs:433:pub struct AnyLit;
crates/lichen-highlevel/src/program.rs:435:impl<P> LiteralExt<P> for AnyLit
crates/lichen-highlevel/src/program.rs:440:        let value = ctx.value_node(P::Value::from(LowValue::Any));
crates/lichen-highlevel/src/program.rs:584:    + AnyLit as Any;
crates/lichen-highlevel/src/program.rs:604:            HighProgramLiteral::Any(lit) => lit.build(ctx),
crates/lichen-highlevel/src/program.rs:1515:    let any = module.add_node(block, None, Some(P::Value::from(LowValue::Any)));
crates/lichen-highlevel/src/shape.rs:692:            Some(LowValue::Any)
crates/lichen-highlevel/tests/highlevel/checker.rs:9:    AnyLit, HighProgramLiteral, HighProgramValue, ProgramImpl, StructTemplateLit, TypeValue,
crates/lichen-highlevel/tests/highlevel/checker.rs:31:    w.fill(literal, ExprKind::Literal(HighProgramLiteral::from(AnyLit)));
crates/lichen-highlevel/tests/highlevel/lowering.rs:5:use lichen_highlevel::program::{AnyLit, HighProgramLiteral, ProgramImpl};
crates/lichen-highlevel/tests/highlevel/lowering.rs:134:    w.fill(literal, ExprKind::Literal(HighProgramLiteral::from(AnyLit)));
crates/lichen-highlevel/tests/highlevel/table.rs:6:use lichen_highlevel::program::{AnyLit, HighProgramLiteral, ProgramImpl};
crates/lichen-highlevel/tests/highlevel/table.rs:40:    w.fill(literal, ExprKind::Literal(HighProgramLiteral::from(AnyLit)));
crates/lichen-language/src/compile.rs:17:    AnyLit, ArrayTemplateLit, FloatLit, FloatTypeLit, HighProgramLiteral, IntLit, IntTypeLit,
crates/lichen-language/src/compile.rs:495:            Expr::Any(span) => {
crates/lichen-language/src/compile.rs:496:                self.alloc(ExprKind::Literal(HighProgramLiteral::from(AnyLit)), span)
crates/lichen-language/src/dirty.rs:299:        | Expr::Any(..)
crates/lichen-language/src/program.rs:706:        let any = module.add_node(block, None, Some(LangValue::from(LowValue::Any)));
crates/lichen-language/src/resolve/content_key.rs:226:            Expr::Any(_) => self.u(36),
crates/lichen-language/src/resolve.rs:611:            | Expr::Any(..)
crates/lichen-language/src/resolve.rs:699:/// innermost body. Any other prime is refused rather than bound to an ordinary
crates/lichen-language/src/session.rs:944:/// The result is exactly what a whole-buffer parse of `new_tokens` produces. Any
crates/lichen-language/src/spans.rs:29:        | Expr::Any(span) => shift(span, old_starts, new_starts, delta),
crates/lichen-language/src/testing.rs:180:                                    P::Value::from(LowValue::Any)
crates/lichen-language/src/testing.rs:189:                                    P::Value::from(LowValue::Any)
crates/lichen-language-lex/src/lib.rs:83:    Any,
crates/lichen-language-lex/src/lib.rs:239:            TokenKind::Any => "'@any'",
crates/lichen-language-lex/src/lib.rs:808:            "any" => Some(TokenKind::Any),
crates/lichen-language-parser/src/ast.rs:123:    Any(Span),
crates/lichen-language-parser/src/ast.rs:411:            Expr::Any(s) => *s,
crates/lichen-language-parser/src/parse/error_blocks.rs:26:            | Expr::Any(..) => {}
crates/lichen-language-parser/src/parse.rs:801:                    fold_annotations(Expr::Any(span_at(tokens, me.span().start)), rest)
crates/lichen-language-parser/src/parse.rs:1095:            token(TokenKind::Any).map(|t| Expr::Any(t.span)),
crates/lichen-language-parser/src/parse.rs:1194:        }) if matches!(&*value, Expr::Any(_)) => Some(Expr::Annotation {
crates/lichen-language-parser/src/path.rs:68:    /// Any other expression position.
crates/lichen-language-parser/src/path.rs:97:        | Expr::Any(..)
crates/lichen-language-server/src/analysis.rs:1499:        | TokenKind::Any
crates/lichen-language-server/src/analysis.rs:1639:            | Expr::Any(..)
crates/lichen-language-server/src/analysis.rs:1923:            | Expr::Any(..)
crates/lichen-language-server/src/analysis.rs:2268:            | Expr::Any(..)
crates/lichen-lowlevel/src/apply.rs:305:            if matches!(value, LowValue::Any) {
crates/lichen-lowlevel/src/apply.rs:408:            Some(LowValue::Any)
crates/lichen-lowlevel/src/apply.rs:464:/// a clone may answer with, however open its interior is. `Any` is undecided for this
crates/lichen-lowlevel/src/apply.rs:475:        slot_value(item.node).is_none_or(|value| matches!(value.as_enum(), Some(LowValue::Any)))
crates/lichen-lowlevel/src/codec.rs:255:            LowValue::Any => w.u8(9),
crates/lichen-lowlevel/src/codec.rs:301:            9 => LowValue::Any,
crates/lichen-lowlevel/src/equality.rs:387:    /// Only a lone wildcard (`Any`) is cleared: it says the position never decides,
crates/lichen-lowlevel/src/equality.rs:399:            Some(LowValue::Any)
crates/lichen-lowlevel/src/equality.rs:460:    /// An open cell is a node with no value or with `Any` — the wildcard a template's
crates/lichen-lowlevel/src/equality.rs:483:                    Some(LowValue::Any) => return true,
crates/lichen-lowlevel/src/equality.rs:1122:    /// Whether a function's signature carries `Any` in its parameter value or return type.
crates/lichen-lowlevel/src/equality.rs:1125:    /// `Any` recurses only into arrays: signature comparison stops before parameter/return
crates/lichen-lowlevel/src/equality.rs:1139:                    Some(LowValue::Any)
crates/lichen-lowlevel/src/equality.rs:1164:        // Directional wildcard: only a left `Any` may join an undecided right class.
crates/lichen-lowlevel/src/equality.rs:1245:            // An `Any` on the right never decides an undecided left: `_ = @any` is `_`.
crates/lichen-lowlevel/src/equality.rs:1247:                if !matches!(AsEnum::<LowValue>::as_enum(&y), Some(LowValue::Any)) {
crates/lichen-lowlevel/src/equality.rs:1394:            Some(LowValue::Any)
crates/lichen-lowlevel/src/equality.rs:1448:/// Whether a side's class holds the wildcard `Any`.
crates/lichen-lowlevel/src/equality.rs:1450:    matches!(value.and_then(|value| value.as_enum()), Some(LowValue::Any))
crates/lichen-lowlevel/src/evaluation.rs:171:            if matches!(value.as_enum(), Some(LowValue::Any)) {
crates/lichen-lowlevel/src/evaluation.rs:186:                    Some(LowValue::Any) => None,
crates/lichen-lowlevel/src/evaluation.rs:272:                                                        // is undecided, never the value `Any`.
crates/lichen-lowlevel/src/evaluation.rs:277:                                                            Some(LowValue::Any) => None,
crates/lichen-lowlevel/src/evaluation.rs:884:            Some(value) if matches!(value.as_enum(), Some(LowValue::Any)) => {
crates/lichen-lowlevel/src/evaluation.rs:1026:            .is_some_and(|value| !matches!(value.as_enum(), Some(LowValue::Any)));
crates/lichen-lowlevel/src/function.rs:322:        // A cloned `Any` codomain reads the template requirement without running the body.
crates/lichen-lowlevel/src/function.rs:329:                self.node_value(node).and_then(|value| value.as_enum()) == Some(LowValue::Any)
crates/lichen-lowlevel/src/function.rs:450:                    .is_none_or(|value| matches!(value.as_enum(), Some(LowValue::Any)))
crates/lichen-lowlevel/src/function.rs:606:    /// `Any` is not an answer: it stays open and triggers replay only when an open instance
crates/lichen-lowlevel/src/function.rs:610:            .is_some_and(|value| !matches!(value.as_enum(), Some(LowValue::Any)))
crates/lichen-lowlevel/src/function.rs:867:        let wildcard = matches!(value.and_then(|value| value.as_enum()), Some(LowValue::Any));
crates/lichen-lowlevel/src/lib.rs:89:    /// Unification is directional: a left `Any` joins an undecided right class; every other
crates/lichen-lowlevel/src/lib.rs:90:    /// wildcard comparison succeeds without merging or writing. Readers treat `Any` as
crates/lichen-lowlevel/src/lib.rs:92:    /// remains reusable while an apply clone replaces `Any` with an open instance cell.
crates/lichen-lowlevel/src/lib.rs:93:    Any,
crates/lichen-lowlevel/src/lib.rs:113:            (LowValue::Any, LowValue::Any) => true,
crates/lichen-lowlevel/src/loop_run.rs:128:            // The selector indexes `[else, then]`. **Any other index is the
crates/lichen-lowlevel/src/loop_run.rs:222:            let any = self.add_node(block, None, Some(P::Value::from(LowValue::Any)));
crates/lichen-lowlevel/src/loop_run.rs:256:                let any = self.add_node(block, None, Some(P::Value::from(LowValue::Any)));
crates/lichen-lowlevel/src/static_module/apply.rs:475:            Some(LowValue::Any)
crates/lichen-lowlevel/src/static_module/apply.rs:567:            Some(LowValue::Any)
crates/lichen-lowlevel/src/static_module/apply.rs:763:                                Some(LowValue::Any)
crates/lichen-lowlevel/src/static_module.rs:339:                    .is_some_and(|value| matches!(value, crate::LowValue::Any));
crates/lichen-lowlevel/src/static_module.rs:503:                .is_none_or(|value| matches!(value.as_enum(), Some(LowValue::Any)))
crates/lichen-lowlevel/src/table.rs:251:            Some(LowValue::Any) => match mode {
crates/lichen-lowlevel/src/template.rs:297:        matches!(value.as_enum(), Some(LowValue::Any))
crates/lichen-lowlevel/src/template.rs:402:        matches!(value.as_enum(), Some(LowValue::Any))
crates/lichen-lowlevel/src/template.rs:539:        let wildcard = matches!(value.and_then(|value| value.as_enum()), Some(LowValue::Any));
crates/lichen-lowlevel/src/template.rs:746:        let wildcard = matches!(value.and_then(|value| value.as_enum()), Some(LowValue::Any));
crates/lichen-render/src/render/type_printer.rs:130:                LowValue::Any => "@any".to_string(),
crates/lichen-render/src/render/type_printer.rs:164:                LowValue::Any => Some("@any".to_string()),
crates/lichen-render/src/render/type_printer.rs:471:            Some(LowValue::Any) => None,
crates/lichen-render/src/render/type_printer.rs:510:            Some(LowValue::Any) => "@any".to_string(),
crates/lichen-render/src/render/value_printer.rs:104:                Some(LowValue::Any | LowValue::None | LowValue::Error)
crates/lichen-render/src/render/value_printer.rs:274:        if matches!(value.as_enum(), Some(LowValue::Any)) {
crates/lichen-render/src/render/value_printer.rs:636:            Some(LowValue::Any) => Some("@any".to_string()),
lichen-std-native/src/lib.rs:95:                    Some(LowValue::Any) => return None,
lichen-std-native/src/lib.rs:134:                        Some(<P::Value as From<LowValue>>::from(LowValue::Any)),
```

### Verbatim dump — `@any` sites in source, examples, grammar, editors, tests (881 lines; docs/ excluded)

```text
crates/lichen-compute/examples/recursion.rs:245:sum_to = (s = (_x => (@any : Int, @any : Int))) => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)
crates/lichen-compute/examples/recursion.rs:262:loop = f => (s = (_x => (@any : Int, @any : Int))) => if s(0) == 0 then s(1) else loop f (s(0) - 1, f s(1))
crates/lichen-compute/examples/recursion.rs:280:sum_to = (s = (_x => (@any : Int, @any : Int))) => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)
crates/lichen-compute/examples/recursion.rs:299:        "sum_to = (s = (_x => (@any : Int, @any : Int))) => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)\n\
crates/lichen-compute/examples/recursion.rs:356:        "{marker}sum_to = (s = (_x => (@any : Int, @any : Int))) => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)\n\
crates/lichen-compute/examples/reduction_interface.rs:252:element_raw = (p = (_x => (@any : Float, @any : Float))) => (p(0) * p(1)) ~ "element"
crates/lichen-compute/src/compute.lichen:4:KT := {.I @any, .O @any}
crates/lichen-compute/src/compute.lichen:5:K := {.native @any, .I @any, .O @any}
crates/lichen-compute/src/compute.lichen:6:A := 'I -> {.n @any : Int, .I I}
crates/lichen-compute/src/compute.lichen:7:P := 'T = KT -> {.n @any : Int, .in T.I}
crates/lichen-compute/src/compute.lichen:9:BufParam := {.type @any, .length @any}
crates/lichen-compute/src/compute.lichen:11:  .native @any
crates/lichen-compute/src/compute.lichen:23:Read := 'p = BufParam -> {.from (Buf p) # warp, .at @any : Int}
crates/lichen-compute/src/compute.lichen:24:Write := 'p = BufParam -> {.to (Buf p) # warp, .at @any : Int, .value @any : p.type}
crates/lichen-compute/src/compute.lichen:25:WriteAtomic := 'p = BufParam -> {.to (Buf p) # warp, .at @any : Int, .value @any : p.type}
crates/lichen-compute/src/compute.lichen:32:  .s @any : string
crates/lichen-compute/src/compute.rs:3607:parameter (for example `p = (@any : Int, @any : Int)` for a tuple domain, or `y : Int` for a scalar one)";
crates/lichen-compute/src/compute.rs:3611:const POLYMORPHIC_TEMPLATE: &str = "a polymorphic template cannot be jitted: the kernel parameter's domain is not decided when the kernel is compiled, and a template's parameter has no domain before an instance binds it — annotate the parameter (for example `k : Par` for a named struct, or `p = (@any : Int, @any : Int)` for a tuple)";
crates/lichen-compute/src/compute.rs:4100:/// A decided buffer payload, or `None` while its value is absent or `@any`.
crates/lichen-compute/src/tensor.lichen:4:Pair := {.row @any : Int, .column @any : Int}
crates/lichen-compute/src/tensor.lichen:9:  .origin @any : Int,
crates/lichen-compute/src/tensor.lichen:12:  .rows @any : Int,
crates/lichen-compute/src/tensor.lichen:13:  .cols @any : Int,
crates/lichen-compute/src/tensor.lichen:19:  .origin @any : Int,
crates/lichen-compute/tests/lichen_compute/automatic_tiling.rs:75:Par := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/automatic_tiling.rs:76:Arg := {{.n @any : Int, .I In}}
crates/lichen-compute/tests/lichen_compute/automatic_tiling.rs:77:GA  := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/automatic_tiling.rs:110:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/buf_element_class_direct.lichen:6:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/buf_element_class_recorded.lichen:6:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/buf_element_class_recorded.lichen:12:Arg := {.n @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/buf_element_class_recorded.lichen:13:GArg := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/chain_merge.rs:64:Par  := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/chain_merge.rs:65:Arg  := {{.n @any : Int, .I In}}
crates/lichen-compute/tests/lichen_compute/chain_merge.rs:75:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/chain_merge.rs:76:Arg2 := {{.n @any : Int, .I In2}}
crates/lichen-compute/tests/lichen_compute/chain_merge.rs:87:Par3 := {{.n @any : Int, .in In3}}
crates/lichen-compute/tests/lichen_compute/chain_merge.rs:88:Arg3 := {{.n @any : Int, .I In3}}
crates/lichen-compute/tests/lichen_compute/chain_merge.rs:97:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/chain_merge.rs:128:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/combine_fold.rs:27:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/combine_fold.rs:39:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/compile_order_class.rs:34:ParS := {.n @any : Int, .keys @any : Int, .dim @any : Int, .in QK}
crates/lichen-compute/tests/lichen_compute/compile_order_class.rs:35:ArgS := {.n @any : Int, .keys @any : Int, .dim @any : Int, .I QK}
crates/lichen-compute/tests/lichen_compute/compile_order_class.rs:47:ParA := {.n @any : Int, .dim @any : Int, .keys @any : Int, .in QSV}
crates/lichen-compute/tests/lichen_compute/compile_order_class.rs:48:ArgA := {.n @any : Int, .dim @any : Int, .keys @any : Int, .I QSV}
crates/lichen-compute/tests/lichen_compute/compile_order_class.rs:81:GArg := {.lanes @any : Int, .on @any : Int, .keys @any : Int, .dim @any : Int, .in QKV}
crates/lichen-compute/tests/lichen_compute/composed_chain.rs:199:Par  := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/composed_chain.rs:209:Par2 := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/composed_chain.rs:218:GArg := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:39:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:41:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:70:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:72:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:173:k := compute.jit ('p = (@any : Int, @any : Int) -> p(0) + p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:193:k := compute.jit ('p = (@any : Int, @any : Int, @any : Int) -> p(0) + p(1) + p(2))
crates/lichen-compute/tests/lichen_compute/compute.rs:213:k := compute.jit ('p = (@any : Int, @any : Int) -> p(0) - p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:231:k := compute.jit ('p = (@any : Int, @any : Int) -> p(0) + p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:271:k := compute.jit ('p = (@any : Int, @any : Int) -> (p(0) + p(1)) - (p(0) <= p(1)))
crates/lichen-compute/tests/lichen_compute/compute.rs:288:k2 := compute.jit ('p = (@any : Int, @any : Int) -> p(0) > p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:289:k3 := compute.jit ('p = (@any : Int, @any : Int) -> p(0) >= p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:290:k4 := compute.jit ('p = (@any : Int, @any : Int) -> p(0) != p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:291:k5 := compute.jit ('p = (@any : Int, @any : Int) -> (p(0) < p(1)) | (p(0) == p(1)))
crates/lichen-compute/tests/lichen_compute/compute.rs:292:k6 := compute.jit ('p = (@any : Int, @any : Int) -> (p(0) > p(1)) ^ (p(0) == p(1)))
crates/lichen-compute/tests/lichen_compute/compute.rs:369:k := compute.jit ('p = ((@any : Int, @any : Int), @any : Int) -> p(0)(0) + p(0)(1) + p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:462:k0 := compute.jit ('p = (@any : Int, @any : Int) -> p(0) + p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:478:k0 := compute.jit ('p = (@any : Int, @any : Int) -> p(0) - p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:479:k1 := compute.jit ('q = (@any : Int, @any : Int) -> k0 q)
crates/lichen-compute/tests/lichen_compute/compute.rs:495:k0 := compute.jit ('p = ((@any : Int, @any : Int), @any : Int) -> p(0)(0) - p(0)(1) + p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:496:k1 := compute.jit ('r = (@any : Int, ((@any : Int, @any : Int), @any : Int)) -> k0 r(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:512:k0 := compute.jit ('p = (@any : Int, @any : Int) -> p(0) + p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:557:k := compute.jit ('p = (@any : Int, @any : Int) -> (p(0), p(1)))
crates/lichen-compute/tests/lichen_compute/compute.rs:570:k := compute.jit ('p = (@any : Int, @any : Int) -> (p(0), p(0) + p(1)))
crates/lichen-compute/tests/lichen_compute/compute.rs:583:k := compute.jit ('p = (@any : Int, @any : Int) -> (p(0) + 10, p(1) + 100))
crates/lichen-compute/tests/lichen_compute/compute.rs:584:r := (compute.launch k (2, 3) = (@any : Int, @any : Int))
crates/lichen-compute/tests/lichen_compute/compute.rs:616:k := compute.jit ('p = (@any : Int, @any : Int) -> (p(0) - p(1), p(0) + p(1)))
crates/lichen-compute/tests/lichen_compute/compute.rs:638:k0 := compute.jit ('p = (@any : Int, @any : Int) -> (p(0), p(1)))
crates/lichen-compute/tests/lichen_compute/compute.rs:639:k1 := compute.jit ('q = (@any : Int, @any : Int) -> compute.launch k0 q)
crates/lichen-compute/tests/lichen_compute/compute.rs:665:k := compute.jit ('p = (@any : Int, @any : Int) -> p(0) + p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:710:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:712:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:742:In1  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:744:Par1 := {.n @any : Int, .in In1}
crates/lichen-compute/tests/lichen_compute/compute.rs:754:Par2 := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/compute.rs:779:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:781:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:806:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:808:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:837:In0  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:839:Par0 := {.n @any : Int, .in In0}
crates/lichen-compute/tests/lichen_compute/compute.rs:849:Par := {.n @any : Int, .alpha @any : Float, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:850:Host := {.n @any : Int, .alpha @any : Float, .I In}
crates/lichen-compute/tests/lichen_compute/compute.rs:886:In0  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:888:Par0 := {.n @any : Int, .in In0}
crates/lichen-compute/tests/lichen_compute/compute.rs:898:Par := {.n @any : Int, .alpha @any : Float, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:899:Host := {.n @any : Int, .alpha @any, .I In}
crates/lichen-compute/tests/lichen_compute/compute.rs:927:k := compute.jit ('x = (@any : Int, @any : Int) -> x(0))
crates/lichen-compute/tests/lichen_compute/compute.rs:979:k := compute.jit ('p = (@any : Int, @any : Int) -> p(0) + p(1))
crates/lichen-compute/tests/lichen_compute/compute.rs:1004:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1006:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1031:In1  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1033:Par1 := {.n @any : Int, .in In1}
crates/lichen-compute/tests/lichen_compute/compute.rs:1043:Par2 := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/compute.rs:1069:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1071:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1095:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1097:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1129:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1131:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1181:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1183:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1193:Par2 := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/compute.rs:1221:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1223:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1254:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1256:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1297:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1334:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1343:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1380:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1381:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1436:k := compute.jit ('p = (@any : Int, @any : Float) -> p(0))
crates/lichen-compute/tests/lichen_compute/compute.rs:1453:k := compute.jit ('p = (@any : Int, @any : Int) -> p(0))
crates/lichen-compute/tests/lichen_compute/compute.rs:1454:compute.launch k (1, ([@any : Float, 3][1.5, 2.5, 3.5] : Array))
crates/lichen-compute/tests/lichen_compute/compute.rs:1473:k := compute.jit ('p = (@any : Int, @any : Int) -> p(0))
crates/lichen-compute/tests/lichen_compute/compute.rs:1634:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1636:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1677:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1678:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1721:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1723:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1761:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1763:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/compute.rs:1785:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/compute.rs:1787:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/consumer_guard.rs:44:Par  := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/consumer_guard.rs:54:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/consumer_guard.rs:63:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/consumer_guard.rs:110:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/consumer_guard.rs:153:Par  := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/consumer_guard.rs:163:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/consumer_guard.rs:171:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/cooperative_product.rs:60:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/elementwise_chain.rs:33:ParE := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/elementwise_chain.rs:60:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/elementwise_chain.rs:69:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/elementwise_chain.rs:96:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/elementwise_chain.rs:103:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fan_in_merge.rs:44:ParM := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fan_in_merge.rs:54:ParS := {{.n @any : Int, .in Soft}}
crates/lichen-compute/tests/lichen_compute/fan_in_merge.rs:65:ParY := {{.n @any : Int, .in Norm}}
crates/lichen-compute/tests/lichen_compute/fan_in_merge.rs:74:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fan_out_merge.rs:52:ParE := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fan_out_merge.rs:53:ArgE := {{.n @any : Int, .I In}}
crates/lichen-compute/tests/lichen_compute/fan_out_merge.rs:62:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/fan_out_merge.rs:63:Arg2 := {{.n @any : Int, .I In2}}
crates/lichen-compute/tests/lichen_compute/fan_out_merge.rs:70:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fan_out_merge.rs:98:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/fixtures/gauge.lichen:1:Gauge := {.v @any : Int # 32}
crates/lichen-compute/tests/lichen_compute/fixtures/gauge.lichen:3:Choose := '_p -> {.v @any : Int # _p}
crates/lichen-compute/tests/lichen_compute/float_leaf.rs:58:Par  := {{.n @any : Int, .cols @any : Int, .scale @any : {kind}, .in In1}}
crates/lichen-compute/tests/lichen_compute/float_leaf.rs:59:Arg  := {{.n @any : Int, .cols @any : Int, .scale @any : {kind}, .I In1}}
crates/lichen-compute/tests/lichen_compute/float_leaf.rs:60:GA   := {{.n @any : Int, .cols @any : Int, .scale @any : {kind}, .in In1}}
crates/lichen-compute/tests/lichen_compute/float_leaf.rs:96:Par  := {{.n @any : Int, .scale @any : {leaf}, .in In1}}
crates/lichen-compute/tests/lichen_compute/float_leaf.rs:97:Arg  := {{.n @any : Int, .scale @any : {leaf}, .I In1}}
crates/lichen-compute/tests/lichen_compute/float_leaf.rs:98:GA   := {{.n @any : Int, .scale @any : {leaf}, .in In1}}
crates/lichen-compute/tests/lichen_compute/float_leaf.rs:148:Fill  := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/fold_datum.rs:35:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/fold_datum.rs:47:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/frozen_buffer_reuse.rs:20:InA  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/frozen_buffer_reuse.rs:22:ParA := {.n @any : Int, .in InA}
crates/lichen-compute/tests/lichen_compute/frozen_buffer_reuse.rs:30:ParB := {.n @any : Int, .in InB}
crates/lichen-compute/tests/lichen_compute/fused_region.rs:74:Par  := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fused_region.rs:84:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/fused_region.rs:93:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fused_region.rs:133:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/fuse_licence.rs:60:ParE := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fuse_licence.rs:69:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/fuse_licence.rs:76:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fuse_licence.rs:141:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/fuse_licence.rs:167:Par3 := {{.n @any : Int, .in In3}}
crates/lichen-compute/tests/lichen_compute/fuse_licence.rs:175:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fuse_licence.rs:828:Par  := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/fuse_licence.rs:838:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/fuse_licence.rs:847:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/graph_argument.rs:34:ParE := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/graph_argument.rs:43:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/graph_argument.rs:56:GArg := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/graph_argument.rs:67:GArg := {.n @any : Int, .in Expr}
crates/lichen-compute/tests/lichen_compute/graph_argument.rs:78:GArg := {.n @any : Int, .in In, .q Side}
crates/lichen-compute/tests/lichen_compute/graph_argument.rs:123:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:148:In1  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:150:Par1 := {.n @any : Int, .in In1}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:159:Par2 := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:176:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:187:InS  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:188:ParS := {.n @any : Int, .in InS}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:207:InS  := {{.a @any : Int}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:208:ParS := {{.n @any : Int, .in InS}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:249:InS  := {{.a @any : Int}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:250:ParS := {{.n @any : Int, .in InS}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:298:GArg := {{.n @any : Int, .in In1}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:341:GArg := {{.n @any : Int, .in In1}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:366:GArg := {{.n @any : Int, .in In1}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:394:step := ('ins = (@any : Int, )) -> compute.call one (ins(0))
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:430:step := ('ins = (@any : Int, )) -> ins(0)
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:451:In1  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:453:Par1 := {.n @any : Int, .in In1}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:462:Par2 := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:470:GArg := {.n @any : Int, .in In1}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:492:    let body = r#"InS  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:493:GArg := {.n @any : Int, .in InS}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:547:In1  := {{.a @any : Int}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:549:Par1 := {{.n @any : Int, .in In1}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:558:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/graph_jit.rs:566:GArg := {{.n @any : Int, .in In1}}
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:45:Par  := {.n @any : Int, .cols @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:46:Arg  := {.n @any : Int, .cols @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:65:Par2  := {.n @any : Int, .cols @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:66:Arg2  := {.n @any : Int, .cols @any : Int, .I In2}
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:80:         GA := {{.n @any : Int, .cols @any : Int, .in In}}\nstep := ('s = GA) -> {{\n  \
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:101:         Fill := {{.n @any : Int}}\nfill := ('c = Fill) -> {{\n  i := compute.range c.n\n  \
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:295:            "GA := {{.n @any : Int, .cols @any : Int, .in In2}}\nstep := ('s = GA) -> {{\n  \
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:337:        "{HEAD}\nPar3 := {{.n @any : Int, .cols @any : Int, .rows @any : Int, .in In}}\n\
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:338:         Arg3 := {{.n @any : Int, .cols @any : Int, .rows @any : Int, .I In}}\n\
crates/lichen-compute/tests/lichen_compute/graph_runtime_scalar.rs:344:         GA := {{.n @any : Int, .cols @any : Int, .rows @any : Int, .in In}}\nstep := ('s = GA) -> {{\n  \
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:164:InA  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:166:ParA := {.n @any : Int, .in InA}
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:174:ParB := {.n @any : Int, .in InB}
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:183:ParS := {.n @any : Int, .in InS}
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:255:InA  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:257:ParA := {.n @any : Int, .in InA}
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:265:ParB := {.n @any : Int, .in InB}
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:392:InA  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:394:ParA := {.n @any : Int, .in InA}
crates/lichen-compute/tests/lichen_compute/graph_structure.rs:402:ParB := {.n @any : Int, .in InB}
crates/lichen-compute/tests/lichen_compute/helper_refuses_a_parallel_refusal.rs:15:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/helper_refuses_a_parallel_refusal.rs:24:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/helper_refuses_a_parallel_refusal.rs:25:Arg := {.n @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/identity_from_body.rs:58:Par := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/identity_from_body.rs:70:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/inplace_write_refusal.lichen:6:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/inplace_write_refusal.lichen:7:Arg := {.n @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/inplace_write_refusal.lichen:8:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/lane_coordinates.rs:22:Par := {.n @any : Int, .cols @any : Int, .stride @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/lane_coordinates.rs:23:Host := {.n @any : Int, .cols @any : Int, .stride @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/lane_coordinates.rs:25:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/lane_coordinates.rs:56:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/lane_coordinates.rs:57:Host := {.n @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/lane_coordinates.rs:59:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/ln_domain.rs:75:args := (compute.buffer ([@any : Float, {}][{}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/loop_write.rs:23:Par := {.n @any : Int, .taps @any : Int}
crates/lichen-compute/tests/lichen_compute/loop_write.rs:24:Arg := {.n @any : Int, .taps @any : Int}
crates/lichen-compute/tests/lichen_compute/mark_anchor.rs:42:ParE := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/mark_anchor.rs:43:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/mark_anchor.rs:56:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/mark_anchor.rs:85:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/mark_anchor.rs:94:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/mark_anchor.rs:111:ParE := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/mixed_group.rs:34:Src := {{.a @any : Int}}
crates/lichen-compute/tests/lichen_compute/nested_config.rs:53:Meta  := {{.rows @any : Int, .cols @any : {kind}}}
crates/lichen-compute/tests/lichen_compute/nested_config.rs:89:Par3  := {{.n @any : Int, .K @any : Int, .cols @any : Int, .in In1}}
crates/lichen-compute/tests/lichen_compute/nested_config.rs:90:Arg3  := {{.n @any : Int, .K @any : Int, .cols @any : Int, .I In1}}
crates/lichen-compute/tests/lichen_compute/nested_config.rs:91:GA3   := {{.n @any : Int, .K @any : Int, .cols @any : Int, .in In1}}
crates/lichen-compute/tests/lichen_compute/nested_config.rs:137:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/nested_config.rs:175:const CARRYING: &str = "{.rows @any : Int, .cols @any : Int, .in In2}";
crates/lichen-compute/tests/lichen_compute/nested_config.rs:313:    let graph = graph_of(&nested("Float", "{.rows @any : Int, .in In2}", "2.5"));
crates/lichen-compute/tests/lichen_compute/nested_config.rs:428:Par  := {.n @any : Int, .in In2, .spare (compute.Buf (compute.BufParam{.type Int, .length 4}))}
crates/lichen-compute/tests/lichen_compute/nested_config.rs:429:Arg  := {.n @any : Int, .in In2, .spare (compute.Buf (compute.BufParam{.type Int, .length 4}))}
crates/lichen-compute/tests/lichen_compute/nested_role.rs:43:Src := {{.a @any : Int}}
crates/lichen-compute/tests/lichen_compute/nested_role.rs:45:SrcPar := {{.n @any : Int, .in Src}}
crates/lichen-compute/tests/lichen_compute/pack_placement.rs:53:Par := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/pack_placement.rs:66:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/perspective_requirements.rs:24:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/perspective_requirements.rs:100:        "Gauge := {.v @any : Int # 32}\nstrict := ('g = Gauge) -> g.v\nstrict (Gauge{.v (5 # 1)})\n",
crates/lichen-compute/tests/lichen_compute/perspective_requirements.rs:147:        "Choose := '_p -> {.v @any : Int # _p}\npick := (g = (Choose _)) -> g.v\npick ((Choose 32){.v (5 # 1)})\n",
crates/lichen-compute/tests/lichen_compute/plan_staging.rs:103:Par := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/recompute_reads.rs:46:ParE := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/recompute_reads.rs:55:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/recompute_reads.rs:64:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/recompute_reads.rs:104:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/reduction_chain.rs:107:Par := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/refusal_over_an_answer.rs:18:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/refusal_over_an_answer.rs:19:Arg := {.n @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/refusal_over_an_answer.rs:20:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/rmsnorm_denominator.rs:36:ParS := {{.n @any : Int, .in Row}}
crates/lichen-compute/tests/lichen_compute/rmsnorm_denominator.rs:46:ParY  := {{.n @any : Int, .in Scale}}
crates/lichen-compute/tests/lichen_compute/rmsnorm_denominator.rs:56:GArg := {{.n @any : Int, .in All}}
crates/lichen-compute/tests/lichen_compute/runtime_scalar_leaf.rs:65:ParS  := {{.n @any : Int, .in Row}}
crates/lichen-compute/tests/lichen_compute/runtime_scalar_leaf.rs:74:ParQ  := {{.n @any : Int, .in Row}}
crates/lichen-compute/tests/lichen_compute/runtime_scalar_leaf.rs:84:ParY    := {{.n @any : Int, .eps @any : Float, .in Moments}}
crates/lichen-compute/tests/lichen_compute/runtime_scalar_leaf.rs:98:GArg  := {{.n @any : Int, .eps @any : Float, .in All}}
crates/lichen-compute/tests/lichen_compute/runtime_scalar_leaf.rs:99:ArgY  := {{.n @any : Int, .eps @any : Float, .I Moments}}
crates/lichen-compute/tests/lichen_compute/runtime_scalar_leaf.rs:142:Fill := {{.n @any : Int}}
crates/lichen-compute/tests/lichen_compute/sibling_diamond.rs:117:ParS  := {{.n @any : Int, .in Row}}
crates/lichen-compute/tests/lichen_compute/sibling_diamond.rs:126:ParQ  := {{.n @any : Int, .in Row}}
crates/lichen-compute/tests/lichen_compute/sibling_diamond.rs:136:ParY    := {{.n @any : Int, .in Moments}}
crates/lichen-compute/tests/lichen_compute/sibling_diamond.rs:150:GArg  := {{.n @any : Int, .in All}}
crates/lichen-compute/tests/lichen_compute/sibling_diamond.rs:176:ParS  := {{.n @any : Int, .in Row}}
crates/lichen-compute/tests/lichen_compute/sibling_diamond.rs:185:ParQ  := {{.n @any : Int, .in Row}}
crates/lichen-compute/tests/lichen_compute/sibling_diamond.rs:195:ParY    := {{.n @any : Int, .in Moments}}
crates/lichen-compute/tests/lichen_compute/sibling_diamond.rs:205:GArg := {{.n @any : Int, .in Row}}
crates/lichen-compute/tests/lichen_compute/silu_exp_nested.rs:35:ParA  := {{.n @any : Int, .in Gate}}
crates/lichen-compute/tests/lichen_compute/sparse_input_classes.rs:39:Par  := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/sparse_input_classes.rs:48:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/sparse_input_classes.rs:56:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/swallowed_refusal.rs:17:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/swallowed_refusal.rs:26:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/swallowed_refusal.rs:27:Arg := {.n @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/tiled_reduction.rs:137:Par := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/tiling_visibility.rs:37:Par  := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/tiling_visibility.rs:47:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/tiling_visibility.rs:56:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/trig_domain.rs:127:angles := (compute.buffer ([@any : Float, {}][{}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/union_ports.rs:35:Par  := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/union_ports.rs:45:Par2 := {{.n @any : Int, .in In2}}
crates/lichen-compute/tests/lichen_compute/union_ports.rs:53:GArg := {{.n @any : Int, .in In}}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:18:MPar := {.n @any : Int, .w @any : Int, .least @any : Float, .in MIn, .out MOut}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:19:MArg := {.n @any : Int, .w @any : Int, .least @any : Float, .I MIn}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:28:HPar := {.n @any : Int, .big @any : Int, .in HIn, .out HOut}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:29:HArg := {.n @any : Int, .big @any : Int, .I HIn}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:40:FPar := {.n @any : Int, .big @any : Float, .in FIn, .out FOut}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:41:FArg := {.n @any : Int, .big @any : Float, .I FIn}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:52:TPar := {.n @any : Int, .w @any : Int, .big @any : Int, .in TIn, .out TOut}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:53:TArg := {.n @any : Int, .w @any : Int, .big @any : Int, .I TIn}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:62:UPar := {.n @any : Int, .w @any : Int, .big @any : Float, .in UIn, .out UOut}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:63:UArg := {.n @any : Int, .w @any : Int, .big @any : Float, .I UIn}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:70:ZIn  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:72:ZPar := {.n @any : Int, .in ZIn, .out ZOut}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:73:ZArg := {.n @any : Int, .I ZIn}
crates/lichen-compute/tests/lichen_compute/zz_argmax_probe.rs:79:x := (compute.buffer ([@any : Float, 8][1.0, 3.0, 2.0, 7.0, 0.5, 9.0, 4.0, 2.5] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
crates/lichen-compute/tests/lichen_compute/zz_contract_probe.rs:15:Meta := {.n @any : Int, .k @any : Int, .c1 @any : Int, .ar @any : Int, .ac @any : Int, .br @any : Int, .bc @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_contract_probe.rs:20:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:18:Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:26:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:37:Shape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:169:PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:195:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:219:Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:227:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:238:PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:343:PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:369:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:376:PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:402:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:425:Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:433:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:444:PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:527:T := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:555:In1  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:557:Par1 := {.n @any : Int, .in In1}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:566:Par2 := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:574:In2A := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:591:Sum := {.n @any : Int, .in Buffers}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:593:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:653:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:687:In2 := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:718:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:743:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:781:Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:789:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:800:PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:863:Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:871:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:882:PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:964:Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:968:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:979:PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1068:  Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1071:  Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1161:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1162:Par := {.n @any : Int, .in B1}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1238:Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1241:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1403:Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1411:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1422:PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1554:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1566:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1578:T := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1590:T := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1602:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1615:T := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1660:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1669:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1694:/// Print-only: an `@any` field takes the supplied value (`@any` is undecided and always
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1702:T := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1710:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1751:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1759:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1768:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_contract_probe.rs:1777:Inner := {.x @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_trip_probe.rs:19:PPar := {.n @any : Int, .least @any : Float, .in PIn, .out POut}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_trip_probe.rs:20:PArg := {.n @any : Int, .least @any : Float, .I PIn}
crates/lichen-compute/tests/lichen_compute/zz_dynamic_trip_probe.rs:28:x := (compute.buffer ([@any : Float, 6][1.0, 2.0, 4.0, 8.0, 16.0, 32.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
crates/lichen-compute/tests/lichen_compute/zz_dynamic_trip_probe.rs:33:lim := (compute.buffer ([@any : Int, 1][3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_dynamic_trip_probe.rs:40:lim := (compute.buffer ([@any : Int, 1][5] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_fold_struct_probe.rs:34:XPar := {.n @any : Int, .alpha @any : Float, .in XIn, .out XOut}
crates/lichen-compute/tests/lichen_compute/zz_fold_struct_probe.rs:35:XArg := {.n @any : Int, .alpha @any : Float, .I XIn}
crates/lichen-compute/tests/lichen_compute/zz_fold_struct_probe.rs:44:x := (compute.buffer ([@any : Float, 6][4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
crates/lichen-compute/tests/lichen_compute/zz_fold_struct_probe.rs:57:@loop march := ((s = (('_x -> (@any : Int, XOut)) _)) -> if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x s(1).y}}) : Struct)))
crates/lichen-compute/tests/lichen_compute/zz_fold_struct_probe.rs:68:@loop march := ((s = (('_x -> (@any : Int, XOut)) _)) -> if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x s(1).y}}) : Struct)))
crates/lichen-compute/tests/lichen_compute/zz_fold_type_probe.rs:18:XPar := {.n @any : Int, .alpha @any : Float, .in XIn, .out XOut}
crates/lichen-compute/tests/lichen_compute/zz_fold_type_probe.rs:19:XArg := {.n @any : Int, .alpha @any : Float, .I XIn}
crates/lichen-compute/tests/lichen_compute/zz_fold_type_probe.rs:28:x := (compute.buffer ([@any : Float, 6][4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
crates/lichen-compute/tests/lichen_compute/zz_fold_type_probe.rs:58:@loop run := ((s = (('_x -> (@any : Int, @any : Array)) _)) -> if s(0) == 0 then s(1) else run (s(0) - 1, (compute.collect (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x (compute.buffer (s(1)) : (compute.Buf (compute.BufParam{.type Float, .length _})))}}) : Struct).y : Array)))
crates/lichen-compute/tests/lichen_compute/zz_frozen_body_probe.rs:23:Par := {.n @any : Int, .taps @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_frozen_body_probe.rs:24:Arg := {.n @any : Int, .taps @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/zz_frozen_body_probe.rs:41:Par := {.n @any : Int, .taps @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_frozen_body_probe.rs:42:Arg := {.n @any : Int, .taps @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/zz_frozen_body_probe.rs:43:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_frozen_body_probe.rs:68:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_frozen_body_probe.rs:87:Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_frozen_body_probe.rs:148:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_frozen_body_probe.rs:273:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_gather_address_probe.rs:17:PPar := {.n @any : Int, .in PIn}
crates/lichen-compute/tests/lichen_compute/zz_gather_address_probe.rs:18:PArg := {.n @any : Int, .I PIn}
crates/lichen-compute/tests/lichen_compute/zz_gather_address_probe.rs:26:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:22:In1  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:30:GArg := {.n @any : Int, .in In1}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:40:In1  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:48:GArg := {.n @any : Int, .in In1}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:67:GArg := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:86:GArg := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:91:src := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:97:In1  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:114:GArg := {.n @any : Int, .in In1}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:133:GArg := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:138:src := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:152:GArg := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:158:src := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:172:GArg := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:177:src := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:184:Par  := {.n @any : Int, .kdim @any : Int, .in InG, .out OutG}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:185:ArgK := {.n @any : Int, .kdim @any : Int, .I InG}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:194:w := (compute.buffer ([@any : Float, 12][1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:195:x := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:203:Par  := {.n @any : Int, .in InG, .out OutG}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:204:ArgK := {.n @any : Int, .I InG}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:213:w := (compute.buffer ([@any : Float, 12][1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:214:x := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:230:GArg := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:235:p := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:236:q := (compute.buffer ([@any : Int, 3][10, 20, 30] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:251:GArg := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:254:p := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:255:q := (compute.buffer ([@any : Float, 3][10.0, 20.0, 30.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:270:GArg := {.n @any : Int, .in InG}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:273:w := (compute.buffer ([@any : Float, 4][1.0, 2.0, 3.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:274:x := (compute.buffer ([@any : Float, 4][1.0, 2.0, 3.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:290:GArg := {.n @any : Int, .in In2}
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:293:p := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs:294:q := (compute.buffer ([@any : Float, 3][10.0, 20.0, 30.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_graph_bound_probe.rs:19:LPar := {.n @any : Int, .in LIn, .out LOut}
crates/lichen-compute/tests/lichen_compute/zz_graph_bound_probe.rs:32:SPar := {.n @any : Int, .in SIn, .out SOut}
crates/lichen-compute/tests/lichen_compute/zz_graph_bound_probe.rs:40:GArg := {.n @any : Int, .in SIn}
crates/lichen-compute/tests/lichen_compute/zz_graph_bound_probe.rs:41:x := (compute.buffer ([@any : Float, 6][1.0, 2.0, 4.0, 8.0, 16.0, 32.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
crates/lichen-compute/tests/lichen_compute/zz_graph_bound_probe.rs:42:d := (compute.buffer ([@any : Int, 1][2] : Array) : (compute.Buf (compute.BufParam{.type Int, .length _})))
crates/lichen-compute/tests/lichen_compute/zz_graph_scalar_probe.rs:18:PPar := {.n @any : Int, .off @any : Int, .in PIn, .out POut}
crates/lichen-compute/tests/lichen_compute/zz_graph_scalar_probe.rs:19:PArg := {.n @any : Int, .off @any : Int, .I PIn}
crates/lichen-compute/tests/lichen_compute/zz_graph_scalar_probe.rs:26:x := (compute.buffer ([@any : Int, 4][1, 2, 3, 4] : Array) : (compute.Buf (compute.BufParam{.type Int, .length _})))
crates/lichen-compute/tests/lichen_compute/zz_graph_scalar_probe.rs:31:GArg := {.n @any : Int, .off @any : Int, .in PIn}
crates/lichen-compute/tests/lichen_compute/zz_graph_scalar_probe.rs:41:GArg := {.n @any : Int, .off @any : Int, .in PIn}
crates/lichen-compute/tests/lichen_compute/zz_graph_scalar_probe.rs:51:GArg := {.n @any : Int, .off @any : Int, .in PIn}
crates/lichen-compute/tests/lichen_compute/zz_graph_scalar_probe.rs:61:GArg := {.n @any : Int, .off @any : Int, .in PIn}
crates/lichen-compute/tests/lichen_compute/zz_graph_step_shape_probe.rs:51:data := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
crates/lichen-compute/tests/lichen_compute/zz_graph_step_shape_probe.rs:56:GArg := {.n @any : Int, .in Inc, .out Out}
crates/lichen-compute/tests/lichen_compute/zz_graph_step_shape_probe.rs:66:GArg := {.n @any : Int, .in Two}
crates/lichen-compute/tests/lichen_compute/zz_graph_step_shape_probe.rs:76:GArg := {.n @any : Int, .in Two}
crates/lichen-compute/tests/lichen_compute/zz_graph_step_shape_probe.rs:86:GArg := {.n @any : Int, .in Inc}
crates/lichen-compute/tests/lichen_compute/zz_graph_step_shape_probe.rs:96:GArg := {.n @any : Int, .in Inc}
crates/lichen-compute/tests/lichen_compute/zz_if_in_fold_probe.rs:26:TPar := {{.n @any : Int, .taps @any : Int, .in TIn, .out TOut}}
crates/lichen-compute/tests/lichen_compute/zz_if_in_fold_probe.rs:27:TArg := {{.n @any : Int, .taps @any : Int, .I TIn}}
crates/lichen-compute/tests/lichen_compute/zz_if_in_fold_probe.rs:34:x := (compute.buffer ([@any : Float, 2][1.0, 1.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_loop_write_probe.rs:26:Par := {.n @any : Int, .taps @any : Int, .in In, .out Out}
crates/lichen-compute/tests/lichen_compute/zz_loop_write_probe.rs:27:Arg := {.n @any : Int, .taps @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/zz_loop_write_probe.rs:33:x := (compute.buffer ([@any : Float, 2][1.0, 1.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:19:CPar := {.n @any : Int, .cols @any : Int, .taps @any : Int, .in CIn, .out COut}
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:20:CArg := {.n @any : Int, .cols @any : Int, .taps @any : Int, .I CIn}
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:30:x := (compute.buffer ([@any : Float, 25][0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:31:w := (compute.buffer ([@any : Float, 9][0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:41:CPar := {.n @any : Int, .cols @any : Int, .taps @any : Int, .in CIn, .out COut}
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:42:CArg := {.n @any : Int, .cols @any : Int, .taps @any : Int, .I CIn}
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:52:x := (compute.buffer ([@any : Float, 25][0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:53:w := (compute.buffer ([@any : Float, 9][0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125, 0.125] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:63:TPar := {.n @any : Int, .taps @any : Int, .in TIn, .out TOut}
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:64:TArg := {.n @any : Int, .taps @any : Int, .I TIn}
crates/lichen-compute/tests/lichen_compute/zz_masked_conv_probe.rs:71:x := (compute.buffer ([@any : Float, 2][1.0, 1.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_nested_loop_probe.rs:14:Src := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_nested_loop_probe.rs:16:SrcPar := {.n @any : Int, .in Src}
crates/lichen-compute/tests/lichen_compute/zz_nested_loop_probe.rs:26:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:26:Par  := {.n @any : Int, .kdim @any : Int, .in InG, .out OutG}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:27:ArgK := {.n @any : Int, .kdim @any : Int, .I InG}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:36:w := (compute.buffer ([@any : Float, 12][1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:37:x := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:51:XPar := {.n @any : Int, .alpha @any : Float, .in XIn, .out XOut}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:52:XArg := {.n @any : Int, .alpha @any : Float, .I XIn}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:63:x := (compute.buffer ([@any : Float, 6][4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:76:XPar := {.n @any : Int, .alpha @any : Float, .in XIn, .out XOut}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:77:XArg := {.n @any : Int, .alpha @any : Float, .I XIn}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:88:x := (compute.buffer ([@any : Float, 6][4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:97:GPar := {.n @any : Int, .cols @any : Int, .alpha @any : Float, .in GIn, .out GOut}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:98:GArg := {.n @any : Int, .cols @any : Int, .alpha @any : Float, .I GIn}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:113:plane := (compute.buffer ([@any : Float, 9][0.0, 0.0, 4.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:150:            r#"SArg := {{.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .m @any : Int, .in DIn}}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:164:KPar  := {{.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .in DIn, .out KVOut}}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:165:KArg  := {{.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .I DIn}}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:180:GPar := {{.n @any : Int, .kdim @any : Int, .in GIn, .out GOut}}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:181:GArg := {{.n @any : Int, .kdim @any : Int, .I GIn}}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:188:k0 := (compute.buffer ([@any : Float, {count}][{cache}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:189:v0 := (compute.buffer ([@any : Float, {count}][{values}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:190:nk := (compute.buffer ([@any : Int, 1][{key}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:191:nv := (compute.buffer ([@any : Int, 1][{value}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:192:q  := (compute.buffer ([@any : Float, {dim}][{query}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:211:FPar := {.n @any : Int, .taps @any : Int, .in FIn, .out FOut}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:212:FArg := {.n @any : Int, .taps @any : Int, .I FIn}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:221:x := (compute.buffer ([@any : Float, 6][1.0, 2.0, 3.0, 4.0, 5.0, 6.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:222:w := (compute.buffer ([@any : Float, 3][1.0, 2.0, 1.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:231:RPar := {.n @any : Int, .kdim @any : Int, .in RIn, .out ROut}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:232:RArg := {.n @any : Int, .kdim @any : Int, .I RIn}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:241:x := (compute.buffer ([@any : Float, 8][1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:250:SPar := {.n @any : Int, .k @any : Float, .in SIn, .out SOut}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:251:SArg := {.n @any : Int, .k @any : Float, .I SIn}
crates/lichen-compute/tests/lichen_compute/zz_operator_floor_probe.rs:260:x := (compute.buffer ([@any : Float, 4][1.0, 2.0, 3.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:18:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:19:FillArg := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:27:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:28:Arg := {.n @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:65:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:101:LPar := {.n @any : Int, .taps @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:138:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:173:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:208:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:246:Param := {.type @any, .length @any}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:248:buf := ('x = Param) -> (BufT x){.native 0, .type @any, .length @any}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:278:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:286:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:287:Arg := {.n @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:323:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:331:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_pure_kernel_probe.rs:332:Arg := {.n @any : Int, .I In}
crates/lichen-compute/tests/lichen_compute/zz_replicate_address_probe.rs:24:APar := {.n @any : Int, .cols @any : Int, .taps @any : Int, .in AIn, .out AOut}
crates/lichen-compute/tests/lichen_compute/zz_replicate_address_probe.rs:49:TPar := {.n @any : Int, .cols @any : Int, .in TIn, .out TOut}
crates/lichen-compute/tests/lichen_compute/zz_store_loop_probe.rs:34:In  := {.a @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_store_loop_probe.rs:36:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_subrange_write_probe.rs:18:PPar := {.n @any : Int, .off @any : Int, .in PIn, .out POut}
crates/lichen-compute/tests/lichen_compute/zz_subrange_write_probe.rs:19:PArg := {.n @any : Int, .off @any : Int, .I PIn}
crates/lichen-compute/tests/lichen_compute/zz_subrange_write_probe.rs:26:x := (compute.buffer ([@any : Int, 4][1, 2, 3, 4] : Array) : (compute.Buf (compute.BufParam{.type Int, .length _})))
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:19:Pair := {.row @any : Int, .column @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:100:Tensor := 's -> {.shape s, .value @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:134:Pair := {.row @any : Int, .column @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:167:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:190:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:200:ReadPar := {.n @any : Int, .in ReadIn}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:228:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:237:Pair := {.row @any : Int, .column @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:238:Tensor := {.buffer (compute.Buf (compute.BufParam{.type Float, .length _})), .shape Pair, .stride Pair, .origin @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:240:ReadPar := {.n @any : Int, .in ReadIn}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:270:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:280:Pair := {.row @any : Int, .column @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:281:Tensor := {.buffer (compute.Buf (compute.BufParam{.type Float, .length _})), .shape Pair, .stride Pair, .origin @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:283:Par := {.m @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:319:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:330:Par := {.m @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:366:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:433:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:443:Par := {.n @any : Int, .in In}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:600:Pair := {.row @any : Int, .column @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:607:Pair := {.row @any : Int, .column @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:623:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:632:Pair := {.row @any : Int, .column @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:633:T6 := {.buffer (compute.Buf (compute.BufParam{.type Float, .length 6})), .shape Pair, .stride Pair, .origin @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:642:Fill := {.n @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:651:Pair := {.row @any : Int, .column @any : Int}
crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs:652:TOpen := {.buffer (compute.Buf (compute.BufParam{.type Float, .length _})), .shape Pair, .stride Pair, .origin @any : Int}
crates/lichen-highlevel/src/attr.rs:118:    /// when a literal states nothing there (`@any`).
crates/lichen-highlevel/src/checker/annotations.rs:550:    /// element slot positions are `@any`, so the lowlevel compare is vacuous by
crates/lichen-highlevel/src/checker/annotations.rs:616:    /// its arm vacuous — which is exactly why `@any : T` is no pin.
crates/lichen-highlevel/src/program.rs:425:/// The wildcard literal `@any` — a value that unifies with everything and
crates/lichen-highlevel/src/program.rs:430:/// what `@any : T` binds, and a shared cell would make the first annotation bind
crates/lichen-highlevel/tests/highlevel/checker.rs:23:/// The wildcard `@any` — a value that unifies with everything and constrains
crates/lichen-highlevel/tests/highlevel/checker.rs:35:/// The wildcard bound to `ty` — the `@any : T` a template element spells.
crates/lichen-highlevel/tests/highlevel/checker.rs:314:    // (\x. x) : (@any : int, @any : int) — a tuple template is a value, not a
crates/lichen-highlevel/tests/highlevel/checker.rs:327:        "(\\x. x) : (@any : int, @any : int) must fail (a lambda is not a tuple)"
crates/lichen-highlevel/tests/highlevel/checker.rs:367:    // [@any : int, 3] — the value is the element template and three holes; the
crates/lichen-highlevel/tests/highlevel/checker.rs:374:    assert!(b.ok, "[@any : int, 3] should check");
crates/lichen-highlevel/tests/highlevel/checker.rs:402:    // [@any : int, 3] : Type — the array template's own type is the `Array`
crates/lichen-highlevel/tests/highlevel/checker.rs:427:    // (\x. x) : [@any : int, 3] — a function's type is not an array's.
crates/lichen-highlevel/tests/highlevel/checker.rs:448:    // [@any : int, 2][1, 2] — the type is the shared `Array` template; the
crates/lichen-highlevel/tests/highlevel/checker.rs:481:    // [@any : int, 2][1, 2] — every element is matched against its own clone
crates/lichen-highlevel/tests/highlevel/checker.rs:496:    // [@any : int, 3][1, 2] — the template's length is the instance's.
crates/lichen-highlevel/tests/highlevel/checker.rs:520:    // [@any : int, 2][1, int] — the second element's type is Type; the
crates/lichen-highlevel/tests/highlevel/checker.rs:597:    // (1, 2) = (@any : int, @any : int) — both sides are evaluated values.
crates/lichen-highlevel/tests/highlevel/checker.rs:610:    assert!(b.ok, "(1, 2) against (@any : int, @any : int) should check");
crates/lichen-highlevel/tests/highlevel/checker.rs:616:    // (1, 2) = (@any : int) — the template states one element.
crates/lichen-highlevel/tests/highlevel/checker.rs:627:        "(1, 2) against (@any : int) must fail (length mismatch)"
crates/lichen-highlevel/tests/highlevel/checker.rs:832:    // (\x. x) : (@any : Type, @any : Int) — a function's type is not a tuple's.
crates/lichen-highlevel/tests/highlevel/checker.rs:929:    // (1, 2) = (@any : int)
crates/lichen-highlevel/tests/highlevel/checker.rs:1133:    // [@any : int, 3][1, 2, 3][1] — the element sits one slot past the
crates/lichen-highlevel/tests/highlevel/checker.rs:1261:    // {.f @any : int, .t @any : Type} — the value holds the name table, the
crates/lichen-highlevel/tests/highlevel/checker.rs:1268:    assert!(b.ok, "{{.f @any : int, .t @any : Type}} should check");
crates/lichen-highlevel/tests/highlevel/checker.rs:1338:    // {.a @any : int, .b @any : Type} — the value's first slot maps each name
crates/lichen-highlevel/tests/highlevel/checker.rs:1482:    // 5 : {.f @any : int} — the literal's int type conflicts with the struct
crates/lichen-highlevel/tests/highlevel/checker.rs:1490:    assert!(!b.ok, "5 : {{.f @any : int}} must fail");
crates/lichen-highlevel/tests/highlevel/checker.rs:1547:    // (\x. (x : {.f @any : int})) 5 — the apply-time unify checks the argument.
crates/lichen-highlevel/tests/highlevel/checker.rs:1792:        "(@any : int, @any : int){{.f 1, .g 2}} must be rejected"
crates/lichen-highlevel/tests/highlevel/checker.rs:1907:    // [@any : int, _][1] — the length is a value now, so a placeholder
crates/lichen-highlevel/tests/highlevel/checker.rs:1942:    // [@any : int, 2][1, @shallow(1) 2] — the marker masks the element's
crates/lichen-highlevel/tests/highlevel/checker.rs:1973:    // [@any : int, 2][1, @shallow(1) (x => x + 1) 5] — the apply sits at the
crates/lichen-highlevel/tests/highlevel/checker.rs:2021:    // [@any : int, 2][1, @shallow(1) (x => x + 1) 5][1] — the read runs the
crates/lichen-highlevel/tests/highlevel/checker.rs:2328:/// Two **separately written** occurrences of `{.x @any : int}` are two distinct
crates/lichen-highlevel/tests/highlevel/checker.rs:2392:    // The regression `f = p => { p = (@any : int, @any : int); p(0) }`;
crates/lichen-language/src/core.lichen:3:add := 'operands -> { let Pair := (@any ! in_num, @any ! in_num); operands = Pair; operands(0) + operands(1) }
crates/lichen-language/src/core.lichen:4:sub := 'operands -> { let Pair := (@any ! in_num, @any ! in_num); operands = Pair; operands(0) - operands(1) }
crates/lichen-language/src/core.lichen:5:mul := 'operands -> { let Pair := (@any ! in_num, @any ! in_num); operands = Pair; operands(0) * operands(1) }
crates/lichen-language/src/core.lichen:6:div := 'operands -> { let Pair := (@any ! in_num, @any ! in_num); operands = Pair; operands(0) / operands(1) }
crates/lichen-language/src/core.lichen:7:less := 'operands -> { let Pair := (@any ! in_num, @any ! in_num); operands = Pair; operands(0) < operands(1) }
crates/lichen-language/src/core.lichen:8:greater := 'operands -> { let Pair := (@any ! in_num, @any ! in_num); operands = Pair; operands(0) > operands(1) }
crates/lichen-language/src/core.lichen:9:less_or_equal := 'operands -> { let Pair := (@any ! in_num, @any ! in_num); operands = Pair; operands(0) <= operands(1) }
crates/lichen-language/src/core.lichen:10:greater_or_equal := 'operands -> { let Pair := (@any ! in_num, @any ! in_num); operands = Pair; operands(0) >= operands(1) }
crates/lichen-language/src/tests/render_tests.rs:100:        output("A := {.f @any : Int, .t @any : Type}\nA"),
crates/lichen-language/src/tests/render_tests.rs:101:        "(@any, @any): Struct"
crates/lichen-language/src/tests/render_tests.rs:108:        output("A := {.f @any : Int, .t @any : Type}\na := A{.f 1, .t Int}\n(A, a, a.f, a.t)"),
crates/lichen-language/src/tests/render_tests.rs:109:        "((@any, @any), (1, Int), 1, Int): Tuple"
crates/lichen-language/src/tests/render_tests.rs:117:        output("B := {.f @any : Int}\nb := B{.f 1}\n(B, b)"),
crates/lichen-language/src/tests/render_tests.rs:118:        "((@any,), (1,)): Tuple"
crates/lichen-language/src/tests/render_tests.rs:132:        output("T := [@any : Int, 3]\nT[1, 2, 3]"),
crates/lichen-language/src/tests/render_tests.rs:213:        with_type_of("T := [@any, 2]\ntype_of T[1, 2]"),
crates/lichen-language/tests/language/block_boundary.rs:84:        "A := [@any, 1]\ny := A[@gc ('x -> x + 1),]\n(y[0]) 3",
crates/lichen-language/tests/language/block_boundary.rs:86:        "A := [@any, 2]\nx := A[@gc 1, 2]\nx[0]",
crates/lichen-language/tests/language/callee_inference.rs:31:            "Param := {.type @any, .length @any}\n\
crates/lichen-language/tests/language/callee_inference.rs:46:            "Param := {.type @any, .length @any}\n\
crates/lichen-language/tests/language/callee_inference.rs:62:        refused("Param := {.type @any, .length @any}\ne := _{.type Float, .length 192}");
crates/lichen-language/tests/language/cyclic_type_cut.rs:27:        root_type("A := [@any, 2]\nf := 'x -> A[x, @shallow f (x + 1.0)]; inf := f 0.0; inf"),
crates/lichen-language/tests/language/cyclic_type_cut.rs:38:            "A := [@any, 2]\nf := 'x -> A[x, @shallow g x]; g := 'y -> A[y, @shallow f y]; inf := f 0.0; inf"
crates/lichen-language/tests/language/doc.rs:9:/// `Doc := {.name @any : string, .description @any : string}` — a
crates/lichen-language/tests/language/doc.rs:11:const DOC: &str = "Doc := {.name @any : string, .description @any : string}\n";
crates/lichen-language/tests/language/doc.rs:56:        "{DOC}Point := {{.x @any : Int, .y @any : Int}} ? Doc{{.name \"Point\", .description \"a point\"}}\n(Point, Point{{.x 1, .y 2}})"
crates/lichen-language/tests/language/doc.rs:68:        "{DOC}A := [@any, 2]\nA[1 ? Doc{{.name \"x\", .description \"a\"}}, 2 ? Doc{{.name \"y\", .description \"b\"}}][0]"
crates/lichen-language/tests/language/field_read_kinds.rs:49:        "S := {.a @any : string}
crates/lichen-language/tests/language/field_read_kinds.rs:64:            "Inner := {.b @any : Int}
crates/lichen-language/tests/language/field_read_kinds.rs:65:Outer := {.a @any}
crates/lichen-language/tests/language/field_read_kinds.rs:80:        undecided_read("f := 'x -> x(0)\nA := [@any, 2]\nf A[10, 20]", "tuple read"),
crates/lichen-language/tests/language/field_read_kinds.rs:89:    let (message, kind) = refused("A := [@any, 2]\nl := A[10, 20]\nl(0)");
crates/lichen-language/tests/language/field_read_kinds.rs:101:        refused("S := {.x @any : Int, .y @any : Type}\ns := S{.x 1, .y Int}\ns(0)");
crates/lichen-language/tests/language/field_read_kinds.rs:121:        refused("T := (@any : Int, @any : Int)\nf := ('x = T) -> x[0]\nf (10, 20)");
crates/lichen-language/tests/language/field_read_kinds.rs:157:        output("A := {.f @any : Int, .g @any : Type}\na := A{.f 1, .g Type}\n(a.f, a.g)"),
crates/lichen-language/tests/language/field_read_kinds.rs:169:    let (message, kind) = refused("A := [@any, 2]\nl := A[10, 20]\nl.a");
crates/lichen-language/tests/language/field_read_kinds.rs:181:        undecided_read("f := 'x -> x.a\nA := [@any, 2]\nf A[10, 20]", "field read",),
crates/lichen-language/tests/language/field_read_kinds.rs:193:            "S := {.a @any : Int, .b @any : string}\nf := ('x = S) -> x.a\nf (S{.a 1, .b \"h\"})"
crates/lichen-language/tests/language/frozen_function_type.rs:63:        "K := {.native @any, .I @any, .O @any}\nlaunch := 'k = K -> 'r -> {r = k.O}\nwrap := 'f -> 'x -> {I := '_x -> _; f = ('i = (I _) -> _); x = (I _); O := f (I _); r := f x; launch (K{.native _, .I I, .O O}) r}",
crates/lichen-language/tests/language/gc_block.rs:93:    let out = run::evaluate("A := [@any, 1]\ny := A[@gc ('x -> x + 1),]\n(y[0]) 3")
crates/lichen-language/tests/language/gc_block.rs:102:        "A := [@any, 2]\nadd := 'x -> 'y -> x + y\ndouble := 'n -> @gc { a := n + 1\nb := add a a\nb }\n@gc A[double 3, double 7]",
crates/lichen-language/tests/language/link_arity.rs:179:const STRUCTS: &str = "GridName := {.path @any : string, .name @any : string}\n\
crates/lichen-language/tests/language/link_arity.rs:180:    WriteSphere := {.radius @any : Float, .voxel @any : Float, .path @any : string, .name @any : string}\n\
crates/lichen-language/tests/language/link_arity.rs:181:    Voxels := {.kind @any : Type, .path @any : string, .name @any : string}\n";
crates/lichen-language/tests/language/merge_carry.rs:16:        "T := (@any : Int, @any : Int)\nf := ('x = T) -> {\n  a := x(0) + x(0)\n  p := x = T\n  a\n}\nf (10, 20)",
crates/lichen-language/tests/language/one_value.rs:16:S := {.a @any : Int}
crates/lichen-language/tests/language/one_value.rs:52:S := {.a @any : Int}
crates/lichen-language/tests/language/pipeline.rs:147:            "('p = (@any : Int, @any : Int) -> p(0)) (1, 2) : Int"
crates/lichen-language/tests/language/pipeline.rs:155:            "f := 'p = (@any : Int, @any : Int) -> p(0)\nf (1, 2)"
crates/lichen-language/tests/language/pipeline.rs:166:            "sum3 := [@any : Int, 3]['a, 'b, 'c] -> 'a + 'b + 'c\n\
crates/lichen-language/tests/language/pipeline.rs:207:        "A := [@any, 4]\na := 1; f1 := 'x -> { b := 2; f2 := 'y -> A[a, b, x, y]; f2 }; f1 3 4",
crates/lichen-language/tests/language/pipeline.rs:224:        evaluated_array_element_ids("A := [@any : Int, 3]\n(A[1, 2, 3] : Array)").len(),
crates/lichen-language/tests/language/pipeline.rs:237:        evaluated_array_element_ids("A := [@any, 2]\nA['x -> x, 'x -> x]").len(),
crates/lichen-language/tests/language/pipeline.rs:241:        evaluated_array_element_ids("A := [@any, 2]\nA['y -> y, 'x -> x]").len(),
crates/lichen-language/tests/language/pipeline.rs:251:        usize_of(&evaluate("A := [@any : Int, 3]\n(A[1, 2, 3])[1]")),
crates/lichen-language/tests/language/pipeline.rs:264:            "A := [@any : Int, 2]\n(('i -> A[10, 20][i]) 1 : Int)"
crates/lichen-language/tests/language/pipeline.rs:278:            "A := [@any : Int, 2]\na := A[1, 2]; b := 0; a[b]"
crates/lichen-language/tests/language/pipeline.rs:349:    let source = "Doc := {.tag @any : Int}\nfive := 5 # 8 ? Doc{.tag 7}\nfive # 4";
crates/lichen-language/tests/language/pipeline.rs:409:            "A := [@any : Int, 2]\na := A[1, 2]; b := a[0]; b"
crates/lichen-language/tests/language/pipeline.rs:417:    let d = diags("A := [@any : Int, 2]\na := A[1, 2]; a[5]");
crates/lichen-language/tests/language/pipeline.rs:456:            "A := [@any : Int, 2]\nf := ('a = A) -> {i := 0; a[i]}; (f (A[7, 8]) : Int)"
crates/lichen-language/tests/language/pipeline.rs:717:            "s := ({.a @any : Int, .b @any : string}){.a 1, .b \"a\"}; s.a == 1"
crates/lichen-language/tests/language/pipeline.rs:722:    let (module, value, _) = common::evaluate("A := [@any : Int, 2]\nA[1, 2] : Array");
crates/lichen-language/tests/language/pipeline.rs:964:            "A := [@any : Int, 2]\na := b; b := A[1, 2]; a[0]"
crates/lichen-language/tests/language/pipeline.rs:986:    assert_self_reference_is_refused("A := [@any, 1]\na := A[a]; a");
crates/lichen-language/tests/language/pipeline.rs:1097:    run("{.f @any : Int, .g @any : Int}");
crates/lichen-language/tests/language/pipeline.rs:1106:    let (module, root) = run("A := [@any, 2]\ns := {.f @any : Int}; A[s, s]");
crates/lichen-language/tests/language/pipeline.rs:1121:    let report = compile("S1 := {.f @any : Int}; S2 := {.f @any : Int}; A := [S1, 2]; A[S1, S2]");
crates/lichen-language/tests/language/pipeline.rs:1136:    let report = compile("A := [@any, 2]\nf := 't -> {.f t}; A[f (Int), f (Int)]");
crates/lichen-language/tests/language/pipeline.rs:1151:    let report = compile("A := [@any, 2]\nBox := 't -> {.f t}; A[Box (Int), Box (Int)]");
crates/lichen-language/tests/language/pipeline.rs:1157:    let report = compile("A := [@any, 2]\nBox := 't -> {.f t}; A[Box (Int), Box (Type)]");
crates/lichen-language/tests/language/pipeline.rs:1172:    let (module, root) = run("A := 'I -> {.n @any : Int, .I I}\n\
crates/lichen-language/tests/language/pipeline.rs:1173:         In := {.x @any, .y @any}\n\
crates/lichen-language/tests/language/pipeline.rs:1191:        "A := 'I -> {.n @any : Int, .I @any : I}\n\
crates/lichen-language/tests/language/pipeline.rs:1207:        "A := 'I -> {.n @any : I}\n\
crates/lichen-language/tests/language/pipeline.rs:1224:    let v = evaluate("A := {.x @any : Int, .y @any : Type}; a := A{.x 1, .y Int}; a.x");
crates/lichen-language/tests/language/pipeline.rs:1226:    let v = evaluate("A := {.x @any : Int, .y @any : Type}; a := A{.x 1, .y Int}; a.y");
crates/lichen-language/tests/language/pipeline.rs:1241:        "A := {.n @any : Int, .alpha @any : Float}\n\
crates/lichen-language/tests/language/pipeline.rs:1247:        "f := ('x = (@any : Int, @any : Float)) -> x(1) + x(1)\n\
crates/lichen-language/tests/language/pipeline.rs:1257:    let d = diags("A := {.x @any : Int}; a := A{.x 1}; a.z");
crates/lichen-language/tests/language/pipeline.rs:1269:    let d = diags("A := {.x @any : Int, .sub @any : Int}; a := A{.x 1, .sub 2}; a.sux");
crates/lichen-language/tests/language/pipeline.rs:1339:    let report = compile("S := {.a @any : Int}\nf := 'g -> g 1\nf (S{.a 1})");
crates/lichen-language/tests/language/pipeline.rs:1381:    let (module, root) = run("({.f @any : Int, .g @any : Int}){.f 1, .g 2}");
crates/lichen-language/tests/language/pipeline.rs:1386:    let (module, root) = run("s := {.f @any : Int, .g @any : Int}; s{.f 1, .g 2}");
crates/lichen-language/tests/language/pipeline.rs:1407:    let d = diags("({.f @any : Int}){.f 1, .g 2}");
crates/lichen-language/tests/language/pipeline.rs:1413:    let d = diags("S := {.f @any : Type, .g @any : Type}; S{.f 1, .g Int}");
crates/lichen-language/tests/language/pipeline.rs:1424:        "s1 := {.f @any : Int, .g @any : Int}; s2 := {.f @any : Int, .g @any : Int}; A := [s1, 2]; A[(('_x -> s1) _){.f 1, .g 2}, (('_x -> s2) _){.f 1, .g 2}]",
crates/lichen-language/tests/language/pipeline.rs:1446:        run("s := {.f @any : Int, .t @any : Type}; a := s{.f 1, .t Int}; (a.f, a.t)");
crates/lichen-language/tests/language/pipeline.rs:1462:        "s := {.f @any : Int, .t @any : Type}; f := ('a = s) -> a.f; f (s{.f 1, .t Int}) : Int",
crates/lichen-language/tests/language/pipeline.rs:1475:    let d = diags("s := {.f @any : Int, .t @any : Type}; a := s{.f 1, .t Int}; a(0)");
crates/lichen-language/tests/language/pipeline.rs:1495:        run("S := {.x @any : Int, .y @any : Type}; a := S{.y Int, .x 1}; (a.x, a.y)");
crates/lichen-language/tests/language/pipeline.rs:1515:    let value = evaluate("S := {.x @any : Int, .y @any : Type}; S{.x 1, .y Int}");
crates/lichen-language/tests/language/pipeline.rs:1525:        run("S := {.x @any : Int, .y @any : Type}; a := S{.y Int, .x 1}; (a.x, a.y)");
crates/lichen-language/tests/language/pipeline.rs:1537:    let d = diags("S := {.x @any : Int, .y @any : Type}; S{.z 1, .x 1, .y Int}");
crates/lichen-language/tests/language/pipeline.rs:1550:    let d = diags("S := {.x @any : Int, .y @any : Type}; S{.x 1, .x 2, .y Int}");
crates/lichen-language/tests/language/pipeline.rs:1558:    let d = diags("S := {.x @any : Int, .y @any : Type}; S{.x 1}");
crates/lichen-language/tests/language/pipeline.rs:1576:    let d = diags("S := {.x @any : Int, .y @any : Type}; S{.x Type, .y Int}");
crates/lichen-language/tests/language/pipeline.rs:1588:        "S := {.x @any : Int, .y @any : Type}; a := S{.y Int, .x 1}; apply := ('s = S) -> s.y; apply (a) : Type",
crates/lichen-language/tests/language/pipeline.rs:1627:        "mk := 'u -> {.f @any : Int, .g @any : Int}\n(('_x -> (mk (Int))) _){.f 1, .g 2}",
crates/lichen-language/tests/language/pipeline.rs:1631:        "mk := 'u -> {.f @any : Int, .g @any : Int}\nt := mk (Int)\n(('_x -> t) _){.f 1, .g 2}",
crates/lichen-language/tests/language/pipeline.rs:1657:    let d = diags("(@any : Int, @any : Int){.f 1}");
crates/lichen-language/tests/language/pipeline.rs:1753:        common::evaluate("a := c{.f 1, .g 2}\nb := {.f @any : Int, .g @any : Int}\nc := b\na");
crates/lichen-language/tests/language/pipeline.rs:1875:        "A := {.f @any : Int, .g B}
crates/lichen-language/tests/language/pipeline.rs:1876:         B := {.f @any : Type, .g A}
crates/lichen-language/tests/language/pipeline.rs:1906:        evaluated_array_element_ids("((('n -> {A := [@any : Int, n]; A[1, 2, 3]}) 3) : Array)")
crates/lichen-language/tests/language/pipeline.rs:1915:    let d = diags("(('n -> {A := [@any : Int, n]; A[1, 2, 3]}) 5)");
crates/lichen-language/tests/language/pipeline.rs:2001:    let d = diags("T := (@any : Int, @any : Int)\ng := ('x = T) -> x(0)\ng (5)");
crates/lichen-language/tests/language/pipeline.rs:2072:    let d = diags("A := [@any, 2]\nA[1, 'x -> x]");
crates/lichen-language/tests/language/pipeline.rs:2082:    let d = diags("A := [@any : Int, 3]\n(A[1, 2] : Array)");
crates/lichen-language/tests/language/pipeline.rs:2094:    let d = diags("A := [@any : Int, 3]\n(A[1, 2, 3])[5]");
crates/lichen-language/tests/language/pipeline.rs:2178:    let (module, value, _) = common::evaluate("A := [@any : Int, 3]\nA[1, 2, 3] : Array");
crates/lichen-language/tests/language/pipeline.rs:2245:        "A := [@any, 2]\nf := 'x -> A[x, @shallow f (x + 1)]\n\
crates/lichen-language/tests/language/pipeline.rs:2268:        common::evaluate("A := [@any, 2]\nl := A[1, @shallow(2) (2, 3)]\nl[1](0)");
crates/lichen-language/tests/language/pipeline.rs:2300:        "A := [@any, 2]\nf := 'x -> A[x, @shallow(1) f (x + 1)]\n\
crates/lichen-language/tests/language/pipeline.rs:2346:Param := {.type @any, .length @any}
crates/lichen-language/tests/language/registry.rs:59:    write(&dir, "s.lichen", "{.f @any : Int}\n");
crates/lichen-language/tests/language/registry.rs:90:    write(&dir, "inner.lichen", "{.f @any : Int}\n");
crates/lichen-language/tests/language/shallow_zero_depth.rs:33:    let unmarked = compile("A := [@any, 2]\nA[1, 2]");
crates/lichen-language/tests/language/shallow_zero_depth.rs:39:    assert_refused_once("A := [@any, 2]\nA[@shallow(0) 1, 2]", (2, 3));
crates/lichen-language/tests/language/shallow_zero_depth.rs:45:    let report = compile("A := [@any, 2]\nA[@shallow(1) 1, 2]");
crates/lichen-language/tests/language/shallow_zero_depth.rs:52:    let report = compile("A := [@any, 2]\nA[@shallow 1, 2]");
crates/lichen-language/tests/language/shallow_zero_depth.rs:63:    let d = refusals("A := [@any, 2]\nA[@shallow(0) 1, @shallow(0) 2]");
crates/lichen-language/tests/language/std_native.rs:191:A := [@any : Int, 3]
crates/lichen-language/tests/language/std_native.rs:209:A4 := [@any, 4]
crates/lichen-language/tests/language/std_native.rs:210:A2 := [@any, 2]
crates/lichen-language/tests/language/std_native.rs:251:A := [@any, 2]
crates/lichen-language/tests/language/struct_element_attributes.rs:27:    let (message, kind) = refused("S := {.a @any : Int # 4}\nS{.a 1 # 2}");
crates/lichen-language/tests/language/struct_element_attributes.rs:45:        output("S := {.a @any : Int # 4}\nS{.a 1 # 4}"),
crates/lichen-language/tests/language/struct_element_attributes.rs:75:            "A := {.f @any : Int, .g B}
crates/lichen-language/tests/language/struct_element_attributes.rs:76:             B := {.f @any : Type, .g A}
crates/lichen-language/tests/language/template_instances.rs:32:        output("D := {.a @any, .b @any}\n(D{.a 1, .b 2}, D{.a 1.5, .b 2.5})"),
crates/lichen-language/tests/language/template_instances.rs:63:            "Div := 'T -> {.dividen @any: T, .dividor @any: T}\n\
crates/lichen-language/tests/language/template_instances.rs:73:        output("D := {.a @any : Int}\n(D{.a 1}, D{.a 2})"),
crates/lichen-language/tests/language/template_instances.rs:76:    let (message, kind) = refused("D := {.a @any : Int}\nD{.a 1.5}");
crates/lichen-language-lex/src/lib.rs:81:    /// `@any` — the wildcard literal: it unifies with everything and constrains
crates/lichen-language-lex/src/lib.rs:239:            TokenKind::Any => "'@any'",
crates/lichen-language-lex/src/lib.rs:816:                         `@assert`, `@in`, `@shallow`, `@any`, `@gc` and `@cache` are the \
crates/lichen-language-parser/src/ast.rs:121:    /// `@any` — the wildcard literal: it unifies with everything and constrains
crates/lichen-language-parser/src/parse.rs:1093:            // `@any` is a literal, not a keyword: it is a value, so it parses
crates/lichen-language-parser/src/tests/parse_tests.rs:384:    // so a wildcard is written `@any : Int`.
crates/lichen-language-server/src/analysis.rs:1498:        // `@any` is the wildcard literal — a reserved form like `_`.
crates/lichen-lowlevel/src/equality.rs:1245:            // An `Any` on the right never decides an undecided left: `_ = @any` is `_`.
crates/lichen-lowlevel/src/evaluation.rs:977:    /// not read: the element template of an array instance is `@any` more often than
crates/lichen-render/src/render/type_printer.rs:130:                LowValue::Any => "@any".to_string(),
crates/lichen-render/src/render/type_printer.rs:164:                LowValue::Any => Some("@any".to_string()),
crates/lichen-render/src/render/type_printer.rs:454:    /// hole, not `@any`. A hole is no pin either, so the position is suppressed
crates/lichen-render/src/render/type_printer.rs:510:            Some(LowValue::Any) => "@any".to_string(),
crates/lichen-render/src/render/value_printer.rs:275:            return "@any".to_string();
crates/lichen-render/src/render/value_printer.rs:636:            Some(LowValue::Any) => Some("@any".to_string()),
crates/lichen-serve/examples/contract_direct.rs:22:Meta := {.n @any : Int, .k @any : Int, .c1 @any : Int, .ar @any : Int, .ac @any : Int, .br @any : Int, .bc @any : Int}
crates/lichen-serve/tests/serve/schema_probe.rs:31:In   := {.x (compute.Buf (compute.BufParam{.type Float, .length @any})), .w (compute.Buf (compute.BufParam{.type Float, .length @any}))}
crates/lichen-serve/tests/serve/schema_probe.rs:32:Rows := {.y (compute.Buf (compute.BufParam{.type Float, .length @any}))}
crates/lichen-serve/tests/serve/schema_probe.rs:33:Par  := {.n @any : Int, .dim @any : Int, .in (In)}
crates/lichen-serve/tests/serve/schema_probe.rs:34:Arg  := {.n @any : Int, .dim @any : Int, .I (In)}
lichen-std/README.md:22:`std.type_of [@any, 2][1, 2]` is `array{Int, 2}`, and in a type position it *is*
lichen-std-native/src/lib.rs:155:    /// Evaluate even with open `@any` type cells; elements decide laziness.
tree-sitter-lichen/grammar.js:144:    // with no value (`: T # a`) annotates the wildcard `@any`, which is how a
tree-sitter-lichen/grammar.js:290:    // `@any` — the wildcard literal, which unifies with everything.  The `@`
tree-sitter-lichen/grammar.js:293:    wildcard: $ => token(prec(1, '@any')),
tree-sitter-lichen/queries/highlights.scm:53:; `@any` is the `wildcard` node: the query syntax reads a `"@…"` string as a
tree-sitter-lichen/README.md:15:  `t <== k`, `@in`/`@any`/`@shallow`/`@loop`, and the prefix keyword operators.
tree-sitter-lichen/tests/samples.rs:91:        "x := @any\nx",
lichen-language-zed/languages/lichen/highlights.scm:51:; `@any` is the `wildcard` node: the query syntax reads a `"@…"` string as a
examples/array.lichen:5:Int3 := [@any : Int, 3]
examples/attention.lichen:138:ParS := {.n @any : Int, .keys @any : Int, .dim @any : Int, .in (QK)}
examples/attention.lichen:150:ParA := {.n @any : Int, .dim @any : Int, .keys @any : Int, .in (QSV)}
examples/attention.lichen:167:Fill := {.n @any : Int}
examples/closure.lichen:5:Any4 := [@any, 4]
examples/convolution2d.lichen:53:CPar := {.n @any : Int, .cols @any : Int, .taps @any : Int, .in (CIn)}
examples/convolution2d.lichen:77:Fill := {.n @any : Int}
examples/decode.lichen:190:ParK  := {.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .in (KVIn)}
examples/decode.lichen:209:ParS := {.n @any : Int, .kdim @any : Int, .in (Gin)}
examples/decode.lichen:219:ParM := {.n @any : Int, .in (Row)}
examples/decode.lichen:229:ParY := {.n @any : Int, .in (Soft)}
examples/decode.lichen:242:ParW := {.n @any : Int, .dim @any : Int, .keys @any : Int, .in (AV)}
examples/decode.lichen:251:Fill := {.n @any : Int}
examples/decode_loop.lichen:170:ParK  := {.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .in (KVIn)}
examples/decode_loop.lichen:189:ParS := {.n @any : Int, .kdim @any : Int, .in (Gin)}
examples/decode_loop.lichen:199:ParM := {.n @any : Int, .in (Row)}
examples/decode_loop.lichen:209:ParY := {.n @any : Int, .in (Soft)}
examples/decode_loop.lichen:222:ParW := {.n @any : Int, .dim @any : Int, .keys @any : Int, .in (AV)}
examples/decode_loop.lichen:233:ParT  := {.n @any : Int, .j @any : Int, .dim @any : Int, .in (KTIn)}
examples/decode_loop.lichen:245:Fill := {.n @any : Int}
examples/decode_loop.lichen:309:DriveState := (@any : Int, @any : Struct, @any : Struct, @any : Struct)
examples/doc.lichen:5:Doc := {.tag @any : Int}
examples/filter.lichen:42:FPar := {.n @any : Int, .taps @any : Int, .in (FIn)}
examples/filter.lichen:50:Fill := {.n @any : Int}
examples/gc.lichen:5:Any2 := [@any, 2]
examples/gcd.lichen:10:Pair := (@any : Int, @any : Int)
examples/gemv.lichen:111:Par  := {.n @any : Int, .kdim @any : Int, .in (Gin)}
examples/gemv.lichen:119:Fill := {.n @any : Int}
examples/generate.lichen:191:ParK  := {.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .in (KVIn)}
examples/generate.lichen:210:ParS := {.n @any : Int, .kdim @any : Int, .in (Gin)}
examples/generate.lichen:211:ArgS := {.n @any : Int, .kdim @any : Int, .I (Gin)}
examples/generate.lichen:221:Par  := {.n @any : Int, .w @any : Int, .in (Row)}
examples/generate.lichen:222:Arg  := {.n @any : Int, .w @any : Int, .I (Row)}
examples/generate.lichen:231:Fill := {.n @any : Int}
examples/generate.lichen:278:GenerateState := (@any : Int, @any : Struct, @any : Struct)
examples/index.lichen:5:Any2 := [@any, 2]
examples/kvcache.lichen:118:Par := {.n @any : Int, .cap @any : Int, .dim @any : Int, .t @any : Int, .in (KVIn)}
examples/kvcache.lichen:135:Fill := {.n @any : Int}
examples/layernorm.lichen:61:ParS := {.n @any : Int, .in (Row)}
examples/layernorm.lichen:70:ParQ := {.n @any : Int, .in (Row)}
examples/layernorm.lichen:80:ParY := {.n @any : Int, .eps @any : Float, .in (Moments)}
examples/layernorm.lichen:93:Fill := {.n @any : Int}
examples/lazy_infinite.lichen:5:Any2 := [@any, 2]
examples/mathlib.lichen:100:SPar := {.n @any : Int, .k @any : Float, .in (SIn)}
examples/mathlib.lichen:110:LPar := {.n @any : Int, .t @any : Float, .in (LIn)}
examples/mathlib.lichen:121:CPar := {.n @any : Int, .lo @any : Float, .hi @any : Float, .in (CIn)}
examples/mathlib.lichen:131:RPar := {.n @any : Int, .kdim @any : Int, .in (RIn)}
examples/mathlib.lichen:141:MPar := {.n @any : Int, .kdim @any : Int, .least @any : Float, .in (MIn)}
examples/mathlib.lichen:151:DPar := {.n @any : Int, .kdim @any : Int, .in (DIn)}
examples/mathlib.lichen:159:Fill := {.n @any : Int}
examples/matmul.lichen:64:Par := {.n @any : Int, .cols @any : Int, .k @any : Int, .in (Ain)}
examples/matmul.lichen:74:Fill := {.n @any : Int}
examples/nested_function.lichen:5:Any2 := [@any, 2]
examples/placeholder.lichen:7:    x = (@any : Type, @any : _)
examples/pooling.lichen:38:PPar := {.n @any : Int, .cols @any : Int, .stride @any : Int, .taps @any : Int, .least @any : Float, .in (PIn)}
examples/pooling.lichen:48:Fill := {.n @any : Int}
examples/rmsnorm.lichen:15:ParS := {.n @any : Int, .in (Row)}
examples/rmsnorm.lichen:25:ParY := {.n @any : Int, .in (Scale)}
examples/rmsnorm.lichen:35:Fill := {.n @any : Int}
examples/rope.lichen:29:ParC := {.n @any : Int, .in (Turn)}
examples/rope.lichen:38:ParS := {.n @any : Int, .in (Turn)}
examples/rope.lichen:48:ParR := {.n @any : Int, .in (Expr)}
examples/rope.lichen:63:Fill := {.n @any : Int}
examples/sample.lichen:139:Par := {.n @any : Int, .w @any : Int, .in (Row)}
examples/sample.lichen:140:Arg := {.n @any : Int, .w @any : Int, .I (Row)}
examples/sample.lichen:149:Fill := {.n @any : Int}
examples/silu.lichen:17:ParA := {.n @any : Int, .in (Gate)}
examples/silu.lichen:27:ParB := {.n @any : Int, .in (UpIn)}
examples/silu.lichen:36:Fill := {.n @any : Int}
examples/softmax.lichen:20:ParM := {.n @any : Int, .in (Row)}
examples/softmax.lichen:30:ParY := {.n @any : Int, .in (Soft)}
examples/softmax.lichen:41:Fill := {.n @any : Int}
examples/stencil.lichen:83:XPar := {.n @any : Int, .alpha @any : Float, .in (XIn)}
examples/stencil.lichen:84:XArg := {.n @any : Int, .alpha @any : Float, .I (XIn)}
examples/stencil.lichen:107:Fill := {.n @any : Int}
examples/struct.lichen:3:  output = "((@any, @any), (1, Int), 1, Int, (@any,), (1,), 1): Tuple"
examples/struct.lichen:5:A := {.x @any : Int, .y @any : Type}
examples/struct.lichen:7:B := {.x @any : Int}
examples/struct_generic.lichen:11:Box := 't -> {.f @any : t}
examples/struct_recursion.lichen:3:  output = "((@any, (@any, …)), (@any, (@any, …)), (1, (Int, …)), (Int, (1, …))): Tuple"
examples/struct_recursion.lichen:5:A := {.f @any : Int, .g (B)}
examples/struct_recursion.lichen:6:B := {.f @any : Type, .g (A)}
examples/table.lichen:5:Any2 := [@any, 2]
examples/topk.lichen:124:Par0 := {.n @any : Int, .w @any : Int, .in (Row)}
examples/topk.lichen:125:Arg0 := {.n @any : Int, .w @any : Int, .I (Row)}
examples/topk.lichen:136:Par1 := {.n @any : Int, .w @any : Int, .rows @any : Int, .ranks @any : Int, .in (Sel1)}
examples/topk.lichen:137:Arg1 := {.n @any : Int, .w @any : Int, .rows @any : Int, .ranks @any : Int, .I (Sel1)}
examples/topk.lichen:155:Par2 := {.n @any : Int, .w @any : Int, .rows @any : Int, .ranks @any : Int, .in (Sel2)}
examples/topk.lichen:156:Arg2 := {.n @any : Int, .w @any : Int, .rows @any : Int, .ranks @any : Int, .I (Sel2)}
examples/topk.lichen:174:Fill := {.n @any : Int}
examples/torus.lichen:63:GPar := {.n @any : Int, .cols @any : Int, .alpha @any : Float, .in (GIn)}
examples/torus.lichen:64:GArg := {.n @any : Int, .cols @any : Int, .alpha @any : Float, .I (GIn)}
examples/torus.lichen:92:Fill := {.n @any : Int}
examples/tuple.lichen:5:(1, Int) = (@any : Int, @any : Type)
examples/type_of.lichen:6:Any2 := [@any, 2]
examples/worldgen.lichen:33:InSeed := {.seed @any : Int}
examples/worldgen.lichen:41:PNoise := {.n @any : Int, .cols @any : Int, .scale @any : Int, .seed @any : Int, .in (InSeed)}
examples/worldgen.lichen:42:ANoise := {.n @any : Int, .cols @any : Int, .scale @any : Int, .seed @any : Int, .I (InSeed)}
examples/worldgen.lichen:65:PMix := {.n @any : Int, .cols @any : Int, .w @any : Float, .in (InField2)}
examples/worldgen.lichen:66:AMix := {.n @any : Int, .cols @any : Int, .w @any : Float, .I (InField2)}
examples/worldgen.lichen:76:PSmooth := {.n @any : Int, .cols @any : Int, .in (InField)}
examples/worldgen.lichen:77:ASmooth := {.n @any : Int, .cols @any : Int, .I (InField)}
examples/worldgen.lichen:94:PBiome := {.n @any : Int, .cols @any : Int, .sea @any : Float, .ridge @any : Float, .in (InLand)}
examples/worldgen.lichen:95:ABiome := {.n @any : Int, .cols @any : Int, .sea @any : Float, .ridge @any : Float, .I (InLand)}
examples/worldgen.lichen:106:PTile := {.n @any : Int, .cols @any : Int, .seed @any : Int, .in (InTile)}
examples/worldgen.lichen:107:ATile := {.n @any : Int, .cols @any : Int, .seed @any : Int, .I (InTile)}
```

### `@any` usage shapes and their new-SPEC spelling

Every `.lichen` `@any` in the dump is one of five shapes. Target spelling per the SPEC's new Ideal API (forms 1-5, Nested form 6, sugar `a <: b` => `a = b _`) and the measured caveats in `docs/notes/ideal-api-review.md`:

1. **Scalar wildcard field with type — `{.n @any : Int}`, `{.w @any : Float}`, `{.v @any : Int # 32}`.** The bulk: every `examples/*Par.lichen`, `examples/attention.lichen:138,150,167`, `decode(.lichen|_loop.lichen)`, `generate.lichen`, `worldgen.lichen:33-107`, `mathlib.lichen`, `topk.lichen`, `stencil.lichen`, `torus.lichen`, `kvcache.lichen:118`, `sample.lichen:139-140`, `matmul.lichen:64`, `pooling.lichen:38`, `rope.lichen`, `rmsnorm.lichen`, `layernorm.lichen`, `silu.lichen`, `softmax.lichen`, `filter.lichen:42`, `convolution2d.lichen:53,77`, `gemv.lichen:111`, `doc.lichen:5`, `struct.lichen:5,7`, `tensor.lichen:4,9,12,13,19`, `compute.lichen:6,7,23-25,32`, `fixtures/gauge.lichen:1,3`, `buf_element_class_direct.lichen:6`, `gcd.lichen:10` (tuples), `array.lichen:5` (`[@any : Int, 3]`), `decode_loop.lichen:309` / `generate.lichen:278` (state tuples), `placeholder.lichen:7`. → **plain value hole**: `{.n _ : Int}`. This is the form-3 field split minus `@any` (SPEC `div := {.'dividend: Int, ...}`); the hole is undecided per SPEC §Undecided, not a wildcard value. Watch: `@any` is *decided* for the concreteness verdict (evaluation.rs:884) and counts as *open* for closure (template.rs:297); a `_` hole is open on both axes, so a position that relied on "decided-wildcard" (e.g. `loop_run.rs:222` attribute fill, checker.rs:1031 shared default) cannot become `_` — those are the internal-mechanism sites, they retire with the variant, not by spelling rewrite.
2. **Bare wildcard field — `{.native @any}`, `{.I @any, .O @any}` (compute.lichen:4-5,11), `Any2 := [@any, 2]` (table.lichen:5, index.lichen:5, gc.lichen:5, lazy_infinite.lichen:5, nested_function.lichen:5, type_of.lichen:6, closure.lichen:5), `(@any, @any)` printed in struct.lichen/struct_recursion.lichen outputs.** → plain hole `_` in the value slot (`{.native _}`, `[_ , 2]`); the kernel template `K := {.native _, .I _, .O _}` stays a direct template per value-templates.md — which then needs its "direct template" ruling re-stated without `@any` (§6 open questions).
3. **Type-in-value field — `{.x (compute.Buf …)}`, `{.in (CIn)}` (convolution2d.lichen:51-53, attention.lichen, decode.lichen, compute.lichen:7 `.in T.I`, filter/stencil/topk/worldgen `… .in (XIn)`).** No `@any` textually, but these are the fields whose `.native`/open cells the CIn-collide measured. → SPEC **Nested form 6** (`B := _ -> {.a A _, .'z: String}`): sanctioned as a conformant usage; the collide is the implementation gap (one-answer-per-class.md), and `ideal-api-review.md` §"The Nested form (6)" already records this verdict.
4. **Annotated wildcard in the prelude — core.lichen:3-10 `(@any ! in_num, @any ! in_num)`** (the operand Pair of every routed `+ - * / < > <= >=`). → attribute-annotated hole `(_ ! in_num, _ ! in_num)`; the `!` perspective pin stays on the hole. Per apply-clone-ownership §8 the Pair now lives inside each operator's body; the migration must keep the operators polymorphic without the wildcard — this is the highest-risk lowering site because every shipped program routes through it.
5. **Value-less annotation head — parser-generated, not written: `: T # a` folds to `@any : T # a` (parse.rs:793-801), and the primed sugar swaps the head for a formal (parse.rs:1174-1202).** With `@any` gone the head must become `_` (an undecided expr) — the sugar survives unchanged if the fold and the swap pattern-match on the hole instead of `Expr::Any`.

`Box := 't -> {.f @any : t}` (struct_generic.lichen:11) → `'t -> {.f _ : t}` — form 5 (function generic) with a hole field. `Div := _ -> {…}` as the SPEC now writes it needs `Div _` at each use — measured refusal today is `the callee of an instantiation must be a struct type, found TypeFunction` (see §3b); the example migrations are blocked on that implementation gap, not on the spelling.

`lichen-std/README.md:22` (`std.type_of [@any, 2][1, 2]`) and `tree-sitter-lichen/README.md:15` are prose mentions → update with the grammar. `helix/` has **zero** Any references (grep over the whole directory returns nothing).

## 3. Mechanism questions

### 3a. Where the old "auto unify `@any` to parameter in the end" lives today

There is **no single desugaring site** on dev that appends `@any = x` to a function. The old sugar is realized as five cooperating mechanisms:

1. **Parser** — `crates/lichen-language-parser/src/parse.rs:793-801`: a chain of annotations with no value head is folded over a synthesized `Expr::Any` head: `.map_with(|rest, me| fold_annotations(Expr::Any(span_at(...)), rest))` with the comment "A chain with no value: `: T # a` annotates the wildcard, which is how a compound slot leaves its value position open." And `parse.rs:1174-1202` `primed_field_value` `# Invariant`: "the position it leaves open — the wildcard head of a value-less chain — is exactly where the formal goes", i.e. the `.'name` field sugar **matches the `Expr::Any` head and replaces it with `Expr::Formal`** (`}) if matches!(&*value, Expr::Any(_)) => ...`). This is the sugar's parser half; with Any removed it must match the hole `Expr::Placeholder` instead (or the formal is simply the written value).
2. **Checker construction defaults** — the wildcard fills every position the source did not state: `checker.rs:1031` (`self.any_value = self.alloc_node(root, None, Some(Any))` — "Default positions share this wildcard; source literals and function sugar are fresh"), `checker.rs:1136-1143` (`any()`), `checker.rs:1520-1529` (`slot_expr` `# Invariant` — "every other slot is the wildcard an unstated attribute is"), `tuples.rs:105,111,137`, `checker.rs:1751` ("A value literal decides its own attributes: each is its extension's `[value, type]` expr, else the wildcard"), `checker.rs:1850` ("`_` is undecided in every attribute: each attribute slot is a hole"), and `program.rs:433-455` `AnyLit::build` (**fresh per occurrence**, its own type cell — the invariant that makes `@any : T` bind locally).
3. **The identity-lambda rule** — `checker/lambda.rs:126-142`: for an unasserted lambda whose body *is* its parameter, the parameter's class value is written `LowValue::Any` ("The identity is the **domain**: a body that returns the parameter hands the call the argument"). This is the closest surviving form of "unannotated part become `Any`": `f := 'x -> x` ends with `x`'s class holding `Any`, which the clone then turns per call into a fresh cell the argument fills. Under a hole-based semantics the write becomes "leave the class value empty", which the clone rule already treats the same way (function.rs:866 mints a fresh **empty** node for an `Any`; an undecided node detaches per §Clone) — but `class_answer_is_decided` (function.rs:605-612) and `answer_elements_are_undecided` (apply.rs:464-477) deliberately read `Any`, not emptiness, and those two are where the migration must prove equivalence.
4. **Function normalization** — `docs/notes/function-normalization.md` §1/§2.1: the birth normalize is the lambda-value deep-evaluate loop in `checker.rs` `build_with`; it normalizes what the **domain/codomain cells** hold. An `Any` written by mechanism 3 survives normalize as "decided content" (evaluation.rs:884 verdict), which is precisely why cloning reads it; a hole must be made to read the same way or the clone detaches differently. No Any-specific branch was found inside normalize itself.
5. **Clone conversion Any→hole** — dynamic `function.rs:866-872`, frozen `static_module/apply.rs:474-478` and `565-570`, template walk `template.rs:297,402,539,746`. After the migration these arms are the **hole/detach rule only** (§Clone: "Clone an undecided node will detached from its original class"); the wildcard special case disappears with the variant.

### 3b. Applying a decided non-function today; where `TypeFunction` blocks instantiation

- **Runtime**: `crates/lichen-lowlevel/src/evaluation.rs:336-400`, the `LowOperator::Apply` arm. The operand array is evaluated; the callee (operand 0) is evaluated. Undecided callee → `apply_callee_type` (signature apply, `:727-750`) and `ran_undecided` (lazy, no panic). Decided non-function callee (`USize/Float/Str/Table/None`) → `self.eval_errors.push(EvalError::ApplyTarget { function: operands[0].node })` and the node's answer is `LowValue::Error` (`:371-386`). A decided **Array** callee falls to `OperatorExt::is_callable` (compute decides for its struct values); `Error` propagates. So `Int _` is refused and recorded **once**, it does not panic.
- **Check time**: `checker/lambda.rs:427` `check_app` resolves the callee function (`function_apply_function`/`static_function_of_callee`/curried arms); `:540-545`: `let callable = callee.is_some() || self.is_function_type(function_ty); if !callable && self.type_is_concrete(function_ty) { self.check_unify(function_ty, fn_ty, ..., DiagKind::Guard); }` — a concrete non-function callee is refused by unifying against the shared `FunctionType` template (the `TypeFunction` marker: `highlevel/src/shape.rs:64`), which fails with a Guard diagnostic.
- **Instantiation refusal**: `checker/structs.rs:820` `check_instantiate` evaluates the template's value and type (`:850-856`); not concrete → `DiagKind::InstantiateCalleeUndetermined` (`:853-859`, function-normalization ruling 7 territory); concrete but `!is_struct_layout(template_ty)` → `DiagKind::InstantiateCallee` (`:867-876`), rendered at `lichen-language/src/render.rs:338-341` as "the callee of an instantiation must be a struct type, found {}". Printing the callee's type name (`TypeFunction`) is `type_printer.rs:183`. So the measured new-SPEC form `Div := _ -> {.dividend _: Int, ...}; div := 'x = Div _ -> ...` fails today when the instance `Div _{...}` reaches `check_instantiate` with the **unapplied** function type: the implementation applies `Div _` lazily and the instantiation gate only accepts struct layouts. Pinned by `tests/language/callee_inference.rs:73` and `pipeline.rs:1647,1662`.
### 3c. The clone-vs-reference decision today, and the two strict-clean commits

**Dev rule** (`node_apply`, `crates/lichen-lowlevel/src/function.rs:855-950`): the wildcard arm fires first (`:866-872` — value `Any` ⇒ `add_node(target, None, None)` fresh empty, origin recorded). Then `proven_concrete` = deep-pass proved AND no operator AND own answer elements decided AND template decided (`:881-888`); then `depends_on_parameter` = `ctx.in_parameter OR node == ctx.parameter OR wildcard OR !proven_concrete OR scoped OR value_holds_foreign_function` (`:896-903`) — a proven node fails this and is referenced (`:904-906`). Otherwise the class gate decides: `class_inside_scope` (`:909`; `scope.rs:199-209`), with `(!class_inside OR canonical) AND !replay_operation AND !wildcard => reference as-is` (`:918-921`). So **today an undecided (empty) non-`Any` node whose class reaches outside the applying scope is REFERENCED** — exactly the reading `one-answer-per-class.md` measured into the CIn collide; the SPEC Clone sentence that an undecided node's clone detaches is not yet the dev gate for empties, only for `Any` (which bypasses the gate through `wildcard`). The frozen mirror: `static_module/apply.rs:474-478, 561-570` (wildcard fresh before the `FrozenClassRule` question), the template walk `template.rs:297,402,539,746` shares the policy.

This is the crux for the migration: removing `Any` makes every open template cell an **empty**, and today empties in an external class are *referenced* (shared) — which reproduces the collide. Either the clone walk treats "undecided detaches" as the maintainer's Clone sentence says (that is the `5fcc181a` territory), or requirement instantiation (`templates.rs`) keeps copying per use. Both options change `# Invariant` text in `scope.rs` / `function.rs` — maintainer's permission required (AGENTS.md, "Moving an invariant does not change it").
**`git show 5fcc181a`** ("lowlevel: the Clone verdict follows the outermost member's scope", strict-clean 17:44; **not an ancestor of dev**; the notes call the same content `c73ed549`). One file, `scope.rs` +24/-8. `class_inside_scope` stops reading the cached class **summary** (nearest common ancestor of the members' places) and instead walks every member's `owner_scope`: a member with no owner returns false immediately ("The root is the outermost place there is"); otherwise it finds the one place that is an ancestor-or-self of all the others and answers `scope.is_ancestor_or_self(outer)`; incomparable places answer outside. It **rewords the `# Invariant`** to the maintainer's sentence ("The verdict is a fact of the **class**... The outermost member's place decides..."). `git apply --check` on dev: **applies cleanly** (dev still has the pre-image summary version). Measured there: the launch still runs five times; `convolution2d` still hits the CIn collide (the declared `Buf` class has a root member ⇒ outermost place is the root ⇒ still referenced). Cost note: it trades the O(log F) ancestor test on the cached summary (apply-clone-ownership §4) for an O(members) scan; the clone walk's per-pass verdict cache bounds it, but the invariant reword needs the maintainer's word before landing.

**`git show 8552d500`** ("drop the parameter-value special cases from the clone walk", strict-clean 17:36; not an ancestor). `static_module/apply.rs` +6/-25: deletes `parameter_value` from `StaticApplyCtx`, the `is_parameter_value` verdict override (the formal's value cell cloned per call "even though its class is shared with the declaration"), the `outer_instance` bypass, the owner-tag and value-carry branches keyed on it, and the 15353da6-era ctx plumbing. `git apply --check` on dev: **does not apply** (`patch failed ... static_module/apply.rs:32`). Verified reason: dev's `StaticApplyCtx` has **no** `parameter_value` at all (grep: 0 occurrences) — the flag was introduced and carried only on the `spec/instance-v` → `strict-clean` line (notes `287a4835` narrative), so on dev there is nothing to remove. Lineage check with `git merge-base --is-ancestor` against HEAD: `f48883c4, 8ca8493b, 287a4835, 845085a0, bc669628, c73ed549, 15353da6, 0eb2539a, 61c537e0` are **all NOT ancestors of dev** — dev is the pre-strict-fix architecture, and its `check_named_field` (structs.rs:199) with the `is_formal_rooted` arm (structs.rs:209) is dev's own older code, not the strict-clean launch fix. What still matters for this migration: 8552d500 records the maintainer's verdict that **the formal's value needs no special case, the SPEC clone rule by class scope decides it** — once `Any` is gone that verdict applies to the plain undecided-detach path, so the migration should carry its *intent*, not its diff.
### 3d. `loop_run.rs:222 / :256` — what the `Any` cells there are, and who consumes them

Mechanism: the loop-run machinery (`crates/lichen-lowlevel/src/loop_run.rs`) materializes replayed parameters and collected arrays as **full-width slot exprs**. `slot_expr` (`:215-246`, called from `:75` and `:211`) builds `[value, type, wildcard...]`: position 0 the value node, position 1 the entering type (or a fresh unchecked cell, `:203-210`), and **every remaining attribute position gets a node holding `LowValue::Any`** (`:222`). `slot_array` (`:248-269`, called from `:190`) does the same per element slot (`:256`). The `Any` there means "an attribute position no one stated" — the same convention as `checker.rs:1031`'s shared default — so any later **stated** attribute can be written in (the position is a value, not empty, but `clear_wildcard`/directional unify accept a fill).

Clarification of the question's phrasing: `loop_run.rs:128` — "The selector indexes `[else, then]`. **Any other index is the evaluator's own out-of-bounds read**" — is English prose about the condition's `USize(choice)`: choices 0/1 pick `test.on_zero`/`test.on_one`, any other choice yields `Iteration::Unnamed` (`:129-138`). It is **not** a use of `LowValue::Any`; the brief conflated the two. The 222/256 nodes are the attribute fill.

Downstream match arms that consume the wildcard value (the census in §2 lists each line):

- `equality.rs:1164-1177` (left-`Any` joins an undecided right class; other wildcard pairs succeed without writing) and `:1245-1251` (right-`Any` never decides a left) — so a stated attribute unifying against the `Any` fill writes the stated side.
- `equality.rs:397-407 clear_wildcard` — a statement may empty the lone `Any` to take a value; `:483` and `apply.rs:464-477` — openness tests that count `Any` as open.
- `evaluation.rs:171-190, 262-285` — a reader answers `None` (undecided) across `Any`; `:884-889` — the deep verdict takes `Any` as **decided content** (what keeps these slots provable); `:1013-1030` — scope release requires a non-`Any` root value.
- `function.rs:866-872` / `static_module/apply.rs:474-478,565-570` / `template.rs:297,402,539,746` — every clone walk mints a fresh **empty** cell in place of an `Any`, which is what makes the replayed slot per-call.
- `indexing.rs:639-680` — a slot is "wildcard" iff it holds `Any` or is an empty clone whose origin reaches `Any` (the only reader that follows `origin`); this test drives requirement-vs-pin behaviour for deferred element templates.
- `annotations.rs:240-263, 548-560, 613-620` — stated-vs-carried split; `checker.rs:1440-1451, 1965-1985` — attribute reads skip `Any`.
- `table.rs:251-255` — key unfolding: `Any` is `Undecided` (Key) / `UNDECIDED_TOKEN` (Template) — decision D5's canonical hash; `codec.rs:255,301` — artifact tag 9; printers (`type_printer.rs:130,164,471,510`, `value_printer.rs:104,274,636`) — source-level spell-back as `@any`.

If `Any` is deleted outright, the 222/256 fills have three options: leave the positions **empty** (then `clear_wildcard`, `carried_wildcard`, and `wildcard_slot`'s "empty alone is not wildcard" rule must be re-decided — an empty position is exactly what a computed hole looks like, and indexing.rs:674's invariant exists to keep those apart), keep a **fresh empty with a marker origin**, or restructure the replay slot to `[value, type]` + attribute-table. The invariants above say which readers notice; the choice belongs to the maintainer (§6).
### 3e. How `@any` lowers (AnyLit) and how `:` annotation unifies

**AnyLit::build** (`crates/lichen-highlevel/src/program.rs:433-455`): `let value = ctx.value_node(P::Value::from(LowValue::Any)); let ty = ctx.fresh();` — a fresh value cell **and a fresh own type cell** per occurrence, with the `# Invariant`: "The value node is **fresh per occurrence**: the spelling's own type cell is what `@any : T` binds, and a shared cell would make the first annotation bind every later one." Reachability: lexer `TokenKind::Any` (`lichen-language-lex/src/lib.rs:83,239,808` — the `@`-keyword map spells `"any"`), parser atom (`parse.rs:1095 token(TokenKind::Any).map(|t| Expr::Any(t.span))`), lowering (`crates/lichen-language/src/compile.rs:495-496 Expr::Any(span) => alloc(ExprKind::Literal(HighProgramLiteral::from(AnyLit)))`), dispatch (`program.rs:584 + AnyLit as Any; :604 HighProgramLiteral::Any(lit) => lit.build(ctx)`).

**Annotation `:`**: SPEC `## Annotation`: an annotated expr builds a new expr whose annotation attribute is *unified with* (or replaced by) the annotation expression (`{ann1 = ann2; ...}`). Implementation: the checker writes the annotated side's attribute slot from the annotation expr — `annotations.rs:217-263` reads/carry-splits slots on the `Any`/stated boundary (`stated_slot` excludes `Any`, `carried_wildcard` requires it), `adopt_stated_attributes` `# Invariant` (`:264-275`: "`Any` unifies with everything and merges with nothing, so a wildcard left in place says the position never decides..."), `unify_element_attributes`/`unify_element_values` (`:548-620`: element slots hold `@any`, so the lowlevel compare is vacuous and the reconciliation happens per marker here). Ordinary `:` on a *value* is still `check_unify` of the annotated expr's type attribute against the annotation expression's denotation — not a template match (value-templates.md "Ordinary `:` annotation is unchanged").

**Type-declaring-field branch of `check_instantiate`** (`checker/structs.rs`, cited as ~993; measured location on dev `:975-1001`): when the supplied field value is undecided, the branch reads the declared field's **own type cell** — `shape::value_item(field, 1)` — evaluates both sides deep, and `self.check_unify(actual_ty, expected, loc, DiagKind::Annotation)` (`:993-1001`). This is the `ideal-api-review.md` "The call site, measured" site: for `{.x @any : (compute.Buf ...)}` the argument's *type* meets the declared field's *type*, and it is where the `expected @any, found Struct` / template-vs-type-value conflict was recorded (the descent root `34560,1185` → slot 1 → `34,774` → slot 0 → nodes `8` vs `770`). Under the migration this branch is the one that turns `{.x _ : (Buf ...)}` into a hole-with-requirement; the collide repair (per-apply clone of the declared type graph) lands here or in the parameter-image path, not in the instance construction itself.
### 3f. Is unify directional? Is there a `<:` token? Where do the sugars desugar?

**Directional — yes, in two senses.**
1. Value-direction: `equality.rs:564 pub fn unify(&mut self, a, b)` (and `:607 unify_ref`, recursive core `:1146 unify_inner`). The wildcard arm is explicitly directional (`:1164-1177`): "Directional wildcard: only a left `Any` may join an undecided right class" — left-`Any` vs undecided-right merges the classes; every other wildcard pairing answers true **without merging or writing**. The undecided/decided arms repeat the direction at `:1235-1251`: `(None, Some(y))` — "An `Any` on the right never decides an undecided left: `_ = @any` is `_`" — no `add_equality`; `(Some(x), None)` merges and writes. `lib.rs:89-92` states the same as the variant's `# Invariant`. `alias_read` (`:1385-1394`) refuses to join a reader to a wildcard target for the same reason.
2. Argument-direction at the apply: `apply.rs:40-60 apply_parameter_check` puts the **parameter on the left, argument on the right** for the compare, and `function-normalization.md` §2 says the SPEC `a = p_clone` shape (argument left) is what currently strands the formal — the direction is load-bearing on the call path too.

**`<:` — no token, confirmed.** The lexer has no `<:`: `crates/lichen-language-lex/src/tests/lex_tests.rs:254-256` pins "`<:` is not a spelling: it lexes as `<` and `:`", and `lichen-language-parser/src/tests/parse_tests.rs:650-651` pins the parse error. The new SPEC sugar `a <: b => a = b _` therefore has **no lexer support yet**; implementing it means either a new token or a parser-level `<`+`:` combination rewriting to `a = (b _)` — a surface change only, given `=` already unifies.

**Sugar desugar sites:**
- `bin_operation` (`<l> op <r>` => `op (l, r)`): **parser** builds `Expr::BinOp` (`parse.rs:833-841`, the foldl levels at `:660-760`); **compile lowering** turns it into an apply on a tuple when a prelude is in scope — `compile.rs:633-656 routed_operator(...) -> alloc(ExprKind::Apply { function, argument: operands })` with the operands a **tuple** (`alloc_tuple(&[left, right])`), else the plain `BinOp` low operator (`:657+`). Routing table `compile.rs:258-267` (`add/sub/mul/div/less/greater/...`). See `docs/notes/operator-polymorphism.md` §7 and apply-clone-ownership §8 — the `Pair` tuple ABI is why the prelude is on the Any-migration critical path.
- `condition` (`if c then t else e` => `(e, t)(c)`): **parser** `Expr::If` (`parse.rs:1689-1708`); **compile lowering** `compile.rs:684-703` builds the tuple `(else, then)` and a positional `ExprKind::Field { container: branches, key: condition }` — the lazy branch read, exactly the SPEC sugar.
- `.'name` primed field: parser only (`parse.rs:1168-1202`), replacing the value-less-annotation `Expr::Any` head with a formal.

The `=>` sugar arrows in the SPEC's `sytax` blocks are documentation; the machinery lives at the two layers above (parser AST, compile lowering to IR), never in the checker.

## 4. Design-note digest

### value-templates.md (Direct @any templates)

- Status as written: current for the source language, **compute migration incomplete**. The note's whole vocabulary is wildcard-based; the migration must re-state it in hole terms or mark the rulings dead.
- Rulings: a direct template (`S := {.a @any, .b @any}`, `T := [@any : Int, 3]`) is a **value**, never a function, never applied; parameterized constructors stay functions, applied explicitly at each use; `=` is the only template match, `<:` was never a token; ordinary `:` annotation is not a template match; no source `@clone`.
- Per-use instantiation: open value slots become fresh empty cells; types, attributes, name table and nominal id are shared — implemented as `instantiate_requirement` (`checker/templates.rs`).
- "Scalar wildcard fields use `@any : Int` or `@any : I`; bare `I` stores a type value, not an arbitrary value of type `I`" — the sentence Any-removal re-speaks as `_ : Int`.
- Operators keep the **tuple ABI** `operator (left, right)`; thin arrow removed; call-result templates use plain `=` (`r = k.O`).
- The compute model restated: `K := {.native @any, .I @any, .O @any}` matched with plain `=`; a computed struct call returns a value; `(Buf x){...}` is not a second instantiation; `type_of (k.O)` needs the space.
- Measured: the class-wide scope verdict commit `14285cd8` fell the compute suite 112 to 67 (table in the note); the four-line `compute.jit` repro answers 43 at `b71b81dd`, `none` from `14285cd8` on; forcing the dynamic verdict to "cloned" answers 43 again — the verdict is what turns the body's read into a reference.
- The mechanism trace: static prelude apply mints a closure; the dynamic apply binds the argument into the parameter **image**; the body's `Index(parameter_slot, 0)` was **referenced** because its class was judged outside; the external member cells had `owner=None`; the split (argument lands in the clone, body reads the unowned cell) is the regression's anatomy.
- Unlanded experiment: including outer instances of value-carrying members re-unified the classes but still answered `none` — the outer-instance key is NOT the root cause; the direct cutoff is the external launch **operation** cell cutting off the operand walk (see replay-apply.md).
- Dependent lengths: a length depending on an enclosing parameter is deferred through the constructor result's length read; the instance count pins that cell; a later mismatch is a **runtime** error; `[T, _]` does not infer a length.
- Verification boundary: the source model is measured when plain `=` performs no application, instances use the template value directly, and the retired spellings (`tuple{..}`, `array{..}`, `struct{..}`, `@clone`, `<:`) are refused; remaining consumer: compute parallel lowering.
### one-answer-per-class.md

- Maintainer's verdict (contract): a class holds **one answer**; there is no "second answer" concept, no diagnostic sentence naming one, no code path reporting one. A write disagreeing with the class's value is an ordinary unify failure rendered `expected X, found Y`; the only right response is to remove the code that produced the second value. The `value_a`/`value_b` special case and the second-answer note are deleted.
- The measured defect (table of six program shapes): two kernels with different buffers sharing one declared `CIn` type collide at `102:79` because `instantiate_requirement_value_with` returns **frozen-typed requirements in place**, handing the template's own `.native` cell to every instance.
- The repair follows SPEC §Struct: an instance's field is a **fresh `None` cell of its own `v`**; the template's field is only the requirement. Straight application is blocked by the slot representation (the declared field's value slot holds the type value `compute.Buf (...)`); the open measurement is *which slot a type-valued field's requirement belongs in*.
- Rejected (each measured): conditional frozen copy — invented a frozen/closed-cell rule the SPEC lacks; moving instantiation to `clone_template` — fixed the example and kept all five suites green, but clone exists only at applications, so reverted; the `expected Int, found Type` build of `[fresh, declared]` fails at `convolution2d.lichen:51`.
- The SPEC's constructions are **three, not one** (table): struct instance → fresh `v` cells; array/table/set instance → unify **against the template's cells** (dependent length stays the parameter's node — cloning there silently drops the length check, measured both ways); annotation → unify, never instantiate; application → `@clone`. The verdict: the SPEC is coherent; the implementation runs one routine for all three.
- Strict branch (`spec/instance-v`, then `strict-clean`) follow-ups, **none merged to dev** (lineage check in §3c): the `(raw none, raw none)` read when the copies come out (reads resolve through the declared template); the gate chain that finally located the blocker at the launch operand's undecided kernel item and the empty-cfg item; `check_named_field` lazy read + `formal_root` making `PLRUN eval` fire five times; the CIn collide then re-surfacing as the annotation check's merge into the shared declared type; the directive `不要管 subtype ... = 就是 unify`; the maintainer's principle that a template referencing an outer node **declares it monomorphic** (two different values through it is a genuine conflict) — the consequence being a per-apply clone of the declared type graph, still open.
- The `PCHECK`/`DYN-PCHECK` measurements: `apply_parameter_check` **does** fill the clone's value with the argument's (the earlier "type comparison only" reading was the post-check value mistaken for a frozen one); the curried stage fix `15353da6` made function 19 bind the kernel per call.
- Build-hygiene warnings this note carries: shared `target/` across branches fabricated a collide and a `(raw none)` from the same tree in two rounds; the package cache is shared across branches — every measurement needs a per-branch `LICHEN_HOME`.
### apply-clone-ownership.md

- Rule: an apply's clones are stamped with the **enclosing template's** owner, not the callee's; stamping with the callee makes the enclosing template reference the first call's cells forever. The one exception is the closure walk (own-scope nodes join the fresh closure id; captures keep the source's owner); the static mirror mirrors the policy (residual clones take the caller's tag, baked clones stay unowned).
- Carriage: a clone carries the template's answer only when it is a **template fact** — the gate is `evaluated_deep` alone (`compile-order-class` ruling 8, author's word); `runned` is a separate axis and does not decide; a unify-written value travels with the clone. Quoted invariant (`apply.rs:464-477`): one level deep by design; **"`Any` is undecided for this run policy even though the deep concreteness verdict accepts it"** — the exact duality the migration has to re-create for the hole or delete.
- Never carried: a bare function id (per-call closure; the `minted` table enforces one per call; the frozen pass re-homes a frozen closure once per pass); any answer holding a **foreign** closure (concreteness cannot see through a body).
- §4 the class-wide verdict: clone-vs-reference is a fact of the equality class (never split); scope = the immutable lexical scope tree; class summary = nearest common ancestor of member owners; a class reaching the virtual root is referenced whole; O(log F) ancestor test with per-pass caching. Implementation boundaries: the parameter **image** runs the check even when referenced; the recursive-expansion 534,900-apply trace was a representation fact (unowned type cell), fixed by the ruling that replaced it — function type is the shared Unit `FunctionType`, constraint is a direct Unify.
- §4 frozen path: the artifact (v25+) carries owners and parents; a frozen open cell is **read in place when closed** and becomes the instance's own when open; materialization uses one canonical dynamic **outer instance** per external open cell per top-level pass (unowned at creation so its summary stays external; promoted only by an enclosing inside verdict); a dynamic walk judging a frozen class takes its counterpart scope only from a same-module `static_origin`.
- §5/§7/§8/§9 recovered measurements that matter for the hole migration: the definition pass pins what a clone copies and an uncomputed body re-runs per application (name-tables compare by identity); proven-condition asserts reference in place; the declared parameter attribute is deliberately left **undecided** so each apply resets it (`f (5 # 4)` yields `5 # 4`); repeated calls to one function: the fix **copies** supplied values into the static parameter image's empty slots instead of joining its class; every node is *placed* (`None` is the root place, summary refreshed at allocation) — which is what removed the `366` untagged-contamination; `Pair` now sits inside each operator's body so routed operators clone per call.
- Rejected/withdrawn carried into §8's text: a separate `root_scope` object (broke both `loop_marker` shapes); graph reachability as ownership; the `Instantiate` operator experiment (withdrawn, operator gone); the old self-referential `f : f` type.
- Measured green set on those changes (this is the ceiling the migration should hold): language 289, lib 69, highlevel 136, lowlevel 167/0/1 after the §9 reset restore, compute 283/1, gpu 94 — but note those numbers are **strict-clean-era measurements**; the dev@174379fb baseline this report records is lower in compute and language (§1), and this slice verified none of the WIP line is in dev.
### function-normalization.md

- Concept: normalization is a function's **birth** — the pass settling its parameter/return type cells so later readers see decided facts; once, at first appearance. Site: the lambda-value deep-evaluate loop in `checker.rs` (late-writes-map §2.1). The maintainer's correction: an apply-time read is NOT a normalize; the applied callee is not re-created; the clones belong to the caller.
- The three sanctioned unify producers, no fourth: the checker's initial unify, the birth normalize, the apply's runtime unify (`static_function_apply`, `apply.rs:78/:139`). "Normalize the type term when it is cloned" would be a fourth site and needs a ruling, not an implementation detail.
- Ruling 1: what a determinable cell still open after normalize means is a **normalize defect**, not a read defect; legitimately-open is a `_` hole or genuinely generic position, reported where needed. Ruling 5: a read before the deciding pass sees the cell **empty, never a default**; undecided classes are reported, not guessed. Ruling 7: the **check** is determined at normalize — an instantiation whose callee struct type (name table, field count, field types, attributes) is not determined there is **refused**; the lazy-resolution route is retired, not extended (measured defects of the lazy route: silently short value, unchecked pin and perspective — a deferred check is absent, not late).
- Not retired by ruling 7: **a read is not a check** (`f 5` reads `x.a` and refuses at run because the language states what a wrong container means; `x[0]` on `5` is refused at check because the container is decided wrong); asserts re-evaluate per apply clone (user's own predicate = runtime checking).
- Placeholder callees: a `_(...)` is determined at normalize **by the expected type the context states**; both instance forms (`_{...}`, `_[...]`) pin the placeholder (array form newly fixed; `sum3 (_[1, 2, 3])` answers `6: Int`); no stated context ⇒ refused by name; a *parameter* callee stays refused.
- §5 word-senses: specialize-then-normalize (access rewrite) and `lichen-fusion`'s normalize (algebra canonicalisation) are not this pass.
- Migration reading: under "types determined at normalize", the hole-annotated forms (`{.n _ : Int}`) must have their **types** decided at the function's birth while the value stays open — which is exactly the reading one-answer-per-class.md's maintainer corrections converged on (`=` is unify; the formal's value is what the call fills).
### template-instantiation.md

- Status: the **lowlevel API is current, the source-level `@clone` is retired**; `Module::clone_template` + `TemplateBoundary` stay because the apply walks share the rules; freshness now comes from application (functions) and from the checker's **requirement instantiation** (direct compound templates), the latter copying open value slots **and open type cells**, sharing decided types/attributes/name table/nominal id — the model value-templates.md records.
- The API: `clone_template(root, target, owner, boundary) -> TemplateInstance { root, remap, shared }`; `None` answers a budget-stopped walk — an incomplete instance is worse than none.
- Why its own walk: the apply asks "does this cell depend on the argument" under the **applied** function's scope; the template walk asks "is this cell's class inside the **template's own** scope, is its content not closed"; everything else (one clone per source cell, operator travels with operand, regroup via `apply::regroup_clones`/`unify_clone_groups`) is the apply's, reused. A third freshness rule would be a third semantics.
- The wildcard is the walk's third own decision (§2): an unconstrained template slot is a **capture position**, so the instance's cell **replaces** `@any` with a fresh empty cell the actual fills (the apply keeps wildcards because its body's `@any : t` is a requirement, not a capture). This sentence changes meaning entirely once Any is gone — hole-detachment per §Clone becomes the single mechanism (and per §2.1's "an undecided node detaches from its class", the gate was already reduced to "references only a node the deep pass **proved**").
- §3 rules to preserve verbatim: class decides, not the cell; marked cells shared whole, marked root shares the template; decided-and-closed cells shared, open interior never (a name table is shared because two tables are never one handle — closure is recursive, over payload and trace edges); wildcard decided but never carried; frozen cell open → instance's own, frozen cell closed → read in place; class topology rebuilt; operator travels, operand becomes the instance's own; operand edges do not propagate openness.
- §4 edges: payload edge = cloned and rewired; trace edge (`ValueExt::traced`) = decides closure but is not rewritten (`P::Value` opaque, `ValueExt` has no rewrite hook — the same rule as the apply's `copy_ext`).
- §5 what it does not do: no closure re-homing (no mint), plugin-held cells stay the template's, everything lands in the caller's target block (no per-scope child blocks, unlike the apply's gc-image rule).
- §2.1 measurements: `D{.a 1.5,...}` conflict now answers both; `div {.'dividend,.'dividor}` takes Int and Float pairs; annotation still decides (`Annotation` refusal on `D{.a 1.5}` against `@any : Int`); `examples/gc.lichen` answers because each `add` call needs its own inner parameter; rejected: copying open type expressions (leaked raw buffer markers through `a_frozen_buffer_cell_still_feeds_the_next_launch`); rejected: used the applied clone as the callee (`@gc` programs panicked in `class_walk`, invalid SlotMap keys).
- §6 verification boundary: the walk's own pins were **removed on instruction**; the standing reading is only `--test basic 164/1 ignored` (dev now 167/1) — probes for the fresh/shared/closure behaviour are not in the tree, so the migration cannot rely on a test net for this walk.

### docs/notes/code-audit.md — decisions governing this surface (grep of Any/wildcard/clone/template/annotation)

A `wontfix:D<n>` row is a contract (legend line 24). Applicable:

- **P1-42 `wontfix:D17`** — a `compute.write` the codomain does not reach is a **dead expression**, not a dropped effect: "a `compute.write` is not an effect, it is the value of an output position"; ordinals resolve from the write's `.to` through any chain, never from emission order; the scope of the decision is **global buffers** (a staged `block[1]` write must re-decide it at §9 step 4). The migration must not turn the CIn-type repair into a new write-reachability diagnostic.
- **P1-50 `wontfix:D18`** — an un-annotated recursive fold's result type cannot be decided; **the parameter's shape is the annotation the language requires** (arity not inferable from uses; field types may stay holes). Rejected and measured: the engine route (clone carrying class members + codomain deciders) hung the infinite-stream deep-budget test; gating it regressed the repro. So `_`-holes must not be sold as deciding recursive types; the `a = b _` sugar inherits this limit.
- **D16 (arrays only)** — `e[i]` is refused on tuples/structs; syntax picks the operator, never a runtime kind dispatch. The array-instance rule (`a[n]` read vs `T[x, y]` instance) already depends on it; the `[@any, n]` → `[_ , n]` rewrite must not blur read-vs-instance disambiguation (the comma rule in value-templates).
- **D5** — table key hashing is canonical content unfolding; the `Any` key arm in `table.rs:251-255` is a *decided* part of that machinery (Key mode `Undecided`, Template mode `UNDECIDED_TOKEN`). Replacing the value changes hash semantics: decide before deleting.
- **D20** — a template's own scope may be collected at compile time (the collect roots are the function's template body); the note's clone-walk capture tests rely on it; the hole migration must not assume template cells outlive their scope.
- **D11** — node state is fully encapsulated (six fields): readers of `LowValue::Any` go through `node_value`/`class_value`/`AsEnum`, so the deletion is API-contained; **D15** ownership (a compiled kernel or buffer) is unaffected but referenced by `apply_result_owners` used at `equality.rs:1005-1035`.
- No `wontfix` covers the wildcard itself; the **AGENTS.md** rule applies: the reworded Clone sentence ("the outer most scope among its class members") is now SPEC text and `scope.rs`'s `# Invariant` is the code-side contract — changing its verdict (e.g. to force holes to detach) rewords the invariant and needs the maintainer's permission.

## 5. `# Invariant` / `// Invariant` comments mentioning Any or wildcard

These move or die with the migration (AGENTS.md: dropping a condition from an `# Invariant` is a semantic change needing the maintainer's permission; a note is never the only home of an invariant, so each deletion must land in the code that replaces it). Quoted rule text is verbatim from the cited lines.

**lichen-lowlevel**

- `src/lib.rs:88-93` — `LowValue::Any` variant: "Unification is directional: a left `Any` joins an undecided right class; every other wildcard comparison succeeds without merging or writing. Readers treat `Any` as **undecided**; only the concreteness verdict takes it as decided, so a written type remains reusable while an apply clone replaces `Any` with an open instance cell." **Dies with the variant.**
- `src/equality.rs:386-392` — `clear_wildcard`: "Only a lone wildcard (`Any`) is cleared: it says the position never decides, so emptying it is the one way a statement may give it a value. A shared one is the default attribute array's, and clearing it empties every hole." **Dies** — but note its replacement question: may a statement empty a shared *hole* cell? (The shared default attribute array at `checker.rs:1031` would need the same protection or a fresh-per-write rule.)
- `src/equality.rs:459-463` — `value_holds_open_cell`: "An open cell is a node with no value or with `Any` — the wildcard a template's field states, which the SPEC calls undecided. It is what distinguishes a template's requirement from a thing's answer." Second clause **dies**; the distinction it encoded (requirement vs answer) must survive on another axis or it is a semantic change.
- `src/equality.rs:1124-1127` — `function_has_wildcard`: "`Any` recurses only into arrays: signature comparison stops before parameter/return recursion. Constructor binding still runs, and Apply specializes with the actual argument." **Dies**; the wildcard-constructor machinery (`lib.rs:1104-1112` `constructor_bindings`, `struct_identity_items` invariant: "A clone references these in place and never copies them: the table and the id are the declaration's, so a per-call copy would give one type two identities") loses its wildcard keys but the identity rule itself is Any-independent.
- `src/equality.rs:1382-1386` — `alias_read`: "The join skips a wildcard target, which is shared: merging it would write its value into the reader. The reader keeps its operation, so the operand edge stays live for the clone walk..." **Reword** (to "empty-or-wildcard", then to "empty") — a careless deletion makes the reader adopt a shared undecided cell's value.
- `src/evaluation.rs:975-979` — deep verdict over frozen values: "An array's **template** position holds the layout, not content, so its verdict is not read: the element template of an array instance is `@any` more often than not..." — the reason-clause **dies**; the skip rule stays.
- `src/evaluation.rs:1013-1024` — `evaluate_block`: "The root's own slot is the signal — a real value, not the wildcard — ... An undecided root keeps the marker, so a later read retries..." — **re-key**: today the test is `!Any`; with holes "real value" == non-empty, which is *stricter* (an empty root never releases). Behaviour change unless the marker rule is re-argued.
- `src/apply.rs:464-470` — `answer_elements_are_undecided`: "One level deep by design... **`Any` is undecided for this run policy even though the deep concreteness verdict accepts it**." The bolded sentence is the one-line statement of the whole Any duality; **it dies**, and §3's open question (which axis a hole takes) replaces it.
- `src/function.rs:605-612` — `class_answer_is_decided`: "`Any` is not an answer: it stays open and triggers replay only when an open instance cell still needs the call's value; a fully bound instance creates no replay." **Reword**; an empty is already "no answer", but `runned==0 with a value present means a unification wrote it` (apply-clone-ownership §2) keeps `Any` distinguishable from a unify-written value — after deletion that distinction disappears.
- `src/function.rs:117` — `ApplyCtx.in_parameter` comment: "...wildcard cells become holes only here" — **reword** (and see apply-clone-ownership §9: the flag's reset is load-bearing).
- `src/static_module/apply.rs` wildcard arms (`474-478`, `565-570`) carry the same rule as function.rs; the surrounding `# Invariant`s (`FrozenClassRule`, outer instance) do not name Any — they survive unchanged.
- `src/template.rs:196-203` — `template_closure_map`: "A cell is **open** when its own content is not closed: it holds no value, it holds the wildcard, it has an operator, or its content reaches an open cell." — clause dies, verdict must not change: with "holds the wildcard" removed, cells that held Any become "no value" ⇒ still open ⇒ still cloned. Verify, then reword.
- `src/template.rs:236-241` — `template_content_is_closed`: "A closed cell holds a decided value that is not the wildcard, it has no operator, and every cell its content reaches is closed too..." — same treatment.
**lichen-highlevel**

- `src/program.rs:428-433` — `AnyLit`: "The value node is **fresh per occurrence**: the spelling's own type cell is what `@any : T` binds, and a shared cell would make the first annotation bind every later one." **Dies with AnyLit**; the freshness argument transfers to whatever a written `_ : T` becomes (today a `Placeholder` builds fresh cells — see checker.rs:1850, so the property survives at the placeholder site).
- `src/checker/annotations.rs:217-221` — `carried_slot`: "Only a **carried** position answers: a canonical index the layout omits is read from the module's default attribute array, which is the shared wildcard, and no caller may write it." **Reword**: the default attribute array is `checker.rs:1029-1031` (`any_value`, comment "Default positions share this wildcard; source literals and function sugar are fresh"); its replacement (shared empty hole?) must keep the no-caller-writes rule or the GC/holes leak into the shared default.
- `src/checker/annotations.rs:264-275` — `adopt_stated_attributes`: "Only a **wildcard** position is cleared: a slot the value already states is its own. `Any` unifies with everything and merges with nothing, so a wildcard left in place says the position never decides, and a call site reads the argument instead of the callee's claim." **Reword-plus-reride**: `stated_slot` (`:240-246`) currently calls "no value" *stated*, and `carried_wildcard` (`:248-258`) requires exactly `Any`; after deletion the pair collapses to stated=non-empty / wildcard=empty, which changes what a statement may clear. This is the sharpest semantic cliff in the migration.
- `src/checker/annotations.rs:548-560` — `unify_element_attributes`: "An element slot's attribute positions hold the requirement and a value's element slot positions are `@any`, so the lowlevel compare is vacuous by construction: the reconciliation is here, by marker, exactly as a whole annotation's is..." **Reword**; vacuity now comes from emptiness (unify arm `(None,None) => merge` is *not* vacuous — the compare would merge the classes: the checker must keep steering these pairs away from `unify` or accept the merge — decision needed).
- `src/checker/annotations.rs:613-620` — `unify_element_values`: "A value's element slots hold the wildcard, so the lowlevel compare cannot enforce a pin; ... a wildcard's own class makes its arm vacuous — which is exactly why `@any : T` is no pin." Same; and the SPEC sentence that used to license `@any : T` is gone, so the hole-annotated field must be re-argued from §Annotation instead.
- `src/checker/indexing.rs:674-679` — `wildcard_requirement`: "Empty alone is not wildcard: the node's origin must reach `Any`, so an ordinary undecided or computed pin still checks the whole expression." **Dies**: this invariant's *only* job is telling `Any` from `_`. After migration the origin-reach (`wildcard_slot`, `:639-673`) has nothing to reach; either requirement detection moves to the expr kind (the checker knows the slot came from a template requirement) or the distinction is genuinely gone — which changes `deferred element` checking. A brief that calls this file an incidental casualty is wrong.
- `src/checker/tuples.rs:57-63` — `element_slot_of`: "A top-level `:` spells the value position; an element without one is a type expression whose value position is the wildcard." **Reword** to hole.
- `src/checker/tuples.rs:90-97` — `element_slot`: "The width is `shape::slot_width` whatever an element spells... an unconstrained position holds `Any`, replaced by an open cell in each apply clone." **Reword**: the replacement cell is what the clone mints anyway (function.rs:866); after migration the written form is already the open cell — check the per-apply freshness argument still closes.
- `src/checker.rs:1520-1524` — `slot_expr`: "Every expr the checker mints comes from here... every other slot is the wildcard an unstated attribute is." **Reword**; consumers at `:1440-1451`, `:1965-1985` ("The wildcard states nothing: it is the hole an expr carries for an attribute no one named") already call the wildcard *the hole* — the migration makes the words agree and must re-check every reader that branched on the difference.
- `src/checker.rs:1637-1641` and `src/ir.rs:618-622` — these two Invariants say "wildcard" meaning the **Rust match arm** ("named rather than caught by a wildcard"). **False positives — leave alone.** Useful trap for whoever greps the same census.
- `src/shape.rs:686-696` (inline) — "The wildcard states nothing, so it has no shape to seed": after migration, does an empty hole seed the class low shape? Today `None`-valued slots are also skipped by the same arm only via the `Any` test... re-verify against the `class_low_shape` seeding rule.
**lichen-language-parser / render / compute**

- `crates/lichen-language-parser/src/parse.rs:1174-1178` — `primed_field_value`: "The tail is an annotation chain, so the position it leaves open — the wildcard head of a value-less chain — is exactly where the formal goes; a tail that is another expression is refused, not applied." **Reword** to "the hole head"; the head is minted at `parse.rs:793-801` and matched at `:1194` — both must switch to the placeholder/hole form together.
- `crates/lichen-render/src/render/type_printer.rs:390-394` — `slot_any`: "An element that spells no value holds the wildcard, which constrains nothing, so source states no value either; anything else is a pin and states it. See `docs/notes/compound-element-slots.md`." **Reword.**
- `crates/lichen-render/src/render/type_printer.rs:451-456` — `slot_value_any`: "The wildcard is **one shared node**, so an apply's clone mints a fresh undecided cell in its place — a slot read through a call therefore shows a hole, not `@any`. A hole is no pin either, so the position is suppressed the same way, and only a written `v : T` prints." **Reword** — this sentence already *describes* the hole rendering; after migration its first clause is vacuous and the suppression rule stands on the hole alone.
- `crates/lichen-render/src/render/value_printer.rs:267-274` — `atomic`: invariant text about `?` fallback; the `matches!(... Any) => "@any"` arm below it **dies** — declared example outputs change.
- `crates/lichen-compute/src/compute.rs:6605-6608` — `recorded_leaf`: "Only an empty **or wildcard** leaf follows `field_sources`; a decided value stays authoritative, and at most eight links are followed." **Reword**: dropping "or wildcard" changes which leaves stop the walk today? No — after migration wildcard cells *are* empty, so the clause becomes redundant, not different. Same reading for `:6799-6841` (`buffer_open`/`payload_open`), `:4100-4107` (`decided_buffer_value` doc "absent or `@any`"), `:1513` (cfg read fallback), `:7496/8144` (GraphInput and result-slot `Any` mints → hole mints) — the compute layer has its own `@any` open/closed convention that matches value-templates.md's incomplete-migration note; it should be migrated *with* the lowlevel, not after.

## 6. Findings the parent should weigh before dispatching

1. **The hole machinery already exists and the checker already calls the wildcard "the hole"** (`checker.rs:1850-1856` builds `_` as fresh cells "not the wildcard (spec §1)"; `:1967` "The wildcard states nothing: it is the hole an expr carries..."). The migration is therefore not "add `_`" but **collapse two representations whose differences are load-bearing in exactly six places**: the deep concreteness verdict (Any decided / empty not, `evaluation.rs:884`), the scope-release test (`:1013`), the stated-vs-wildcard attribute pair (`annotations.rs:240-263` — an *empty* slot counts as *stated* there, an `Any` counts as wildcard), `indexing.rs:674` (empty alone is not wildcard), `clear_wildcard` (a statement may empty `Any` but nothing else occupied), and the run-policy line `apply.rs:470`. Each is an `# Invariant`; changing any is a semantic change requiring the maintainer's word (AGENTS.md, comments policy).
2. **Directionality is the cheap half.** Unify is already directional *because of* the wildcard arms; with `Any` deleted, `_ = X` (undecided vs decided) keeps the merge direction via the existing `(None, Some(x))` arm (`equality.rs:1239-1243`), and `X = _` likewise — but `_ = @any` was the one non-merge (`:1245-1251`), and with Any gone that arm is unreachable rather than harmful. No new unify semantics are needed for the value layer; the SPEC sugar `a <: b => a = b _` becomes implementable with the current engine plus a `<:` token (none today; pinned by `lex_tests.rs:254-256`).
3. **The expensive half is what one-answer-per-class measured on strict-clean and dev has none of**: every WIP commit (`f48883c4`..`61c537e0`, verified NOT-ancestors) is out of dev, so the CIn-collide is dev's live behaviour (the 11 language failures + 148 compute failures are downstream of exactly these families). If the parent removes Any without the per-apply clone of the declared type graph (the maintainer's "template references ⇒ polymorphic in fact ⇒ clone" consequence), the collide survives as an empty-cell-sharing problem instead of an Any-sharing one — same tests red, new names. The clone-walk decision point is `class_inside_scope` (`scope.rs:204`) and 5fcc181a's clean-applying outermost-member rewrite is the recorded candidate; landing it needs the invariant reword approved first.
4. **Surface migration order suggested by the census**: lexer token + grammar.js `wildcard` node + editor queries first (pure spelling), then parser head-fold (`parse.rs:793-801`, `:1174-1202`), then `AnyLit`/`Expr::Any` removal from lowering and the checker `any()` sites, then the lowlevel variant with its six cliff-sites, then compute's openness convention (`compute.rs` §2), then examples/prelude re-spelling (core.lichen's annotated holes are the riskiest single file — every operator routes through it, apply-clone-ownership §8), then doc/README/printer-output sync. Tests carrying `@any` are ~770 lines of embedded source (verbatim list in §2 dump) and must be re-spelled *with* the grammar, not before.
5. **Contracts that must not be renegotiated** (code-audit): D17 (`compute.write` is the value of an output position — no new dropped-write diagnostic), D18 (holes do not infer recursive arity — do not sell `_` as fixing `P1-50`), D16 (syntax picks the read operator), D5 (canonical key hashing — decide the Any key arm's replacement before deleting), D11 (all node-state reads are encapsulated, so the deletion is API-contained).
6. **Baseline honesty**: compute 135/148/1 and language 278/11 at dev `174379fb` are *known red*; the migration's bar is no new failures outside the Any/template surface plus the SPEC's own forms answering. The green anchors to hold: highlevel 136, lowlevel basic 167/1-ignored, language lib 69, parser lib 62, lex lib 22.

---
 artefacts: `target/recon/{check,test_build,test_run,test_run2}.log`, `target/recon/{check,test_build,test_run,test_run2}.exit`, `target/recon/{census_rstok,census_atsig,census_atsig_code,failed_names,results_table,table2}.txt`, `target/recon/vcvars.env` (empty — the bash-side capture failed, the PowerShell import worked; kept to record that dead end).
 constraint compliance: no tracked file edited; no test added; the only written file in the tree is `recon-report.md`; no process left running (all channels exited).
