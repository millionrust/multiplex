mod common;

use std::fs;

use termirust_store::{
    HealthCheckKind, HealthEvidenceCode, HealthFindingState, HealthRepository, IndexRepairKind,
    RepairCancellation,
};

use common::StoreFixture;

#[test]
fn scan_is_read_only_and_reports_only_the_two_missing_derived_indexes() {
    let fixture = StoreFixture::new();
    let before = fixture.authoritative_bytes();
    let repository = HealthRepository::open(&fixture.metadata).unwrap();
    let report = repository.scan();

    assert_eq!(fixture.authoritative_bytes(), before);
    for kind in [
        HealthCheckKind::StoreReadable,
        HealthCheckKind::StoreVersion,
        HealthCheckKind::RecordHashes,
    ] {
        assert_eq!(
            report.finding(kind).unwrap().state,
            HealthFindingState::Healthy
        );
    }
    for kind in [
        HealthCheckKind::ProjectSessionIndex,
        HealthCheckKind::PaletteIndex,
    ] {
        let finding = report.finding(kind).unwrap();
        assert_eq!(finding.state, HealthFindingState::Partial);
        assert_eq!(finding.evidence, HealthEvidenceCode::IndexMissing);
    }
}

#[test]
fn named_repairs_make_scan_healthy_without_changing_authoritative_metadata() {
    let fixture = StoreFixture::new();
    let before = fixture.authoritative_bytes();
    let repository = HealthRepository::open(&fixture.metadata).unwrap();
    for kind in [
        IndexRepairKind::ProjectSessionIndex,
        IndexRepairKind::PaletteIndex,
    ] {
        let plan = repository.plan_repair(kind).unwrap();
        assert!(plan.estimated_bytes > 0);
        repository
            .repair(plan, &RepairCancellation::default())
            .unwrap();
    }
    assert!(repository.scan().is_healthy());
    assert_eq!(fixture.authoritative_bytes(), before);
}

#[test]
fn scan_does_not_create_a_missing_metadata_lock() {
    let fixture = StoreFixture::new();
    let lock_path = fixture.metadata.join("metadata.lock");
    fs::remove_file(&lock_path).unwrap();
    let before = fixture.authoritative_bytes();

    let report = HealthRepository::open(&fixture.metadata).unwrap().scan();

    assert!(!lock_path.exists());
    assert_eq!(fixture.authoritative_bytes(), before);
    assert_eq!(
        report
            .finding(HealthCheckKind::StoreReadable)
            .unwrap()
            .state,
        HealthFindingState::Unavailable
    );
}

#[test]
fn open_and_scan_do_not_clean_or_rewrite_derived_repair_debris() {
    let fixture = StoreFixture::new();
    let index_root = fixture.metadata.join("derived-indexes");
    fs::create_dir(&index_root).unwrap();
    fs::write(
        index_root.join(".termirust-derived-indexes-v1"),
        b"termirust-derived-indexes-v1\n",
    )
    .unwrap();
    let temporary = index_root.join(".repair-owned-test.tmp");
    fs::write(&temporary, b"interrupted").unwrap();

    let repository = HealthRepository::open(&fixture.metadata).unwrap();
    let _report = repository.scan();

    assert_eq!(fs::read(temporary).unwrap(), b"interrupted");
}

#[cfg(unix)]
#[test]
fn repair_outputs_are_private_to_the_current_user() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = StoreFixture::new();
    let repository = HealthRepository::open(&fixture.metadata).unwrap();
    for kind in [
        IndexRepairKind::ProjectSessionIndex,
        IndexRepairKind::PaletteIndex,
    ] {
        let plan = repository.plan_repair(kind).unwrap();
        repository
            .repair(plan, &RepairCancellation::default())
            .unwrap();
    }

    let index_root = fixture.metadata.join("derived-indexes");
    assert_eq!(
        fs::metadata(&index_root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in [
        ".termirust-derived-indexes-v1",
        "project-session-v1.json",
        "palette-v1.json",
    ] {
        assert_eq!(
            fs::metadata(index_root.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "unexpected permissions for {name}"
        );
    }
}

#[test]
fn corrupt_and_future_sources_produce_closed_read_only_findings() {
    let corrupt = StoreFixture::new();
    fs::write(corrupt.metadata.join("projects.json"), b"{not-json").unwrap();
    let repository = HealthRepository::open(&corrupt.metadata).unwrap();
    let report = repository.scan();
    assert_eq!(
        report
            .finding(HealthCheckKind::StoreReadable)
            .unwrap()
            .state,
        HealthFindingState::Corrupt
    );
    assert!(
        repository
            .plan_repair(IndexRepairKind::ProjectSessionIndex)
            .is_err()
    );

    let future = StoreFixture::new();
    let mut format: serde_json::Value =
        serde_json::from_slice(&fs::read(future.metadata.join("format.json")).unwrap()).unwrap();
    format["format_version"] = 999.into();
    fs::write(
        future.metadata.join("format.json"),
        serde_json::to_vec(&format).unwrap(),
    )
    .unwrap();
    let report = HealthRepository::open(&future.metadata).unwrap().scan();
    assert_eq!(
        report
            .finding(HealthCheckKind::StoreReadable)
            .unwrap()
            .state,
        HealthFindingState::Healthy
    );
    assert_eq!(
        report.finding(HealthCheckKind::StoreVersion).unwrap().state,
        HealthFindingState::Newer
    );
}

#[cfg(unix)]
#[test]
fn symlinked_derived_index_is_never_followed_or_replaced() {
    use std::os::unix::fs::symlink;

    let fixture = StoreFixture::new();
    let repository = HealthRepository::open(&fixture.metadata).unwrap();
    let plan = repository
        .plan_repair(IndexRepairKind::PaletteIndex)
        .unwrap();
    fs::create_dir(fixture.metadata.join("derived-indexes")).unwrap();
    let outside = fixture.temp.path().join("outside-sentinel");
    fs::write(&outside, b"keep").unwrap();
    symlink(&outside, &plan.target_path).unwrap();
    assert!(
        repository
            .repair(plan, &RepairCancellation::default())
            .is_err()
    );
    assert_eq!(fs::read(outside).unwrap(), b"keep");
}
