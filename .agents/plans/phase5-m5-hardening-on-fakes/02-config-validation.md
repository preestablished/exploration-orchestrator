# Config Validation Matrix

Scope: `API.md` section 7 and standalone mode section 8. Config rejection is an input
validation concern, not an experiment `FAILED` state.

## W5.1 - Centralize config rejection strings

Add a stable catalog for config violations. Keep messages terse and field-scoped:

- `missing required field <field>`
- `field out of range <field>`
- `unknown enum value <field>`
- `invalid config version <version>`
- `staged inner policy cannot be staged`
- `decoded feature not in feature_map <name>` or equivalent exact text

The exact strings can keep today's `Display` output where it is already good, but
the list must be mechanically discoverable from code. Add
`docs/config-validation-rejections.md` and a test that parses the doc list and
compares it to the catalog.

Acceptance:

- One module exposes all committed config rejection message templates or concrete
  strings.
- `ConfigError`/wire validation uses those constants rather than ad hoc text.
- A drift test fails if the doc and code catalog diverge.

## W5.2 - Build the API.md section 7 invalid-shape matrix

Add table-driven tests that cover every field/oneof in the schema. Recommended
test locations:

- `crates/orch-core/tests/config_matrix.rs` for core/effective validation.
- `crates/orch-server/tests/config_validation_surface.rs` for gRPC/YAML surface
  behavior.

Minimum invalid cases:

| API.md field | Invalid case |
|---|---|
| `version` | `version != 1` |
| `seed` | `seed == 0` as required-field absence, unless deliberately overruled |
| `workload_image_ref` | empty |
| `feature_map_ref` | empty |
| `scoring_program_ref` | empty |
| `synth_config_ref` | empty |
| `macro_pack_refs` | malformed/empty entry if the implementation chooses to reject empty refs; otherwise document that repeated string entries are opaque refs and not locally validated |
| `budgets.max_nodes` | no invalid local shape if `0 = unlimited`; assert explicit zero survives defaulting |
| `budgets.max_wall_clock_s` | no invalid local shape if `0 = unlimited`; assert explicit zero survives defaulting |
| `budgets.max_guest_instructions` | no invalid local shape if `0 = unlimited` |
| `budgets.max_expansions` | no invalid local shape if `0 = unlimited` |
| `selection.policy` | unknown enum number |
| `selection.alpha` | NaN or negative |
| `selection.beta` | NaN or negative |
| `selection.gamma` | NaN or negative |
| `selection.delta` | NaN or negative |
| `selection.temperature` | non-finite or `<= 0` |
| `selection.ucb_c` | NaN or negative |
| `selection.staged.inner` | unknown enum number |
| `selection.staged` | `policy = STAGED` with `inner = STAGED` |
| `selection.staged.epsilon_regress` | outside `[0, 1]` or non-finite |
| `selection.max_visits_per_node` | effective zero after defaulting rules if expressible |
| `selection.exhaust_after_dup_expansions` | effective zero after defaulting rules if expressible |
| `burst.k_per_expansion` | `257`; also verify `0` defaults to 16 if keeping current proto3 defaulting |
| `burst.base_burst_len_frames` | greater than `max_burst_len_frames` |
| `burst.max_burst_len_frames` | effective zero if expressible |
| `burst.max_guest_instructions_per_job` | no invalid local shape if `0 = worker default` |
| `plateau.window_n` | `< 10` |
| `plateau.epsilon_s` | non-finite or `<= 0` |
| `plateau.ladder.burst_len_factor` | `< 1` or non-finite |
| `plateau.ladder.temp_factor` | `< 1` or non-finite |
| `plateau.ladder.macro_weight_hot` | negative or non-finite |
| `plateau.ladder.backtrack_kappa` | negative or non-finite |
| `plateau.ladder.backtrack_depth_quantile` | outside `[0, 1]` or non-finite |
| `plateau.ladder.radius_factor` | `< 1` or non-finite |
| `plateau.ladder.max_level` | `> 4` |
| `scheduling.mode` | unknown enum number |
| `scheduling.max_inflight_batches` | `0` in FAST if expressible; verify DETERMINISTIC coerces to 1 |
| `scheduling.job_timeout_s` | effective zero if expressible |
| `scheduling.retry_max` | decide and document whether zero means no retries or invalid; test the chosen semantics |
| `scheduling.retry_backoff_ms` | decide and document whether zero means immediate retry or invalid; test the chosen semantics |
| `scheduling.hypervisor_endpoints` | empty entry if local validation treats endpoints as non-empty strings; otherwise document they are resolved by service config |
| `scheduling.allow_class_mismatch` | boolean, no invalid local shape |
| `checkpoint.every_commits` | effective zero if expressible |
| `checkpoint.every_seconds` | effective zero if expressible |
| `prune_action` | unknown enum number |
| `on_goal` | unknown enum number |
| `decoded_features` | unknown feature name against a compiled feature map |

Where proto3 defaulting makes an invalid zero unexpressible, the test should
assert the defaulting behavior and the doc should say "not rejectable over proto3
v1". Do not pretend it was rejected.

Acceptance:

- Every API.md section 7 field is represented in the table with either a rejecting test
  or an explicit "not rejectable in v1 wire shape" assertion.
- Every rejecting test asserts the exact message.

## W5.3 - Prove gRPC and YAML share the same validator

Today both paths call `effective_config(...).validate_all()`, but there is no
end-to-end proof that surfaced errors match. Add one test that builds the same
bad sparse wire config and bad YAML config, then asserts:

- gRPC `StartExperiment` returns `Status::invalid_argument`.
- `orchestratord --experiment` or the standalone loader returns the same
  `config validation failed: ...` detail after stripping the transport prefix.

Use a validator-level invalid value such as `burst.k_per_expansion: 257`, not a
YAML parser error.

Acceptance:

- One shared helper formats validation violations for both paths.
- The surface test fails if gRPC and YAML diverge on details.

## W5.4 - Validate `decoded_features` against the compiled feature map

API.md section 7 requires every configured decoded feature name to exist in the feature
map. This is not pure `ExperimentConfig` validation because it needs the compiled
map. Add a bring-up validation step after feature-map compilation and before the
runner starts issuing work.

Acceptance:

- A config with `decoded_features: ["does_not_exist"]` fails before the loop.
- The failure is `INVALID_ARGUMENT` over gRPC and a standalone config error over
  `--experiment`.
- The exact field/message is in the config rejection doc.

## W5.5 - Record matrix evidence

Add a small evidence script or extend the final M5 script to capture the named
matrix tests into `evidence/phase5-m5-hardening/config-validation.txt`.

Suggested command set:

```bash
cargo test -p orch-core config_matrix -- --nocapture
cargo test -p orch-server --test config_validation_surface -- --nocapture
```

The evidence file should include the test names and the final `test result` lines,
not full unbounded cargo logs.
