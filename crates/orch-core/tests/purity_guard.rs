use std::collections::BTreeSet;
use std::process::Command;

#[test]
fn production_dependency_tree_matches_reviewed_allowlist() {
    let tree = cargo_tree("normal,build");
    let present = dependency_names(&tree);
    let allowed = [
        "allocator-api2",
        "arrayref",
        "arrayvec",
        "blake3",
        "cc",
        "cfg-if",
        "cobs",
        "constant_time_eq",
        "cpufeatures",
        "equivalent",
        "find-msvc-tools",
        "hashbrown",
        "libc",
        "libm",
        "orch-core",
        "postcard",
        "ppv-lite86",
        "proc-macro2",
        "quote",
        "rand_chacha",
        "rand_core",
        "serde",
        "serde_core",
        "serde_derive",
        "shlex",
        "syn",
        "thiserror",
        "thiserror-impl",
        "unicode-ident",
        "zerocopy",
        "zerocopy-derive",
    ]
    .into_iter()
    .map(String::from)
    .collect::<BTreeSet<_>>();

    let unexpected = present.difference(&allowed).collect::<Vec<_>>();
    assert!(
        unexpected.is_empty(),
        "orch-core normal/build dependency tree contains unreviewed crates: {unexpected:?}\n\n{tree}"
    );

    assert_no_forbidden_dependencies(&present, &tree);
}

#[test]
fn all_dependency_edges_exclude_runtime_io_network_and_clock_crates() {
    let tree = cargo_tree("all");
    let present = dependency_names(&tree);

    assert_no_forbidden_dependencies(&present, &tree);
}

fn cargo_tree(edges: &str) -> String {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--locked",
            "-p",
            "orch-core",
            "--edges",
            edges,
            "--target",
            "all",
            "--prefix",
            "none",
            "--no-dedupe",
        ])
        .output()
        .expect("failed to run cargo tree for orch-core purity guard");

    assert!(
        output.status.success(),
        "cargo tree failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).expect("cargo tree output was not UTF-8")
}

fn assert_no_forbidden_dependencies(present: &BTreeSet<String>, tree: &str) {
    let forbidden = [
        // Async runtimes and executors.
        "async-executor",
        "async-global-executor",
        "async-io",
        "async-net",
        "async-std",
        "futures-executor",
        "smol",
        "tokio",
        // gRPC, HTTP, socket, and event-loop stacks.
        "curl",
        "h2",
        "hyper",
        "isahc",
        "mio",
        "quinn",
        "reqwest",
        "socket2",
        "surf",
        "tonic",
        "tonic-build",
        "ureq",
        // Filesystem watching/tempfile and wall-clock crates.
        "chrono",
        "filetime",
        "fs-err",
        "notify",
        "tempfile",
        "time",
        "walkdir",
    ];

    let violations = forbidden
        .into_iter()
        .filter(|name| present.contains(*name))
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "orch-core dependency tree contains forbidden crates: {violations:?}\n\n{tree}"
    );
}

fn dependency_names(tree: &str) -> BTreeSet<String> {
    tree.lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(String::from)
        .collect()
}
