use futures_util::future::join_all;
use serde::Serialize;
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;
use tempfile::TempDir;
use termirust_relay_protocol::{
    RELAY_LOOPBACK_ORIGIN, RelayAdmissionCredential, RelayEndpointRole, RelayRouteId,
    RelayRouteRegistration,
};
use termirust_relay_server::harness::SyntheticRelayClient;
use termirust_relay_server::{
    RelayMetadataStore, RelayServer, RelayServerConfig, RelayServerLimits,
};

#[derive(Serialize)]
struct BenchmarkReport {
    schema: &'static str,
    schema_version: u32,
    generated_at: String,
    loopback_only: bool,
    machine: Machine,
    runs_per_scenario: usize,
    scenarios: Vec<Scenario>,
}

#[derive(Serialize)]
struct Machine {
    os: String,
    arch: String,
    hardware: String,
    toolchain: String,
    build_profile: &'static str,
}

#[derive(Serialize)]
struct Scenario {
    pairs: usize,
    runs: Vec<RawRun>,
    connect_p50_micros: u64,
    connect_p95_micros: u64,
    connect_p99_micros: u64,
    round_trip_p50_micros: u64,
    round_trip_p95_micros: u64,
    round_trip_p99_micros: u64,
    throughput_p50_bytes_per_second: u64,
    max_rss_bytes: u64,
    max_queue_drops: u64,
    real_tcp_sockets: usize,
    persistent_ciphertext_bytes: u64,
    per_route_log_bytes: u64,
}

#[derive(Clone, Serialize)]
struct RawRun {
    connect_micros: u64,
    round_trip_micros: u64,
    ciphertext_bytes: u64,
    throughput_bytes_per_second: u64,
    max_rss_bytes: u64,
    queue_drops: u64,
    metadata_store_bytes: u64,
}

#[tokio::main]
async fn main() {
    let (pairs, runs, output) = parse_args();
    let mut scenarios = Vec::new();
    for pair_count in pairs {
        let mut raw = Vec::with_capacity(runs);
        for _ in 0..runs {
            raw.push(run_once(pair_count).await);
        }
        scenarios.push(summarize(pair_count, raw));
    }
    let report = BenchmarkReport {
        schema: "termirust-relay-core-benchmark",
        schema_version: 1,
        generated_at: "2026-08-29".to_owned(),
        loopback_only: true,
        machine: Machine {
            os: env::consts::OS.to_owned(),
            arch: env::consts::ARCH.to_owned(),
            hardware: command_output("uname", &["-a"]),
            toolchain: command_output("rustc", &["--version"]),
            build_profile: "workspace dev (opt-level=1; dependencies opt-level=2)",
        },
        runs_per_scenario: runs,
        scenarios,
    };
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).expect("create benchmark output directory");
    }
    fs::write(
        &output,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).expect("serialize benchmark")
        ),
    )
    .expect("write benchmark report");
}

async fn run_once(pairs: usize) -> RawRun {
    let temp = TempDir::new().expect("temporary benchmark state");
    let state_path = temp.path().join("relay-state-v1.json");
    let registrations: Vec<_> = (0..pairs)
        .map(|index| fixture_registration(index).0)
        .collect();
    let store = RelayMetadataStore::acquire(&state_path).expect("acquire metadata store");
    store.commit(&registrations).expect("seed metadata store");
    drop(store);
    let metadata_store_bytes = fs::metadata(&state_path).expect("metadata size").len();
    let server = RelayServer::start(RelayServerConfig {
        bind: SocketAddr::from(([127, 0, 0, 1], 0)),
        state_path,
        allowed_origin: RELAY_LOOPBACK_ORIGIN.to_owned(),
        limits: RelayServerLimits::default(),
    })
    .await
    .expect("start relay");
    let url = server.websocket_url();

    let connect_started = Instant::now();
    let mut connected = Vec::with_capacity(pairs);
    for index in 0..pairs {
        let (registration, host_credential, controller_credential) = fixture_registration(index);
        let host = SyntheticRelayClient::connect(
            &url,
            registration.route_id,
            RelayEndpointRole::Host,
            &host_credential,
        )
        .await
        .expect("connect host");
        let controller = SyntheticRelayClient::connect(
            &url,
            registration.route_id,
            RelayEndpointRole::Controller,
            &controller_credential,
        )
        .await
        .expect("connect controller");
        connected.push((host, controller));
    }
    let connect_micros = elapsed_micros(connect_started);

    let round_trip_started = Instant::now();
    let completed = join_all(
        connected
            .into_iter()
            .map(|(mut host, mut controller)| async move {
                host.send_ciphertext(vec![0xA5; 1_024])
                    .await
                    .expect("send benchmark ciphertext");
                let received = controller
                    .receive_envelope()
                    .await
                    .expect("receive benchmark ciphertext");
                assert_eq!(received.ciphertext().len(), 1_024);
                (host, controller)
            }),
    )
    .await;
    let round_trip_micros = elapsed_micros(round_trip_started);
    drop(completed);
    let diagnostics = server.diagnostics().await;
    let ciphertext_bytes = (pairs as u64) * 1_024;
    let throughput_bytes_per_second = ciphertext_bytes
        .saturating_mul(1_000_000)
        .checked_div(round_trip_micros.max(1))
        .unwrap_or(0);
    server.shutdown().await.expect("shutdown relay");
    RawRun {
        connect_micros,
        round_trip_micros,
        ciphertext_bytes,
        throughput_bytes_per_second,
        max_rss_bytes: max_rss_bytes(),
        queue_drops: diagnostics.dropped_messages,
        metadata_store_bytes,
    }
}

fn summarize(pairs: usize, runs: Vec<RawRun>) -> Scenario {
    let connect: Vec<_> = runs.iter().map(|run| run.connect_micros).collect();
    let round_trip: Vec<_> = runs.iter().map(|run| run.round_trip_micros).collect();
    let throughput: Vec<_> = runs
        .iter()
        .map(|run| run.throughput_bytes_per_second)
        .collect();
    Scenario {
        pairs,
        connect_p50_micros: percentile(&connect, 50),
        connect_p95_micros: percentile(&connect, 95),
        connect_p99_micros: percentile(&connect, 99),
        round_trip_p50_micros: percentile(&round_trip, 50),
        round_trip_p95_micros: percentile(&round_trip, 95),
        round_trip_p99_micros: percentile(&round_trip, 99),
        throughput_p50_bytes_per_second: percentile(&throughput, 50),
        max_rss_bytes: runs.iter().map(|run| run.max_rss_bytes).max().unwrap_or(0),
        max_queue_drops: runs.iter().map(|run| run.queue_drops).max().unwrap_or(0),
        real_tcp_sockets: pairs * 2,
        persistent_ciphertext_bytes: 0,
        per_route_log_bytes: 0,
        runs,
    }
}

fn fixture_registration(
    index: usize,
) -> (
    RelayRouteRegistration,
    RelayAdmissionCredential,
    RelayAdmissionCredential,
) {
    let mut route = [0xA5; 32];
    route[..8].copy_from_slice(&(index as u64).to_be_bytes());
    let mut host_secret = [0x5A; 32];
    host_secret[..8].copy_from_slice(&(index as u64).to_be_bytes());
    let mut controller_secret = [0x3C; 32];
    controller_secret[..8].copy_from_slice(&(index as u64).to_be_bytes());
    let host = RelayAdmissionCredential::from_fixture_bytes(host_secret);
    let controller = RelayAdmissionCredential::from_fixture_bytes(controller_secret);
    (
        RelayRouteRegistration::new(RelayRouteId(route), &host, &controller),
        host,
        controller,
    )
}

fn parse_args() -> (Vec<usize>, usize, PathBuf) {
    let mut pairs = None;
    let mut runs = None;
    let mut output = None;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--pairs" => {
                pairs = arguments.next().map(|value| {
                    value
                        .split(',')
                        .map(|part| part.parse::<usize>().expect("numeric pair count"))
                        .collect::<Vec<_>>()
                });
            }
            "--runs" => {
                runs = arguments
                    .next()
                    .map(|value| value.parse().expect("numeric runs"))
            }
            "--output" => output = arguments.next().map(PathBuf::from),
            _ => panic!("unknown relay benchmark argument: {argument}"),
        }
    }
    let pairs = pairs.expect("--pairs is required");
    assert!(pairs.iter().all(|pairs| matches!(pairs, 1 | 10 | 100)));
    let runs = runs.expect("--runs is required");
    assert!((10..=100).contains(&runs));
    (pairs, runs, output.expect("--output is required"))
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    let mut values = values.to_vec();
    values.sort_unstable();
    let index = ((values.len() - 1) * percentile).div_ceil(100);
    values[index]
}

fn elapsed_micros(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn command_output(command: &str, arguments: &[&str]) -> String {
    Command::new(command)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".to_owned())
}

#[cfg(unix)]
fn max_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the supplied rusage on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return 0;
    }
    // SAFETY: the successful call above initialized usage.
    let usage = unsafe { usage.assume_init() };
    #[cfg(target_os = "macos")]
    {
        usage.ru_maxrss as u64
    }
    #[cfg(not(target_os = "macos"))]
    {
        (usage.ru_maxrss as u64).saturating_mul(1_024)
    }
}

#[cfg(not(unix))]
fn max_rss_bytes() -> u64 {
    0
}
