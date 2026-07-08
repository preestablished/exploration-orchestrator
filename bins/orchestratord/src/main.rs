//! `orchestratord`: the exploration orchestrator daemon (plan W4.6/W4.7).
//!
//! Modes:
//! - `--simulate`: serve the gRPC surface over the in-process fake world
//!   (zero platform dependencies).
//! - `--experiment <file.yaml> [--experiment-id <id>]`: standalone mode —
//!   load the YAML `ExperimentConfig` (same defaults + validation path as
//!   gRPC), run it against the fake world to completion, exit. In this
//!   mode `run_id = experiment_id`.
//!
//! Also serves `/healthz` and `/metrics` over plain HTTP. SIGTERM drains:
//! every running experiment is stopped (final checkpoint) before exit;
//! SIGKILL is the chaos case the resume path covers.
//!
//! `--state-dir <dir>` (both modes): the fake world persists through a
//! crash-consistent journal (`orch-simstate`); an existing journal is
//! reloaded, otherwise one is created. Absent flag = journal-less
//! passthrough, zero behavior change.
//!
//! Test-only chaos hooks (the Tier-2 harness's, plan D-T3 — not for
//! production use):
//! - `ORCH_CHAOS_HANG_AT=<CrashPoint>:<nth>` (`--experiment` mode only):
//!   on the nth arrival at the named crash point, print
//!   `TIER2_CHAOS_HANG point=<point>` and park so the harness can land a
//!   real SIGKILL there. The gRPC-served path keeps no crash policy (D-T4).
//! - `ORCH_SIM_TORN_AT=<wal-append|ckpt-put>:<nth>` (read by
//!   `orch-simstate`): torn journal-frame prefix + marker + park.
//! - `ORCH_SIM_BREAK=perturb-node|drop-scorer-replay`: reload through a
//!   deliberately divergent replay (the negative control, plan W2.5).

mod simulate;

use std::net::SocketAddr;
use std::process::ExitCode;

use orch_proto::orchestrator_v1 as wire;
use orch_server::config::{config_hash, effective_config, wire_config_from_yaml};
use orch_server::events::SharedSink;
use orch_server::experiment::{CrashPoint, CrashPolicy, ExperimentRunner, RunnerConfig};
use orch_server::service::OrchestratorService;
use orch_simstate::world::BreakMode;
use tonic::transport::Server;
use tracing::{error, info};

#[derive(Debug, Default)]
struct Args {
    simulate: bool,
    experiment: Option<String>,
    experiment_id: Option<String>,
    listen: Option<SocketAddr>,
    http: Option<SocketAddr>,
    state_dir: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--simulate" => args.simulate = true,
            "--experiment" => {
                args.experiment = Some(iter.next().ok_or("--experiment needs a file path")?);
            }
            "--experiment-id" => {
                args.experiment_id = Some(iter.next().ok_or("--experiment-id needs a value")?);
            }
            "--listen" => {
                let value = iter.next().ok_or("--listen needs an address")?;
                args.listen = Some(
                    value
                        .parse()
                        .map_err(|error| format!("--listen: {error}"))?,
                );
            }
            "--http" => {
                let value = iter.next().ok_or("--http needs an address")?;
                args.http = Some(value.parse().map_err(|error| format!("--http: {error}"))?);
            }
            "--state-dir" => {
                args.state_dir = Some(iter.next().ok_or("--state-dir needs a directory")?);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if !args.simulate && args.experiment.is_none() {
        return Err("one of --simulate or --experiment <file.yaml> is required".to_owned());
    }
    Ok(args)
}

/// `ORCH_SIM_BREAK` (test-only): reload through a deliberately divergent
/// replay for the negative control.
fn break_mode_from_env() -> Result<Option<BreakMode>, String> {
    match std::env::var("ORCH_SIM_BREAK") {
        Err(_) => Ok(None),
        Ok(value) => match value.as_str() {
            "perturb-node" => Ok(Some(BreakMode::PerturbNode)),
            "drop-scorer-replay" => Ok(Some(BreakMode::DropScorerReplay)),
            other => Err(format!("ORCH_SIM_BREAK: unknown mode '{other}'")),
        },
    }
}

/// The Tier-2 lattice hook: parks at the named crash point so the harness
/// can land a real SIGKILL there (plan D-T3). Never returns `true` — the
/// park *is* the crash site.
struct HangAt {
    point: CrashPoint,
    nth: u32,
    seen: u32,
}

impl CrashPolicy for HangAt {
    fn should_crash(&mut self, point: CrashPoint) -> bool {
        if point != self.point {
            return false;
        }
        self.seen += 1;
        if self.seen < self.nth {
            return false;
        }
        println!("TIER2_CHAOS_HANG point={}", point.as_str());
        use std::io::Write as _;
        std::io::stdout().flush().expect("stdout flush");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
}

/// `ORCH_CHAOS_HANG_AT=<CrashPoint>:<nth>` (test-only, `--experiment` mode).
fn hang_policy_from_env() -> Result<Option<Box<dyn CrashPolicy>>, String> {
    let Ok(value) = std::env::var("ORCH_CHAOS_HANG_AT") else {
        return Ok(None);
    };
    let (point, nth) = value
        .split_once(':')
        .ok_or_else(|| format!("ORCH_CHAOS_HANG_AT: expected <point>:<nth>, got '{value}'"))?;
    let point: CrashPoint = point.parse()?;
    let nth: u32 = nth
        .parse()
        .map_err(|error| format!("ORCH_CHAOS_HANG_AT: bad nth: {error}"))?;
    if nth == 0 {
        return Err("ORCH_CHAOS_HANG_AT: nth must be >= 1".to_owned());
    }
    Ok(Some(Box::new(HangAt {
        point,
        nth,
        seen: 0,
    })))
}

fn producer_id() -> String {
    let startup_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    format!("orchestratord-{startup_unix}")
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("orchestratord: {message}");
            return ExitCode::from(2);
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = if let Some(path) = args.experiment.clone() {
        runtime.block_on(run_standalone(&args, &path))
    } else {
        runtime.block_on(serve_simulate(&args))
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            error!(error = %message, "orchestratord failed");
            ExitCode::FAILURE
        }
    }
}

async fn run_standalone(args: &Args, path: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {path}: {error}"))?;
    let sparse = wire_config_from_yaml(&bytes)?;
    let effective = effective_config(&sparse);
    let violations = effective.validate_all();
    if !violations.is_empty() {
        let details: Vec<String> = violations.iter().map(ToString::to_string).collect();
        return Err(format!("config validation failed: {}", details.join("; ")));
    }
    let experiment_id = args
        .experiment_id
        .clone()
        .or_else(|| {
            std::path::Path::new(path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .ok_or("could not derive an experiment id; pass --experiment-id")?;

    let world = simulate::world(args.state_dir.as_deref(), break_mode_from_env()?)?;
    let sources = simulate::sources(&experiment_id);
    let runner_config = RunnerConfig {
        run_id: experiment_id.clone(), // standalone: run_id = experiment_id
        experiment_id: experiment_id.clone(),
        producer_id: producer_id(),
        config_hash: config_hash(&effective),
        config: effective,
    };
    info!(experiment_id = %experiment_id, "starting standalone experiment");
    let (runner, _handle, _mode) = ExperimentRunner::start(
        runner_config,
        sources,
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        world.observatory(),
        hang_policy_from_env()?,
    )
    .await
    .map_err(|error| error.to_string())?;
    let outcome = runner.run().await.map_err(|error| error.to_string())?;
    info!(
        state = ?outcome.state,
        expansions = outcome.expansions,
        nodes = outcome.nodes,
        best_score = ?outcome.best_score,
        goal_nodes = ?outcome.goal_nodes,
        "experiment finished"
    );
    if outcome.state == orch_checkpoint::ExperimentState::Failed {
        return Err(outcome
            .failure_reason
            .unwrap_or_else(|| "experiment failed".to_owned()));
    }
    Ok(())
}

async fn serve_simulate(args: &Args) -> Result<(), String> {
    let listen = args
        .listen
        .unwrap_or_else(|| "127.0.0.1:7130".parse().expect("static addr"));
    let http = args
        .http
        .unwrap_or_else(|| "127.0.0.1:7131".parse().expect("static addr"));

    // The served path keeps no crash policy (plan D-T4); it still honors
    // --state-dir so a relaunch can resume against the same journal.
    let world = simulate::world(args.state_dir.as_deref(), break_mode_from_env()?)?;
    let service = std::sync::Arc::new(OrchestratorService::new(
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        SharedSink::new(world.observatory()),
        std::sync::Arc::new(|experiment_id: &str| simulate::sources(experiment_id)),
        producer_id(),
    ));

    tokio::spawn(serve_http(http));

    info!(%listen, %http, "orchestratord --simulate serving");
    // SIGTERM/SIGINT drain: stop every running experiment and wait for its
    // final checkpoint before the listener shuts down (review finding: the
    // signal used to only stop the listener, aborting runner tasks with no
    // final checkpoint).
    let drain_service = std::sync::Arc::clone(&service);
    let shutdown = async move {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("sigterm handler");
        tokio::select! {
            _ = sigterm.recv() => info!("SIGTERM: draining experiments"),
            result = tokio::signal::ctrl_c() => {
                let _ = result;
                info!("SIGINT: draining experiments");
            }
        }
        drain_service.shutdown().await;
        info!("all experiments checkpointed; shutting down");
    };
    Server::builder()
        .add_service(
            wire::exploration_orchestrator_server::ExplorationOrchestratorServer::from_arc(service),
        )
        .serve_with_shutdown(listen, shutdown)
        .await
        .map_err(|error| error.to_string())
}

/// Minimal /healthz + /metrics responder (full Prometheus surface is M5).
async fn serve_http(addr: SocketAddr) {
    let Ok(listener) = tokio::net::TcpListener::bind(addr).await else {
        error!(%addr, "http listener failed to bind");
        return;
    };
    loop {
        let Ok((mut stream, _peer)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            // Read until the end of the request headers (bounded at 8 KiB)
            // so split TCP reads are not mis-parsed (review finding).
            let mut buffer = Vec::with_capacity(1024);
            let mut chunk = [0u8; 1024];
            loop {
                let Ok(read) = stream.read(&mut chunk).await else {
                    return;
                };
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.windows(4).any(|window| window == b"\r\n\r\n") || buffer.len() >= 8 * 1024
                {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&buffer);
            let (status, body) = if request.starts_with("GET /healthz") {
                ("200 OK", "ok\n".to_owned())
            } else if request.starts_with("GET /metrics") {
                (
                    "200 OK",
                    "# TYPE orchestratord_up gauge\norchestratord_up 1\n".to_owned(),
                )
            } else {
                ("404 Not Found", "not found\n".to_owned())
            };
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\ncontent-type: text/plain\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
    }
}
