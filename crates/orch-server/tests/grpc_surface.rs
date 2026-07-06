//! M4 accept bar: all six RPCs against a served in-process tonic endpoint
//! wired to fakes — validation error listing, lifecycle, status, and
//! StreamProgress edges including GoalReached.

mod support;

use orch_fakes::grid::GridWorld;
use orch_proto::orchestrator_v1 as wire;
use orch_server::{events::SharedSink, service::OrchestratorService};
use support::{grid_config, sources, FakeWorld};
use tonic::transport::Server;

async fn serve() -> (
    wire::exploration_orchestrator_client::ExplorationOrchestratorClient<tonic::transport::Channel>,
    tokio::task::JoinHandle<()>,
) {
    let world = FakeWorld::new(GridWorld::three_room());
    let service = OrchestratorService::new(
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        SharedSink::new(world.observatory()),
        std::sync::Arc::new(|experiment_id: &str| {
            let mut sources = sources();
            sources.synth_config_yaml = format!(
                "version: 1\nkind: experiment_config\nexperiment_id: {experiment_id}\nmodel: pad\nbutton_alphabet: console16-12btn-v1\ngenerator_mix:\n  weighted_random: 1\n  macro: 0\n  mutation: 0\n  policy: 0\n"
            )
            .into_bytes();
            sources
        }),
        "orchestratord-test",
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(
                wire::exploration_orchestrator_server::ExplorationOrchestratorServer::new(service),
            )
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .expect("serve");
    });
    let client = wire::exploration_orchestrator_client::ExplorationOrchestratorClient::connect(
        format!("http://{addr}"),
    )
    .await
    .expect("connect");
    (client, server)
}

fn wire_config(seed: u64, on_goal_continue: bool) -> wire::ExperimentConfig {
    let mut config = orch_server::config::to_wire(&grid_config(seed));
    if on_goal_continue {
        config.on_goal = wire::OnGoal::Continue as i32;
        if let Some(budgets) = config.budgets.as_mut() {
            budgets.max_expansions = 1_000_000;
        }
    }
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_six_rpcs_work_end_to_end() {
    let (mut client, server) = serve().await;

    // INVALID_ARGUMENT lists every bad field.
    let mut bad = wire_config(1, false);
    bad.workload_image_ref = String::new();
    if let Some(selection) = bad.selection.as_mut() {
        selection.temperature = -1.0;
    }
    if let Some(plateau) = bad.plateau.as_mut() {
        plateau.window_n = 1;
    }
    let error = client
        .start_experiment(wire::StartExperimentRequest {
            experiment_id: "bad".to_owned(),
            config: Some(bad),
            resume_if_exists: false,
            run_id: String::new(),
        })
        .await
        .expect_err("invalid config");
    assert_eq!(error.code(), tonic::Code::InvalidArgument);
    let message = error.message();
    assert!(
        message.contains("workload_image_ref")
            && message.contains("selection.temperature")
            && message.contains("plateau.window_n"),
        "every bad field listed: {message}"
    );

    // StartExperiment (long-running so Pause can land).
    let started = client
        .start_experiment(wire::StartExperimentRequest {
            experiment_id: "exp-grpc".to_owned(),
            config: Some(wire_config(0x5EED, true)),
            resume_if_exists: false,
            run_id: String::new(),
        })
        .await
        .expect("start")
        .into_inner();
    assert_eq!(started.state, wire::ExperimentState::Running as i32);
    assert_eq!(started.resumed_at_batch_seq, 0);

    // Duplicate start without resume_if_exists.
    let duplicate = client
        .start_experiment(wire::StartExperimentRequest {
            experiment_id: "exp-grpc".to_owned(),
            config: Some(wire_config(0x5EED, true)),
            resume_if_exists: false,
            run_id: String::new(),
        })
        .await
        .expect_err("duplicate");
    assert_eq!(duplicate.code(), tonic::Code::AlreadyExists);

    // GetExperimentStatus.
    let status = client
        .get_experiment_status(wire::GetExperimentStatusRequest {
            experiment_id: "exp-grpc".to_owned(),
        })
        .await
        .expect("status")
        .into_inner();
    assert_eq!(status.experiment_id, "exp-grpc");

    // Pause -> durable checkpoint seq; Resume -> running.
    let paused = client
        .pause_experiment(wire::PauseExperimentRequest {
            experiment_id: "exp-grpc".to_owned(),
        })
        .await
        .expect("pause")
        .into_inner();
    assert_eq!(paused.state, wire::ExperimentState::Paused as i32);
    let resumed = client
        .resume_experiment(wire::ResumeExperimentRequest {
            experiment_id: "exp-grpc".to_owned(),
        })
        .await
        .expect("resume")
        .into_inner();
    assert_eq!(resumed.state, wire::ExperimentState::Running as i32);

    // StreamProgress: heartbeats plus the GoalReached edge (on_goal is
    // CONTINUE, so the run keeps going after the goal).
    let mut stream = client
        .stream_progress(wire::StreamProgressRequest {
            experiment_id: "exp-grpc".to_owned(),
            min_interval_ms: 50,
        })
        .await
        .expect("stream")
        .into_inner();
    let mut saw_goal_edge = false;
    for _ in 0..600 {
        let Some(event) = stream.message().await.expect("stream item") else {
            break;
        };
        assert!(event.status.is_some());
        if let Some(wire::progress_event::Edge::Goal(goal)) = event.edge {
            assert!(goal.node_id > 0);
            assert!(!goal.snapshot_ref.is_empty());
            assert!(goal.score > 0.0);
            saw_goal_edge = true;
            break;
        }
    }
    assert!(saw_goal_edge, "GoalReached edge must arrive on the stream");

    // Stop: terminal state + final stats.
    let stopped = client
        .stop_experiment(wire::StopExperimentRequest {
            experiment_id: "exp-grpc".to_owned(),
            abandon_inflight: false,
        })
        .await
        .expect("stop")
        .into_inner();
    let final_stats = stopped.final_stats.expect("final stats");
    assert!(final_stats.expansions > 0);
    assert!(final_stats.nodes_committed > 1);
    assert!(
        stopped.state == wire::ExperimentState::Stopped as i32
            || stopped.state == wire::ExperimentState::GoalReached as i32
    );

    // Unknown experiment surfaces NOT_FOUND.
    let missing = client
        .get_experiment_status(wire::GetExperimentStatusRequest {
            experiment_id: "nope".to_owned(),
        })
        .await
        .expect_err("missing");
    assert_eq!(missing.code(), tonic::Code::NotFound);

    server.abort();
}
