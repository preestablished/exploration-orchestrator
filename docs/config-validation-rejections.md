# Config Validation Rejections

This is the committed config rejection catalog for API.md section 7 input
validation. These messages are surfaced as gRPC `INVALID_ARGUMENT` details and
standalone config errors. They are not runtime terminal failure reasons.

## Catalog

- `missing required field <field>`
- `field out of range <field>`
- `unknown enum value <field>`
- `invalid config version <version>`
- `staged inner policy cannot be staged`
- `decoded feature not in feature_map <name>`

## Accepted zero-value shapes

`budgets.max_wall_clock_s = 0` is accepted, not rejected: zero disables the
wall-clock budget. This behavior is frozen by the core matrix
(`config_matrix.rs` non-rejectable shapes) and the wire surface test
(`config_validation_surface.rs`).
