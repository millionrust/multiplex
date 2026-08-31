#[path = "../../termirust-cli/tests/common/mod.rs"]
mod cli_common;
#[path = "../../../tests/support/local_controller_conformance.rs"]
mod conformance;

use std::sync::{Arc, Mutex};

use cli_common::{FakeControllerState, FakeLauncherState, SeededStore, seed_store, service};
use conformance::{
    ConformanceFixture, FixtureMutation, domain_mutation, load_fixture, normalized_snapshot,
};
use termirust_cli::{Cancellation, ErrorCode, ManagementCommand as CliManagementCommand};
use termirust_domain::{CommandId, Revision};
use termirust_tui::{
    LocalManagementExecutor, ManagementCommand as TuiManagementCommand, ManagementExecutor,
};

#[test]
fn local_controller_conformance_cli_and_tui_match_shared_fixture() {
    let fixture = load_fixture();
    let cli_seed = seeded_fixture(&fixture);
    let tui_seed = seeded_fixture(&fixture);
    assert_eq!(
        normalized_snapshot(&cli_seed.sessions().load().unwrap()),
        fixture.expected_initial
    );
    assert_eq!(
        normalized_snapshot(&tui_seed.sessions().load().unwrap()),
        fixture.expected_initial
    );

    let cli = service(
        &cli_seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let tui = LocalManagementExecutor::with_host_executable(
        tui_seed.config_root.clone(),
        tui_seed.temp.path().join("unused-session-host"),
    );

    for step in &fixture.steps {
        let cli_revision = current_session_revision(&cli_seed);
        cli.execute_management(
            cli_command(&step.mutation, cli_revision),
            &Cancellation::default(),
        )
        .unwrap();
        let tui_revision = current_session_revision(&tui_seed);
        tui.execute(
            tui_command(&step.mutation, tui_revision),
            &Cancellation::default(),
        )
        .unwrap();

        let cli_projection = normalized_snapshot(&cli_seed.sessions().load().unwrap());
        let tui_projection = normalized_snapshot(&tui_seed.sessions().load().unwrap());
        assert_eq!(cli_projection, step.expected);
        assert_eq!(tui_projection, step.expected);
        assert_eq!(cli_projection, tui_projection);
    }

    assert_stale_cli_is_non_mutating(&fixture, &cli_seed, &cli);
    assert_stale_tui_is_non_mutating(&fixture, &tui_seed, &tui);
}

fn seeded_fixture(fixture: &ConformanceFixture) -> SeededStore {
    let seed = seed_store();
    seed.sessions()
        .create_session(fixture.initial.clone(), Revision::ZERO)
        .unwrap();
    seed
}

fn current_session_revision(seed: &SeededStore) -> u64 {
    seed.sessions().load().unwrap().sessions[0].revision.get()
}

fn assert_stale_cli_is_non_mutating(
    fixture: &ConformanceFixture,
    seed: &SeededStore,
    service: &termirust_cli::LocalCommandService,
) {
    apply_intervening_mutation(fixture, seed);
    let sessions_path = seed.metadata_root.join("sessions.json");
    let before = std::fs::read(&sessions_path).unwrap();
    let command = cli_command(
        &fixture.stale.attempt,
        fixture.stale.captured_session_revision,
    );
    assert!(!format!("{command:?}").contains("MUST NOT APPLY"));
    let error = service
        .execute_management(command, &Cancellation::default())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(!format!("{error:?}").contains("MUST NOT APPLY"));
    assert_eq!(std::fs::read(sessions_path).unwrap(), before);
    assert_eq!(
        normalized_snapshot(&seed.sessions().load().unwrap()),
        fixture.stale.expected
    );
}

fn assert_stale_tui_is_non_mutating(
    fixture: &ConformanceFixture,
    seed: &SeededStore,
    executor: &LocalManagementExecutor,
) {
    apply_intervening_mutation(fixture, seed);
    let sessions_path = seed.metadata_root.join("sessions.json");
    let before = std::fs::read(&sessions_path).unwrap();
    let command = tui_command(
        &fixture.stale.attempt,
        fixture.stale.captured_session_revision,
    );
    assert!(!format!("{command:?}").contains("MUST NOT APPLY"));
    let error = executor
        .execute(command, &Cancellation::default())
        .unwrap_err();
    assert_eq!(error.code, "conflict");
    assert!(!format!("{error:?}").contains("MUST NOT APPLY"));
    assert_eq!(std::fs::read(sessions_path).unwrap(), before);
    assert_eq!(
        normalized_snapshot(&seed.sessions().load().unwrap()),
        fixture.stale.expected
    );
}

fn apply_intervening_mutation(fixture: &ConformanceFixture, seed: &SeededStore) {
    let repository = seed.sessions();
    let snapshot = repository.load().unwrap();
    repository
        .mutate_session(
            fixture.initial.id,
            snapshot.revision,
            domain_mutation(&fixture.stale.intervening),
            200,
        )
        .unwrap();
}

fn cli_command(mutation: &FixtureMutation, expected_revision: u64) -> CliManagementCommand {
    match mutation {
        FixtureMutation::Rename(title) => CliManagementCommand::Rename {
            command_id: CommandId::new(),
            session_id: cli_common::SESSION_ID,
            expected_revision: Revision::new(expected_revision),
            title: title.clone(),
        },
        FixtureMutation::SetPinned(pinned) => CliManagementCommand::SetPinned {
            command_id: CommandId::new(),
            session_id: cli_common::SESSION_ID,
            expected_revision: Revision::new(expected_revision),
            pinned: *pinned,
        },
        FixtureMutation::MarkRead => CliManagementCommand::MarkRead {
            command_id: CommandId::new(),
            session_id: cli_common::SESSION_ID,
            expected_revision: Revision::new(expected_revision),
        },
        FixtureMutation::Archive => CliManagementCommand::Archive {
            command_id: CommandId::new(),
            session_id: cli_common::SESSION_ID,
            expected_revision: Revision::new(expected_revision),
        },
        FixtureMutation::Restore => CliManagementCommand::Restore {
            command_id: CommandId::new(),
            session_id: cli_common::SESSION_ID,
            expected_revision: Revision::new(expected_revision),
        },
    }
}

fn tui_command(mutation: &FixtureMutation, expected_revision: u64) -> TuiManagementCommand {
    let session_id = cli_common::SESSION_ID.to_string();
    match mutation {
        FixtureMutation::Rename(title) => TuiManagementCommand::Rename {
            command_id: CommandId::new(),
            session_id,
            expected_revision,
            title: title.clone(),
        },
        FixtureMutation::SetPinned(pinned) => TuiManagementCommand::SetPinned {
            command_id: CommandId::new(),
            session_id,
            expected_revision,
            pinned: *pinned,
        },
        FixtureMutation::MarkRead => TuiManagementCommand::MarkRead {
            command_id: CommandId::new(),
            session_id,
            expected_revision,
        },
        FixtureMutation::Archive => TuiManagementCommand::Archive {
            command_id: CommandId::new(),
            session_id,
            expected_revision,
        },
        FixtureMutation::Restore => TuiManagementCommand::Restore {
            command_id: CommandId::new(),
            session_id,
            expected_revision,
        },
    }
}
