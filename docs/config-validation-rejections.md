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
