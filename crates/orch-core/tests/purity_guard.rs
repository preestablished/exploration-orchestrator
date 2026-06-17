use std::collections::BTreeSet;
use std::process::Command;

#[test]
fn normal_dependency_tree_excludes_runtime_io_network_and_clock_crates() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "orch-core",
            "--edges",
            "normal,build",
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

    let tree = String::from_utf8(output.stdout).expect("cargo tree output was not UTF-8");
    let present = dependency_names(&tree);
    let forbidden = [
        // Async runtimes and executors.
        "async-executor",
        "async-io",
        "async-net",
        "async-std",
        "futures-executor",
        "smol",
        "tokio",
        // gRPC, HTTP, socket, and event-loop stacks.
        "h2",
        "hyper",
        "mio",
        "quinn",
        "reqwest",
        "socket2",
        "tonic",
        "tonic-build",
        "ureq",
        // Filesystem watching/tempfile and wall-clock crates.
        "chrono",
        "filetime",
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
        "orch-core normal/build dependency tree contains forbidden crates: {violations:?}\n\n{tree}"
    );
}

fn dependency_names(tree: &str) -> BTreeSet<&str> {
    tree.lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect()
}
