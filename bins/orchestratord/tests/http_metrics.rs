use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    time::Duration,
};

use orch_core::types::{ExperimentConfig, OnGoal, SchedMode};
use orch_proto::orchestrator_v1 as wire;
use orch_server::{
    config::to_wire,
    metrics::{rendered_families, ORCH_EXPANSIONS_TOTAL, ORCH_NODES_TOTAL},
};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind free port");
    listener.local_addr().expect("local addr")
}

fn grid_config() -> wire::ExperimentConfig {
    let mut config = ExperimentConfig::new(
        0x5EED,
        "workload://grid",
        "featmap://grid",
        "score://grid",
        "synth://grid",
    );
    config.burst.k_per_expansion = 4;
    config.burst.base_burst_len_frames = 3;
    config.burst.max_burst_len_frames = 12;
    config.budgets.max_expansions = 64;
    config.budgets.max_wall_clock_s = 86_400;
    config.budgets.max_nodes = 0;
    config.checkpoint.every_commits = 16;
    config.checkpoint.every_seconds = 3_600;
    config.selection.temperature = 8.0;
    config.selection.max_visits_per_node = 256;
    config.selection.exhaust_after_dup_expansions = 32;
    config.scheduling.mode = SchedMode::Deterministic;
    config.scheduling.max_inflight_batches = 1;
    config.on_goal = OnGoal::Continue;
    config.validate().expect("test config validates");
    to_wire(&config)
}

fn http_get(addr: SocketAddr, path: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(250))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    write!(stream, "GET {path} HTTP/1.1\r\nhost: {addr}\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

async fn wait_for_http(addr: SocketAddr) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if http_get(addr, "/healthz")
            .map(|response| response.contains("200 OK"))
            .unwrap_or(false)
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "http listener did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_grpc(
    addr: SocketAddr,
) -> wire::exploration_orchestrator_client::ExplorationOrchestratorClient<tonic::transport::Channel>
{
    let endpoint = format!("http://{addr}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match wire::exploration_orchestrator_client::ExplorationOrchestratorClient::connect(
            endpoint.clone(),
        )
        .await
        {
            Ok(client) => return client,
            Err(_) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "grpc listener did not become ready"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simulate_metrics_endpoint_serves_catalog_and_live_counters() {
    let listen = free_addr();
    let http = free_addr();
    let child = Command::new(env!("CARGO_BIN_EXE_orchestratord"))
        .args([
            "--simulate",
            "--listen",
            &listen.to_string(),
            "--http",
            &http.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn orchestratord");
    let _guard = ChildGuard(child);

    wait_for_http(http).await;
    let mut client = wait_for_grpc(listen).await;
    client
        .start_experiment(wire::StartExperimentRequest {
            experiment_id: "metrics-smoke".to_owned(),
            config: Some(grid_config()),
            resume_if_exists: false,
            run_id: String::new(),
        })
        .await
        .expect("start experiment");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let response = http_get(http, "/metrics").expect("scrape metrics");
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .unwrap_or(&response);
        let families = rendered_families(body);
        assert!(families.contains(ORCH_EXPANSIONS_TOTAL));
        assert!(families.contains(ORCH_NODES_TOTAL));
        if body.contains("orch_expansions_total 0")
            || body.contains("orch_nodes_total{verdict=\"kept\"} 0")
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "metrics counters stayed zero:\n{body}"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        return;
    }
}
