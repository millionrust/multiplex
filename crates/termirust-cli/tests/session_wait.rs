mod common;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{process::Command, str};

use common::*;
use termirust_cli::{
    Cancellation, CliCommand, CliData, CliWaiter, CommandService, ErrorCode, RenderOptions,
    SessionWaitCondition, render_success,
};
use termirust_domain::{ActivityState, HostedSessionState, SessionMutation};

struct ScriptedWaiter {
    now_ms: AtomicU64,
    waits: AtomicUsize,
    action: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl ScriptedWaiter {
    fn new(action: Option<Box<dyn FnOnce() + Send>>) -> Self {
        Self {
            now_ms: AtomicU64::new(0),
            waits: AtomicUsize::new(0),
            action: Mutex::new(action),
        }
    }

    fn wait_count(&self) -> usize {
        self.waits.load(Ordering::Acquire)
    }
}

impl CliWaiter for ScriptedWaiter {
    fn now(&self) -> Duration {
        Duration::from_millis(self.now_ms.load(Ordering::Acquire))
    }

    fn sleep_interruptibly(&self, duration: Duration, cancellation: &Cancellation) -> bool {
        self.waits.fetch_add(1, Ordering::AcqRel);
        if let Some(action) = self.action.lock().unwrap().take() {
            action();
        }
        let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.now_ms.fetch_add(millis, Ordering::AcqRel);
        !cancellation.is_cancelled()
    }
}

fn wait_command(condition: SessionWaitCondition, timeout_ms: u64) -> CliCommand {
    CliCommand::SessionWait {
        session_id: SESSION_ID,
        condition,
        timeout_ms,
    }
}

fn wait_service(
    seed: &SeededStore,
    waiter: Arc<ScriptedWaiter>,
) -> termirust_cli::LocalCommandService {
    service(
        seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    )
    .with_waiter(waiter)
}

#[test]
fn immediate_lifecycle_and_activity_matches_are_read_only_and_render_stably() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Exited, false);
    let repository = seed.sessions();
    let snapshot = repository.load().unwrap();
    let session = snapshot
        .sessions
        .iter()
        .find(|item| item.id == SESSION_ID)
        .unwrap();
    let mut activity = session.activity.clone();
    activity.state = ActivityState::Done;
    activity.stale = false;
    repository
        .mutate_session(
            SESSION_ID,
            snapshot.revision,
            SessionMutation::SetActivity(activity),
            2,
        )
        .unwrap();
    let before = repository.load().unwrap();
    let waiter = Arc::new(ScriptedWaiter::new(None));
    let mut service = wait_service(&seed, waiter.clone());

    let lifecycle = service
        .execute(
            wait_command(
                SessionWaitCondition::Lifecycle(HostedSessionState::Exited),
                1,
            ),
            &Cancellation::default(),
        )
        .unwrap();
    let activity = service
        .execute(
            wait_command(SessionWaitCondition::Activity(ActivityState::Done), 1),
            &Cancellation::default(),
        )
        .unwrap();

    assert!(matches!(lifecycle, CliData::Wait(_)));
    assert!(matches!(activity, CliData::Wait(_)));
    assert_eq!(waiter.wait_count(), 0);
    assert!(repository.load().unwrap() == before);

    let human = String::from_utf8(
        render_success(
            &activity,
            &[],
            RenderOptions {
                json: false,
                terminal_width: 80,
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert!(human.contains("Wait condition matched: activity=done"));
    assert!(human.contains(&SESSION_ID.to_string()));
    assert!(!human.contains(seed.project_root.to_string_lossy().as_ref()));

    let json = render_success(
        &activity,
        &[],
        RenderOptions {
            json: true,
            terminal_width: 80,
        },
    )
    .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(json["data"]["condition"]["kind"], "activity");
    assert_eq!(json["data"]["condition"]["state"], "done");
    assert!(!json.to_string().contains("durable-sessions"));
}

#[test]
fn delayed_transition_matches_after_one_bounded_poll() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let repository = seed.sessions();
    let action_repository = repository.clone();
    let waiter = Arc::new(ScriptedWaiter::new(Some(Box::new(move || {
        let snapshot = action_repository.load().unwrap();
        action_repository
            .mutate_session(
                SESSION_ID,
                snapshot.revision,
                SessionMutation::SetLifecycle(HostedSessionState::Stopping),
                2,
            )
            .unwrap();
    }))));
    let mut service = wait_service(&seed, waiter.clone());

    let data = service
        .execute(
            wait_command(
                SessionWaitCondition::Lifecycle(HostedSessionState::Stopping),
                1_000,
            ),
            &Cancellation::default(),
        )
        .unwrap();
    let CliData::Wait(data) = data else {
        panic!("expected wait result");
    };
    assert_eq!(data.session.state, "stopping");
    assert_eq!(waiter.wait_count(), 1);
    assert_eq!(waiter.now(), Duration::from_millis(50));
}

#[test]
fn timeout_and_cancellation_are_bounded_and_return_stable_exit_classes() {
    let seed = seed_store();
    let session = insert_session(&seed, HostedSessionState::Exited, false);
    let timeout_waiter = Arc::new(ScriptedWaiter::new(None));
    let mut service = wait_service(&seed, timeout_waiter.clone());
    let error = service
        .execute(
            wait_command(SessionWaitCondition::Lifecycle(HostedSessionState::Live), 0),
            &Cancellation::default(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Validation);
    assert_eq!(timeout_waiter.wait_count(), 0);

    let error = service
        .execute(
            wait_command(
                SessionWaitCondition::Lifecycle(HostedSessionState::Live),
                120,
            ),
            &Cancellation::default(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Timeout);
    assert_eq!(error.code.exit_code(), 7);
    assert_eq!(error.current_revision, Some(session.revision.get()));
    assert_eq!(timeout_waiter.wait_count(), 3);
    assert_eq!(timeout_waiter.now(), Duration::from_millis(120));

    let cancellation = Cancellation::default();
    let cancellation_action = cancellation.clone();
    let cancel_waiter = Arc::new(ScriptedWaiter::new(Some(Box::new(move || {
        cancellation_action.cancel();
    }))));
    let mut service = wait_service(&seed, cancel_waiter.clone());
    let error = service
        .execute(
            wait_command(SessionWaitCondition::Activity(ActivityState::Busy), 300_000),
            &cancellation,
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert_eq!(error.code.exit_code(), 130);
    assert_eq!(cancel_waiter.wait_count(), 1);
}

#[test]
fn session_disappearance_fails_visibly_without_observing_unrelated_data() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Exited, true);
    let repository = seed.sessions();
    let action_repository = repository.clone();
    let waiter = Arc::new(ScriptedWaiter::new(Some(Box::new(move || {
        let plan = action_repository.removal_plan(SESSION_ID).unwrap();
        action_repository
            .remove_session(&plan, plan.expected_revision)
            .unwrap();
    }))));
    let mut service = wait_service(&seed, waiter);
    let error = service
        .execute(
            wait_command(
                SessionWaitCondition::Lifecycle(HostedSessionState::Live),
                1_000,
            ),
            &Cancellation::default(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(error.code.exit_code(), 3);
}

#[test]
fn packaged_binary_waits_against_the_disposable_authoritative_repository() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Exited, false);
    let output = Command::new(env!("CARGO_BIN_EXE_termirust-cli"))
        .args([
            "session",
            "wait",
            &SESSION_ID.to_string(),
            "--state",
            "exited",
            "--timeout-ms",
            "1",
            "--json",
        ])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        str::from_utf8(&output.stderr).unwrap()
    );
    assert!(output.stderr.is_empty());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["data"]["condition"]["kind"], "lifecycle");
    assert_eq!(response["data"]["condition"]["state"], "exited");
    assert_eq!(response["data"]["session"]["id"], SESSION_ID.to_string());
}
