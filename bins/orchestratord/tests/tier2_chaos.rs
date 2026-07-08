//! Tier-2 true-SIGKILL chaos harness (plan W2.4/W2.5, bead
//! `exploration-orchestrator-6ft`): the Tier-1 standard across a process
//! boundary. Every kill is a real SIGKILL (`Child::kill`) landing on the
//! whole `orchestratord` process; after any number of them, the resumed
//! run's committed tree/archive state must be bit-identical to an
//! uninterrupted control run, with the persisted checkpoint at
//! `GoalReached` (exit 0 alone also covers BudgetExhausted/Stopped).
//!
//! Kill classes (plan D-T3):
//! 1. lattice — `ORCH_CHAOS_HANG_AT=<point>:<nth>` parks the child at one
//!    of the 11 Tier-1 crash points; the harness sees the marker and kills.
//! 2. forced torn writes — `ORCH_SIM_TORN_AT=<wal-append|ckpt-put>:<nth>`
//!    parks mid-journal-frame after a torn prefix; the relaunch must land
//!    on the torn-tail truncation path (nonzero `truncated_bytes`).
//! 3. random — sleep a random real-time interval, kill wherever the child
//!    happens to be; early clean exits don't count toward the quota.
//!
//! Scaling env (plan D-T5): `TIER2_SEEDS` (default 1; evidence lane 5),
//! `TIER2_LATTICE=reduced|full` (default reduced: AfterWalWrite,
//! BeforeCasPut, AfterCasPut), `TIER2_RANDOM_KILLS` (default 2),
//! `TIER2_PARALLEL` (worker threads across rounds; state-dirs are
//! independent and the workload is fsync-bound), `TIER2_MIN_KILLS`
//! (evidence lane sets 50), and `CHAOS_SEED` (single-seed override, same
//! contract as `chaos_resume.rs`).

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use orch_checkpoint::{decode_checkpoint, ExperimentState};
use orch_clients::snapshot_store::{GetMetadataRequest, MetadataKey, SnapshotStoreClient};
use orch_server::config::{config_hash, effective_config, wire_config_from_yaml};
use orch_server::experiment::CrashPoint;
use orch_simstate::compare::{
    assert_no_stranded_frontier, scorer_archive_fingerprint, store_tree_hash,
};
use orch_simstate::world::PersistentServices;

const EXPERIMENT_ID: &str = "tier2";
/// Bound on waiting for a hang marker or a completion run; generous — a
/// full journaled debug run measured ~4 min on an ext4 workstation.
const ROUND_TIMEOUT: Duration = Duration::from_secs(1800);

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_orchestratord")
}

/// The harness spawns real processes and fsyncs per mutating op — minutes,
/// not seconds. It runs in the dedicated `tier2-chaos-smoke` CI job and the
/// evidence lane (both set `TIER2_ENABLE=1`), not in every workspace test
/// sweep.
fn enabled() -> bool {
    if std::env::var("TIER2_ENABLE").is_ok() {
        return true;
    }
    eprintln!("tier2_chaos: skipped (set TIER2_ENABLE=1 to run)");
    false
}

fn config_yaml(seed: u64) -> String {
    // The Tier-1 grid tuning (`support::grid_config`) expressed as the
    // sparse wire YAML `wire_config_from_yaml` accepts. FaultPlan stays
    // disabled — a journal soundness invariant (D-T4), not a tuning choice.
    format!(
        "version: 1\n\
         seed: {seed}\n\
         workload_image_ref: workload://grid\n\
         feature_map_ref: featmap://grid\n\
         scoring_program_ref: score://grid\n\
         synth_config_ref: synth://grid\n\
         budgets:\n\
         \x20 max_nodes: 0\n\
         \x20 max_wall_clock_s: 86400\n\
         \x20 max_guest_instructions: 0\n\
         \x20 max_expansions: 4096\n\
         burst:\n\
         \x20 k_per_expansion: 8\n\
         \x20 base_burst_len_frames: 3\n\
         \x20 max_burst_len_frames: 12\n\
         selection:\n\
         \x20 temperature: 8.0\n\
         \x20 max_visits_per_node: 256\n\
         \x20 exhaust_after_dup_expansions: 32\n\
         checkpoint:\n\
         \x20 every_commits: 16\n\
         \x20 every_seconds: 3600\n\
         scheduling:\n\
         \x20 mode: deterministic\n"
    )
}

fn seeds() -> Vec<u64> {
    if let Ok(value) = std::env::var("CHAOS_SEED") {
        return vec![value.parse().expect("CHAOS_SEED must be a u64")];
    }
    let count: u64 = std::env::var("TIER2_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    (0..count).map(|index| 0x5EED + index * 7).collect()
}

fn lattice_points() -> Vec<CrashPoint> {
    match std::env::var("TIER2_LATTICE").as_deref() {
        Ok("full") => CrashPoint::ALL.to_vec(),
        _ => vec![
            CrashPoint::AfterWalWrite,
            CrashPoint::BeforeCasPut,
            CrashPoint::AfterCasPut,
        ],
    }
}

fn random_kills_per_seed() -> u32 {
    std::env::var("TIER2_RANDOM_KILLS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2)
}

fn parallelism() -> usize {
    std::env::var("TIER2_PARALLEL")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get().min(4))
                .unwrap_or(2)
        })
        .max(1)
}

/// Deterministic per-round RNG for random-kill delays (no rand dep).
struct Lcg(u64);

impl Lcg {
    fn next_ms(&mut self, low: u64, high: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        low + (self.0 >> 33) % (high - low)
    }
}

struct Launched {
    child: Child,
    lines: Arc<Mutex<Vec<String>>>,
    receiver: mpsc::Receiver<String>,
}

/// Launch one `orchestratord --experiment` incarnation with the given
/// extra env hooks, stdout piped through a reader thread.
fn launch(yaml: &Path, dir: &Path, envs: &[(&str, String)]) -> Launched {
    let mut command = Command::new(bin());
    command
        .arg("--experiment")
        .arg(yaml)
        .arg("--experiment-id")
        .arg(EXPERIMENT_ID)
        .arg("--state-dir")
        .arg(dir)
        .env_remove("ORCH_CHAOS_HANG_AT")
        .env_remove("ORCH_SIM_TORN_AT")
        .env_remove("ORCH_SIM_BREAK")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn orchestratord");
    let stdout = child.stdout.take().expect("piped stdout");
    let lines = Arc::new(Mutex::new(Vec::new()));
    let (sender, receiver) = mpsc::channel();
    let sink = Arc::clone(&lines);
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            sink.lock().expect("lines mutex").push(line.clone());
            let _ = sender.send(line);
        }
    });
    Launched {
        child,
        lines,
        receiver,
    }
}

enum WaitOutcome {
    Marker,
    CleanExit,
    FailedExit(Option<i32>),
}

/// Wait until the child prints a `TIER2_CHAOS_HANG` marker or exits.
fn wait_marker_or_exit(launched: &mut Launched) -> WaitOutcome {
    let deadline = std::time::Instant::now() + ROUND_TIMEOUT;
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .expect("tier2 round timed out waiting for marker or exit");
        match launched
            .receiver
            .recv_timeout(remaining.min(Duration::from_millis(250)))
        {
            Ok(line) if line.starts_with("TIER2_CHAOS_HANG") => return WaitOutcome::Marker,
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // stdout closed: the child is exiting; reap it.
                let status = launched.child.wait().expect("wait child");
                if status.success() {
                    return WaitOutcome::CleanExit;
                }
                return WaitOutcome::FailedExit(status.code());
            }
        }
        if let Some(status) = launched.child.try_wait().expect("try_wait child") {
            // Drain any tail lines already captured by the reader.
            while let Ok(line) = launched.receiver.try_recv() {
                if line.starts_with("TIER2_CHAOS_HANG") {
                    return WaitOutcome::Marker;
                }
            }
            if status.success() {
                return WaitOutcome::CleanExit;
            }
            return WaitOutcome::FailedExit(status.code());
        }
    }
}

/// SIGKILL (never SIGTERM) and reap.
fn sigkill(launched: &mut Launched) {
    launched.child.kill().expect("SIGKILL child");
    launched.child.wait().expect("reap killed child");
}

/// Run one incarnation to completion; panics on nonzero exit. Returns the
/// captured stdout lines.
fn run_to_completion(yaml: &Path, dir: &Path) -> Vec<String> {
    let mut launched = launch(yaml, dir, &[]);
    match wait_marker_or_exit(&mut launched) {
        WaitOutcome::CleanExit => {}
        WaitOutcome::Marker => panic!("unexpected chaos marker in a clean completion run"),
        WaitOutcome::FailedExit(code) => panic!(
            "clean relaunch failed with exit {code:?}; tail: {:?}",
            launched
                .lines
                .lock()
                .expect("lines")
                .iter()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
        ),
    }
    let lines = launched.lines.lock().expect("lines mutex").clone();
    lines
}

/// End-state fingerprints of a state-dir (after a final clean exit).
struct Fingerprints {
    tree: [u8; 32],
    archive: [u8; 32],
    goal_nodes: Vec<orch_core::types::NodeId>,
    status: ExperimentState,
}

fn fingerprints(dir: &Path, cfg_hash: &[u8; 32]) -> Fingerprints {
    let (services, _stats) = PersistentServices::reload(dir).expect("offline reload");
    let store = services.store.inner();
    assert_no_stranded_frontier(store, EXPERIMENT_ID);
    let checkpoint_bytes = store
        .get_metadata(GetMetadataRequest {
            key: MetadataKey::checkpoint(EXPERIMENT_ID),
        })
        .expect("persisted checkpoint present")
        .value;
    let checkpoint =
        decode_checkpoint(&checkpoint_bytes, EXPERIMENT_ID, cfg_hash).expect("checkpoint decodes");
    Fingerprints {
        tree: store_tree_hash(store, EXPERIMENT_ID),
        archive: scorer_archive_fingerprint(services.scorer.inner(), EXPERIMENT_ID),
        goal_nodes: checkpoint.goal_nodes,
        status: checkpoint.status,
    }
}

/// Exit-0 must always pair with persisted GoalReached — in every round of
/// every class (W2.4).
fn assert_matches_control(dir: &Path, cfg_hash: &[u8; 32], control: &Fingerprints, label: &str) {
    let state = fingerprints(dir, cfg_hash);
    assert_eq!(
        state.status,
        ExperimentState::GoalReached,
        "{label}: persisted status"
    );
    assert_eq!(state.tree, control.tree, "{label}: tree hash diverged");
    assert_eq!(state.archive, control.archive, "{label}: archive diverged");
    assert_eq!(
        state.goal_nodes, control.goal_nodes,
        "{label}: goal nodes diverged"
    );
}

struct SeedContext {
    seed: u64,
    yaml: PathBuf,
    cfg_hash: [u8; 32],
    control: Fingerprints,
    root: PathBuf,
}

fn seed_context(base: &Path, seed: u64) -> SeedContext {
    let root = base.join(format!("seed-{seed:#x}"));
    std::fs::create_dir_all(&root).expect("seed dir");
    let yaml = root.join("experiment.yaml");
    let yaml_bytes = config_yaml(seed);
    std::fs::write(&yaml, &yaml_bytes).expect("write yaml");
    let sparse = wire_config_from_yaml(yaml_bytes.as_bytes()).expect("harness yaml parses");
    let cfg_hash = config_hash(&effective_config(&sparse));

    let control_dir = root.join("control");
    run_to_completion(&yaml, &control_dir);
    let control = fingerprints(&control_dir, &cfg_hash);
    assert_eq!(
        control.status,
        ExperimentState::GoalReached,
        "control run must reach the goal"
    );
    SeedContext {
        seed,
        yaml,
        cfg_hash,
        control,
        root,
    }
}

/// One forced round: launch with a hook env until the marker fires, SIGKILL,
/// relaunch clean to completion, compare against control. Returns the
/// relaunch stdout (for torn-write `truncated_bytes` grepping).
///
/// A clean exit before the marker means the hook never fired this
/// incarnation (point not reached at this nth): the run converged — retry
/// on a fresh dir with a varied nth, like Tier-1's CrashOnce ladder.
fn forced_round(
    context: &SeedContext,
    label: &str,
    hook: impl Fn(u32) -> (&'static str, String),
    kills: &AtomicU32,
) -> Vec<String> {
    for attempt in 0..8u32 {
        let dir = context.root.join(format!("{label}-a{attempt}"));
        let (key, value) = hook(attempt);
        let mut launched = launch(&context.yaml, &dir, &[(key, value)]);
        match wait_marker_or_exit(&mut launched) {
            WaitOutcome::Marker => {
                sigkill(&mut launched);
                kills.fetch_add(1, Ordering::Relaxed);
                let relaunch_lines = run_to_completion(&context.yaml, &dir);
                assert_matches_control(
                    &dir,
                    &context.cfg_hash,
                    &context.control,
                    &format!("seed {:#x} {label}", context.seed),
                );
                return relaunch_lines;
            }
            WaitOutcome::CleanExit => {
                // Converged without reaching the hook; still must equal the
                // control, then retry for a real kill.
                assert_matches_control(
                    &dir,
                    &context.cfg_hash,
                    &context.control,
                    &format!(
                        "seed {:#x} {label} (converged attempt {attempt})",
                        context.seed
                    ),
                );
            }
            WaitOutcome::FailedExit(code) => {
                panic!("{label}: hooked incarnation failed with exit {code:?}")
            }
        }
    }
    panic!(
        "seed {:#x} {label}: hook never fired in 8 attempts — lattice hole",
        context.seed
    );
}

fn random_round(context: &SeedContext, index: u32, kills: &AtomicU32) {
    let mut rng = Lcg(context.seed ^ (u64::from(index) << 32) ^ 0x7112);
    for attempt in 0..16u32 {
        let dir = context.root.join(format!("random-{index}-a{attempt}"));
        let mut launched = launch(&context.yaml, &dir, &[]);
        std::thread::sleep(Duration::from_millis(rng.next_ms(50, 800)));
        if launched.child.try_wait().expect("try_wait").is_some() {
            // Early clean exit: a redundant control run, not a kill. Retry.
            continue;
        }
        sigkill(&mut launched);
        kills.fetch_add(1, Ordering::Relaxed);
        run_to_completion(&context.yaml, &dir);
        assert_matches_control(
            &dir,
            &context.cfg_hash,
            &context.control,
            &format!("seed {:#x} random-{index}", context.seed),
        );
        return;
    }
    panic!("random round could not land a kill in 16 attempts");
}

/// Runs closures across a small worker pool; propagates the first panic.
fn run_parallel<'jobs>(jobs: Vec<Box<dyn FnOnce() + Send + 'jobs>>, workers: usize) {
    let jobs: Vec<Mutex<Option<Box<dyn FnOnce() + Send + 'jobs>>>> =
        jobs.into_iter().map(|job| Mutex::new(Some(job))).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers.max(1))
            .map(|_| {
                scope.spawn(|| loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(slot) = jobs.get(index) else { return };
                    let job = slot
                        .lock()
                        .expect("job mutex")
                        .take()
                        .expect("job taken once");
                    job();
                })
            })
            .collect();
        for handle in handles {
            if let Err(panic) = handle.join() {
                std::panic::resume_unwind(panic);
            }
        }
    });
}

#[test]
fn tier2_kill_matrix_resumes_bit_identically() {
    if !enabled() {
        return;
    }
    let base = tempfile::tempdir().expect("tempdir");
    let lattice_kills = AtomicU32::new(0);
    let torn_kills = AtomicU32::new(0);
    let random_kills = AtomicU32::new(0);
    let workers = parallelism();

    // Phase 1: every seed's uninterrupted control, in parallel.
    let contexts: Vec<SeedContext> = {
        let results: Vec<Mutex<Option<SeedContext>>> =
            seeds().iter().map(|_| Mutex::new(None)).collect();
        let jobs: Vec<Box<dyn FnOnce() + Send + '_>> = seeds()
            .into_iter()
            .zip(results.iter())
            .map(|(seed, slot)| {
                let base = base.path().to_path_buf();
                let job: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
                    *slot.lock().expect("slot") = Some(seed_context(&base, seed));
                });
                job
            })
            .collect();
        run_parallel(jobs, workers);
        results
            .into_iter()
            .map(|slot| slot.into_inner().expect("slot").expect("context built"))
            .collect()
    };

    // Phase 2: all kill rounds, in parallel across independent state-dirs.
    let mut jobs: Vec<Box<dyn FnOnce() + Send + '_>> = Vec::new();
    for context in &contexts {
        for point in lattice_points() {
            let context = &*context;
            let kills = &lattice_kills;
            jobs.push(Box::new(move || {
                let seed = context.seed as u32;
                forced_round(
                    context,
                    &format!("lattice-{}", point.as_str()),
                    move |attempt| {
                        (
                            "ORCH_CHAOS_HANG_AT",
                            format!("{}:{}", point.as_str(), 1 + (attempt + seed) % 3),
                        )
                    },
                    kills,
                );
            }));
        }
        for (kind, nth) in [("wal-append", 2u32), ("ckpt-put", 1u32)] {
            let context = &*context;
            let kills = &torn_kills;
            jobs.push(Box::new(move || {
                let relaunch = forced_round(
                    context,
                    &format!("torn-{kind}"),
                    move |_attempt| ("ORCH_SIM_TORN_AT", format!("{kind}:{nth}")),
                    kills,
                );
                // The torn prefix must actually exercise truncation on
                // reload (W2.4).
                let truncated = relaunch.iter().any(|line| {
                    line.starts_with("TIER2_SIM_RELOAD")
                        && !line.trim_end().ends_with("truncated_bytes=0")
                });
                assert!(
                    truncated,
                    "torn-{kind}: relaunch reload reported no truncated bytes: {:?}",
                    relaunch
                        .iter()
                        .filter(|line| line.starts_with("TIER2_SIM_RELOAD"))
                        .collect::<Vec<_>>()
                );
            }));
        }
        for index in 0..random_kills_per_seed() {
            let context = &*context;
            let kills = &random_kills;
            jobs.push(Box::new(move || random_round(context, index, kills)));
        }
    }
    run_parallel(jobs, workers);

    let lattice = lattice_kills.load(Ordering::Relaxed);
    let torn = torn_kills.load(Ordering::Relaxed);
    let random = random_kills.load(Ordering::Relaxed);
    let total = lattice + torn + random;
    let seeds_run = contexts.len() as u32;
    println!(
        "TIER2_SUMMARY seeds={seeds_run} kills={total} lattice={lattice} torn={torn} random={random}"
    );

    // Structural quota: one kill per forced round plus the random quota. A
    // future default change cannot silently sink the evidence bar (W2.4).
    let expected = seeds_run * (lattice_points().len() as u32 + 2 + random_kills_per_seed());
    assert!(
        total >= expected,
        "kill quota not met: {total} < {expected}"
    );
    if let Some(min) = std::env::var("TIER2_MIN_KILLS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    {
        assert!(total >= min, "evidence kill quota not met: {total} < {min}");
    }
}

/// The demonstrated negative (W2.5): a comparator that cannot detect a real
/// divergence proves nothing. `ORCH_SIM_BREAK=perturb-node` bumps one
/// journaled `create_node`'s progress_score during replay; the comparator
/// must fail — and the *unbroken* reload of the same pre-relaunch dir must
/// still converge, pinning the divergence on the mutation, not the kill.
#[test]
fn negative_control_detects_divergence() {
    if !enabled() {
        return;
    }
    let base = tempfile::tempdir().expect("tempdir");
    let context = seed_context(base.path(), 0x5EED);

    // Kill at BeforeCasPut:1 — 16 commits have happened, so >=1 committed
    // node exists for the perturbation to land on.
    let dir = context.root.join("negative");
    let mut launched = launch(
        &context.yaml,
        &dir,
        &[("ORCH_CHAOS_HANG_AT", "BeforeCasPut:1".to_owned())],
    );
    match wait_marker_or_exit(&mut launched) {
        WaitOutcome::Marker => sigkill(&mut launched),
        other => panic!(
            "BeforeCasPut hook must fire on a fresh run (got {})",
            match other {
                WaitOutcome::CleanExit => "clean exit",
                WaitOutcome::FailedExit(_) => "failed exit",
                WaitOutcome::Marker => unreachable!(),
            }
        ),
    }

    // Copy the pre-relaunch dir so the unbroken arm replays the same bytes.
    let pristine = context.root.join("negative-pristine");
    std::fs::create_dir_all(&pristine).expect("pristine dir");
    std::fs::copy(
        dir.join(orch_simstate::journal::JOURNAL_FILE),
        pristine.join(orch_simstate::journal::JOURNAL_FILE),
    )
    .expect("copy journal");

    // Broken arm: relaunch through the perturbed replay, run to completion
    // (exit may be 0 — divergence shows in state, not exit codes).
    let mut broken = launch(
        &context.yaml,
        &dir,
        &[("ORCH_SIM_BREAK", "perturb-node".to_owned())],
    );
    match wait_marker_or_exit(&mut broken) {
        WaitOutcome::CleanExit | WaitOutcome::FailedExit(_) => {}
        WaitOutcome::Marker => panic!("no chaos hook set on the broken relaunch"),
    }
    let state = fingerprints(&dir, &context.cfg_hash);
    let hash_diverged = state.tree != context.control.tree;
    let outcome_diverged = state.status != ExperimentState::GoalReached;
    println!(
        "TIER2_NEGATIVE mutation=perturb-node hash_diverged={hash_diverged} outcome_diverged={outcome_diverged}"
    );
    // The hash arm specifically must fire: a vacuous pass through the
    // outcome arm alone would mean the mutation was too weak (W2.5).
    assert!(
        hash_diverged,
        "perturb-node mutation failed to diverge the tree hash — comparator not demonstrated"
    );

    // Unbroken arm: same pre-relaunch journal, no mutation — converges.
    run_to_completion(&context.yaml, &pristine);
    assert_matches_control(
        &pristine,
        &context.cfg_hash,
        &context.control,
        "negative-control unbroken arm",
    );
}

/// Served-gRPC resume smoke (D-T4): one case, not a matrix — the runner
/// underneath is identical to the standalone path; only the entry point
/// differs.
#[test]
fn grpc_served_resume_survives_sigkill() {
    use orch_proto::orchestrator_v1 as wire;
    use wire::exploration_orchestrator_client::ExplorationOrchestratorClient;

    if !enabled() {
        return;
    }
    let base = tempfile::tempdir().expect("tempdir");
    let seed = 0x5EED;
    let yaml_bytes = config_yaml(seed);
    let sparse = wire_config_from_yaml(yaml_bytes.as_bytes()).expect("yaml parses");
    let cfg_hash = config_hash(&effective_config(&sparse));

    // Standalone control for the same seed/experiment-id (run_id =
    // experiment_id in both paths when the gRPC request leaves run_id
    // empty).
    let control_root = base.path().join("grpc-control");
    std::fs::create_dir_all(&control_root).expect("control root");
    let yaml_path = base.path().join("experiment.yaml");
    std::fs::write(&yaml_path, &yaml_bytes).expect("write yaml");
    run_to_completion(&yaml_path, &control_root);
    let control = fingerprints(&control_root, &cfg_hash);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut attempt = 0u32;
    'smoke: loop {
        attempt += 1;
        assert!(
            attempt <= 8,
            "gRPC smoke could not land a post-checkpoint kill"
        );
        let state_dir = base.path().join(format!("grpc-state-a{attempt}"));

        // Ephemeral ports: bind-then-release.
        let listen = {
            let socket = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            socket.local_addr().expect("addr")
        };
        let http = {
            let socket = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            socket.local_addr().expect("addr")
        };
        let mut serve = Command::new(bin());
        serve
            .arg("--simulate")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--listen")
            .arg(listen.to_string())
            .arg("--http")
            .arg(http.to_string())
            .env_remove("ORCH_CHAOS_HANG_AT")
            .env_remove("ORCH_SIM_TORN_AT")
            .env_remove("ORCH_SIM_BREAK")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut server = serve.spawn().expect("spawn serve");

        let killed = runtime.block_on(async {
            let endpoint = format!("http://{listen}");
            // Wait for the listener.
            let mut client = loop {
                match ExplorationOrchestratorClient::connect(endpoint.clone()).await {
                    Ok(client) => break client,
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            };
            let response = client
                .start_experiment(wire::StartExperimentRequest {
                    experiment_id: EXPERIMENT_ID.to_owned(),
                    config: Some(sparse.clone()),
                    resume_if_exists: false,
                    run_id: String::new(),
                })
                .await
                .expect("start experiment")
                .into_inner();
            assert_eq!(response.resumed_at_batch_seq, 0, "fresh start");

            // Poll past the first checkpoint boundary (every_commits = 16)
            // before killing, so resumed_at_batch_seq > 0 is not flaky.
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let status = client
                    .get_experiment_status(wire::GetExperimentStatusRequest {
                        experiment_id: EXPERIMENT_ID.to_owned(),
                    })
                    .await
                    .expect("status")
                    .into_inner();
                let batch_seq = status.stats.as_ref().map_or(0, |stats| stats.batch_seq);
                if wire::ExperimentState::try_from(status.state)
                    == Ok(wire::ExperimentState::GoalReached)
                {
                    // Finished before we killed: no resume to prove here.
                    return false;
                }
                if batch_seq >= 8 {
                    return true;
                }
            }
        });
        server.kill().expect("SIGKILL server");
        server.wait().expect("reap server");
        if !killed {
            continue 'smoke; // finished too fast; retry on a fresh dir
        }

        // Offline guard: the kill must have landed after a persisted
        // checkpoint, else retry (poll raced the checkpoint write).
        {
            let (services, _stats) =
                PersistentServices::reload(&state_dir).expect("offline reload");
            if services
                .store
                .inner()
                .get_metadata(GetMetadataRequest {
                    key: MetadataKey::checkpoint(EXPERIMENT_ID),
                })
                .is_err()
            {
                continue 'smoke;
            }
        }

        // Relaunch against the same dir; served resume must pick up the
        // checkpoint.
        let listen2 = {
            let socket = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            socket.local_addr().expect("addr")
        };
        let http2 = {
            let socket = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            socket.local_addr().expect("addr")
        };
        let mut serve2 = Command::new(bin());
        serve2
            .arg("--simulate")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--listen")
            .arg(listen2.to_string())
            .arg("--http")
            .arg(http2.to_string())
            .env_remove("ORCH_CHAOS_HANG_AT")
            .env_remove("ORCH_SIM_TORN_AT")
            .env_remove("ORCH_SIM_BREAK")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut server2 = serve2.spawn().expect("spawn relaunch");

        runtime.block_on(async {
            let endpoint = format!("http://{listen2}");
            let mut client = loop {
                match ExplorationOrchestratorClient::connect(endpoint.clone()).await {
                    Ok(client) => break client,
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            };
            let resumed = client
                .start_experiment(wire::StartExperimentRequest {
                    experiment_id: EXPERIMENT_ID.to_owned(),
                    config: Some(sparse.clone()),
                    resume_if_exists: true,
                    run_id: String::new(),
                })
                .await
                .expect("resume experiment")
                .into_inner();
            assert!(
                resumed.resumed_at_batch_seq > 0,
                "served resume must report a checkpointed batch seq"
            );
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let status = client
                    .get_experiment_status(wire::GetExperimentStatusRequest {
                        experiment_id: EXPERIMENT_ID.to_owned(),
                    })
                    .await
                    .expect("status")
                    .into_inner();
                if wire::ExperimentState::try_from(status.state)
                    == Ok(wire::ExperimentState::GoalReached)
                {
                    break;
                }
            }
        });
        // Orderly stop: SIGTERM drains every experiment to its final
        // checkpoint before the listener exits (the product shutdown path;
        // the chaos kills above are the SIGKILL cases).
        let terminated = Command::new("kill")
            .arg("-TERM")
            .arg(server2.id().to_string())
            .status()
            .expect("send SIGTERM")
            .success();
        assert!(terminated, "SIGTERM delivery failed");
        server2.wait().expect("reap relaunch");

        assert_matches_control(&state_dir, &cfg_hash, &control, "gRPC served resume");
        println!("TIER2_GRPC_SMOKE kills=1 resumed=1");
        break;
    }
}
