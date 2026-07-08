//! Stable config rejection strings for API.md section 7 validation.
//!
//! These are input-validation messages surfaced as INVALID_ARGUMENT over gRPC
//! and as standalone config errors. They are intentionally separate from
//! runtime terminal failure reasons.

pub const MISSING_REQUIRED_FIELD: &str = "missing required field <field>";
pub const FIELD_OUT_OF_RANGE: &str = "field out of range <field>";
pub const UNKNOWN_ENUM_VALUE: &str = "unknown enum value <field>";
pub const INVALID_CONFIG_VERSION: &str = "invalid config version <version>";
pub const INVALID_STAGED_INNER_POLICY: &str = "staged inner policy cannot be staged";
pub const DECODED_FEATURE_NOT_IN_FEATURE_MAP: &str = "decoded feature not in feature_map <name>";

pub const CATALOG: &[&str] = &[
    MISSING_REQUIRED_FIELD,
    FIELD_OUT_OF_RANGE,
    UNKNOWN_ENUM_VALUE,
    INVALID_CONFIG_VERSION,
    INVALID_STAGED_INNER_POLICY,
    DECODED_FEATURE_NOT_IN_FEATURE_MAP,
];

pub fn missing_required_field(field: &str) -> String {
    format!("missing required field {field}")
}

pub fn field_out_of_range(field: &str) -> String {
    format!("field out of range {field}")
}

pub fn unknown_enum_value(field: &str) -> String {
    format!("unknown enum value {field}")
}

pub fn invalid_config_version(version: u32) -> String {
    format!("invalid config version {version}")
}

pub fn decoded_feature_not_in_feature_map(name: &str) -> String {
    format!("decoded feature not in feature_map {name}")
}
