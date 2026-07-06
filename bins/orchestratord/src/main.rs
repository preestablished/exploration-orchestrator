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

mod simulate;

use std::net::SocketAddr;
use std::process::ExitCode;

use orch_proto::orchestrator_v1 as wire;
use orch_server::config::{config_hash, effective_config, wire_config_from_yaml};
use orch_server::events::SharedSink;
use orch_server::experiment::{ExperimentRunner, RunnerConfig};
use orch_server::service::OrchestratorService;
use tonic::transport::Server;
use tracing::{error, info};

#[derive(Debug, Default)]
struct Args {
    simulate: bool,
    experiment: Option<String>,
    experiment_id: Option<String>,
    listen: Option<SocketAddr>,
    http: Option<SocketAddr>,
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
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if !args.simulate && args.experiment.is_none() {
        return Err("one of --simulate or --experiment <file.yaml> is required".to_owned());
    }
    Ok(args)
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

    let world = simulate::SimulatedWorld::new();
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
        None,
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

    let world = simulate::SimulatedWorld::new();
    let service = OrchestratorService::new(
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        SharedSink::new(world.observatory()),
        std::sync::Arc::new(|experiment_id: &str| simulate::sources(experiment_id)),
        producer_id(),
    );

    tokio::spawn(serve_http(http));

    info!(%listen, %http, "orchestratord --simulate serving");
    let shutdown = async {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("sigterm handler");
        tokio::select! {
            _ = sigterm.recv() => info!("SIGTERM: draining"),
            result = tokio::signal::ctrl_c() => {
                let _ = result;
                info!("SIGINT: draining");
            }
        }
    };
    Server::builder()
        .add_service(
            wire::exploration_orchestrator_server::ExplorationOrchestratorServer::new(service),
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
            let mut buffer = [0u8; 1024];
            let Ok(read) = stream.read(&mut buffer).await else {
                return;
            };
            let request = String::from_utf8_lossy(&buffer[..read]);
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
