mod common;

use std::fs;

use termirust_store::{
    HealthCheckKind, HealthErrorCode, HealthFindingState, HealthRepository, IndexRepairKind,
    RepairCancellation, RepairFaultPoint,
};

use common::{StoreFixture, assert_no_repair_debris};

#[test]
fn crash_matrix_recovers_owned_temps_and_preserves_authoritative_bytes() {
    for kind in [
        IndexRepairKind::ProjectSessionIndex,
        IndexRepairKind::PaletteIndex,
    ] {
        for fault in [
            RepairFaultPoint::AfterTempCreated,
            RepairFaultPoint::AfterTempWrite,
            RepairFaultPoint::AfterTempSync,
            RepairFaultPoint::AfterJournalSync,
            RepairFaultPoint::AfterPublish,
        ] {
            let fixture = StoreFixture::new();
            let before = fixture.authoritative_bytes();
            let repository = HealthRepository::open(&fixture.metadata).unwrap();
            let plan = repository.plan_repair(kind).unwrap();
            let error = repository
                .repair_with_fault(plan, &RepairCancellation::default(), Some(fault))
                .unwrap_err();
            assert_eq!(error.code, HealthErrorCode::InjectedCrash);

            let recovered = HealthRepository::open(&fixture.metadata).unwrap();
            recovered.recover_interrupted_repair().unwrap();
            assert_no_repair_debris(&fixture.metadata);
            assert_eq!(fixture.authoritative_bytes(), before);
            let finding = recovered
                .scan()
                .finding(match kind {
                    IndexRepairKind::ProjectSessionIndex => HealthCheckKind::ProjectSessionIndex,
                    IndexRepairKind::PaletteIndex => HealthCheckKind::PaletteIndex,
                })
                .unwrap()
                .clone();
            if fault == RepairFaultPoint::AfterPublish {
                assert_eq!(finding.state, HealthFindingState::Healthy);
            } else {
                assert_eq!(finding.state, HealthFindingState::Partial);
            }
        }
    }
}

#[test]
fn cancellation_and_stale_revision_never_publish() {
    let cancelled = StoreFixture::new();
    let repository = HealthRepository::open(&cancelled.metadata).unwrap();
    let plan = repository
        .plan_repair(IndexRepairKind::ProjectSessionIndex)
        .unwrap();
    let cancellation = RepairCancellation::default();
    cancellation.cancel();
    let error = repository.repair(plan, &cancellation).unwrap_err();
    assert_eq!(error.code, HealthErrorCode::Cancelled);
    assert!(!cancelled.derived_path("project-session-v1.json").exists());

    let stale = StoreFixture::new();
    let before_projects = fs::read(stale.metadata.join("projects.json")).unwrap();
    let repository = HealthRepository::open(&stale.metadata).unwrap();
    let plan = repository
        .plan_repair(IndexRepairKind::PaletteIndex)
        .unwrap();
    let sessions_path = stale.metadata.join("sessions.json");
    let mut sessions: serde_json::Value =
        serde_json::from_slice(&fs::read(&sessions_path).unwrap()).unwrap();
    sessions["revision"] = 1.into();
    fs::write(
        &sessions_path,
        serde_json::to_vec_pretty(&sessions).unwrap(),
    )
    .unwrap();
    let error = repository
        .repair(plan, &RepairCancellation::default())
        .unwrap_err();
    assert_eq!(error.code, HealthErrorCode::StaleSource);
    assert!(!stale.derived_path("palette-v1.json").exists());
    assert_eq!(
        fs::read(stale.metadata.join("projects.json")).unwrap(),
        before_projects
    );
    assert_no_repair_debris(&stale.metadata);
}

#[test]
fn malformed_existing_index_is_replaced_only_by_its_named_repair() {
    let fixture = StoreFixture::new();
    let repository = HealthRepository::open(&fixture.metadata).unwrap();
    let project_plan = repository
        .plan_repair(IndexRepairKind::ProjectSessionIndex)
        .unwrap();
    repository
        .repair(project_plan, &RepairCancellation::default())
        .unwrap();
    fs::write(fixture.derived_path("project-session-v1.json"), b"corrupt").unwrap();
    let before = fixture.authoritative_bytes();
    let finding = repository
        .scan()
        .finding(HealthCheckKind::ProjectSessionIndex)
        .unwrap()
        .clone();
    assert_eq!(finding.state, HealthFindingState::Corrupt);
    let plan = repository
        .plan_repair(IndexRepairKind::ProjectSessionIndex)
        .unwrap();
    repository
        .repair(plan, &RepairCancellation::default())
        .unwrap();
    assert_eq!(fixture.authoritative_bytes(), before);
    assert_eq!(
        repository
            .scan()
            .finding(HealthCheckKind::ProjectSessionIndex)
            .unwrap()
            .state,
        HealthFindingState::Healthy
    );
}
