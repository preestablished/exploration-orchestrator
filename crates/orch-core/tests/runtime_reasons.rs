use std::path::Path;

use orch_core::runtime_reasons;

#[test]
fn runtime_failed_reason_doc_matches_code_catalog() {
    let doc_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("runtime-terminal-reasons.md");
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", doc_path.display()));

    let mut in_failed = false;
    let entries: Vec<String> = doc
        .lines()
        .filter_map(|line| {
            if line == "## FAILED Prefixes" {
                in_failed = true;
                return None;
            }
            if in_failed && line.starts_with("## ") {
                in_failed = false;
            }
            if !in_failed {
                return None;
            }
            line.strip_prefix("- `")
                .and_then(|rest| rest.strip_suffix('`'))
                .map(str::to_owned)
        })
        .collect();

    assert_eq!(entries, runtime_reasons::FAILED_REASON_PREFIXES);
}

#[test]
fn cataloged_prefixes_pass_through_unchanged() {
    for prefix in runtime_reasons::FAILED_REASON_PREFIXES {
        let message = format!("{prefix}: detail");
        assert_eq!(runtime_reasons::cataloged_failed_reason(&message), message);
    }
}

#[test]
fn uncataloged_messages_wrap_under_runtime_error() {
    assert_eq!(
        runtime_reasons::cataloged_failed_reason("scorer stream reset by fake"),
        "runtime-error: scorer stream reset by fake"
    );
}

#[test]
fn cataloged_failed_reason_is_idempotent() {
    let wrapped = runtime_reasons::cataloged_failed_reason("boom");
    assert_eq!(runtime_reasons::cataloged_failed_reason(&wrapped), wrapped);
}
