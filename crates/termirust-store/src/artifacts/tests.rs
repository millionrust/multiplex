use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};

use termirust_domain::{
    ArtifactCancellation, ArtifactError, ArtifactId, ArtifactLimits, ArtifactMediaType,
    ArtifactPreviewKind, ArtifactScope, ArtifactState, HostedSessionId,
};
use uuid::Uuid;

use super::*;

fn id(value: u128) -> ArtifactId {
    ArtifactId::from_uuid(Uuid::from_u128(value))
}

fn scope(value: u128) -> ArtifactScope {
    ArtifactScope {
        session_id: HostedSessionId::from_uuid(Uuid::from_u128(value)),
    }
}

fn request(
    id_value: u128,
    scope: ArtifactScope,
    source: impl Into<PathBuf>,
) -> ArtifactIngestRequest {
    ArtifactIngestRequest {
        id: id(id_value),
        scope,
        source: source.into(),
        display_name: None,
        created_at: id_value as u64,
    }
}

fn open_repository(root: &Path) -> ArtifactRepository {
    ArtifactRepository::open(root).unwrap()
}

fn artifact_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/artifacts")
        .join(name)
}

fn small_limits() -> ArtifactLimits {
    ArtifactLimits {
        item_bytes: 8,
        session_bytes: 12,
        global_bytes: 20,
        artifacts_per_session: 4,
        global_artifacts: 8,
        text_preview_bytes: 8,
        raster_pixels: 4,
        raster_bytes: 16,
    }
}

#[test]
fn artifacts_ingest_hash_list_restart_and_deduplicate_with_separate_metadata() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("result.txt");
    fs::write(&source, b"hello artifact").unwrap();
    let session = scope(1);
    let repository = open_repository(&fixture.path().join("sessions"));
    let first = repository
        .ingest(
            request(10, session, &source),
            &ArtifactCancellation::default(),
            |_| {},
        )
        .unwrap();
    let second = repository
        .ingest(
            request(11, session, &source),
            &ArtifactCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(first.sha256, second.sha256);
    assert_ne!(first.id, second.id);
    assert_eq!(first.media_type, ArtifactMediaType::TextPlainUtf8);
    assert_eq!(first.preview_kind, ArtifactPreviewKind::Text);
    assert_eq!(fs::read(&source).unwrap(), b"hello artifact");

    let snapshot = repository.list(session).unwrap();
    assert_eq!(snapshot.artifacts.len(), 2);
    assert_eq!(snapshot.session_bytes, first.byte_len);
    assert_eq!(
        repository
            .read_payload(session, first.id, &ArtifactCancellation::default())
            .unwrap()
            .bytes,
        b"hello artifact"
    );

    let reopened = open_repository(&fixture.path().join("sessions"));
    assert_eq!(reopened.list(session).unwrap().artifacts.len(), 2);
    #[cfg(unix)]
    {
        let first_data = reopened
            .bucket_path(session, READY_DIR)
            .join(first.id.to_string())
            .join(DATA_FILE);
        let second_data = reopened
            .bucket_path(session, READY_DIR)
            .join(second.id.to_string())
            .join(DATA_FILE);
        assert_eq!(
            fs::metadata(first_data).unwrap().ino(),
            fs::metadata(second_data).unwrap().ino()
        );
    }
}

#[test]
fn artifacts_sniff_bytes_not_extension_and_keep_active_content_metadata_only() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("sessions");
    let repository = open_repository(&root);
    let session = scope(2);
    let html = fixture.path().join("safe.txt");
    fs::write(&html, b"<!doctype html><script>never()</script>").unwrap();
    let png = fixture.path().join("renamed.html");
    fs::write(&png, b"\x89PNG\r\n\x1a\nnot-decoded-during-ingest").unwrap();
    let binary = fixture.path().join("binary.txt");
    fs::write(&binary, b"text\0binary").unwrap();

    let html = repository
        .ingest(request(20, session, html), &Default::default(), |_| {})
        .unwrap();
    let png = repository
        .ingest(request(21, session, png), &Default::default(), |_| {})
        .unwrap();
    let binary = repository
        .ingest(request(22, session, binary), &Default::default(), |_| {})
        .unwrap();
    assert_eq!(html.media_type, ArtifactMediaType::MetadataOnly);
    assert_eq!(html.preview_kind, ArtifactPreviewKind::MetadataOnly);
    assert_eq!(png.media_type, ArtifactMediaType::ImagePng);
    assert_eq!(png.preview_kind, ArtifactPreviewKind::Raster);
    assert_eq!(binary.media_type, ArtifactMediaType::MetadataOnly);
}

#[test]
fn artifacts_hostile_fixtures_are_classified_from_bytes_and_never_modified() {
    let fixture = tempfile::tempdir().unwrap();
    let repository = open_repository(&fixture.path().join("sessions"));
    let session = scope(13);
    let cases = [
        ("safe-text.txt", ArtifactMediaType::TextPlainUtf8),
        ("active-content.html", ArtifactMediaType::MetadataOnly),
        ("vector-content.svg", ArtifactMediaType::MetadataOnly),
        ("archive-signature.zip", ArtifactMediaType::MetadataOnly),
        ("spoofed-image.png", ArtifactMediaType::TextPlainUtf8),
    ];

    for (index, (name, expected)) in cases.into_iter().enumerate() {
        let source = artifact_fixture(name);
        let before = fs::read(&source).unwrap();
        let metadata = repository
            .ingest(
                request(100 + index as u128, session, &source),
                &Default::default(),
                |_| {},
            )
            .unwrap();
        assert_eq!(metadata.media_type, expected, "fixture {name}");
        assert_eq!(fs::read(source).unwrap(), before, "fixture {name}");
    }
}

#[test]
fn artifacts_item_session_global_and_count_quotas_are_exact() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("sessions");
    let repository =
        ArtifactRepository::open_with(&root, small_limits(), Arc::new(SystemAtomicWriter)).unwrap();
    let first_source = fixture.path().join("first");
    fs::write(&first_source, b"12345678").unwrap();
    repository
        .ingest(
            request(30, scope(3), &first_source),
            &Default::default(),
            |_| {},
        )
        .unwrap();
    let too_large = fixture.path().join("large");
    fs::write(&too_large, b"123456789").unwrap();
    assert_eq!(
        repository
            .ingest(
                request(31, scope(3), too_large),
                &Default::default(),
                |_| {}
            )
            .unwrap_err(),
        ArtifactStoreError::Domain(ArtifactError::ItemQuotaExceeded)
    );
    let five = fixture.path().join("five");
    fs::write(&five, b"12345").unwrap();
    assert_eq!(
        repository
            .ingest(request(32, scope(3), &five), &Default::default(), |_| {})
            .unwrap_err(),
        ArtifactStoreError::Domain(ArtifactError::SessionQuotaExceeded)
    );
    repository
        .ingest(request(33, scope(4), &five), &Default::default(), |_| {})
        .unwrap();
    let eight = fixture.path().join("eight");
    fs::write(&eight, b"abcdefgh").unwrap();
    assert_eq!(
        repository
            .ingest(request(34, scope(5), eight), &Default::default(), |_| {})
            .unwrap_err(),
        ArtifactStoreError::Domain(ArtifactError::GlobalQuotaExceeded)
    );
}

#[test]
fn artifacts_cancel_and_source_change_remove_staging_without_committing() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("sessions");
    let repository = open_repository(&root);
    let session = scope(6);
    let source = fixture.path().join("large.txt");
    fs::write(&source, vec![b'x'; IO_CHUNK_BYTES * 2]).unwrap();
    let cancellation = ArtifactCancellation::default();
    let cancel_from_progress = cancellation.clone();
    assert_eq!(
        repository
            .ingest(request(40, session, &source), &cancellation, move |_| {
                cancel_from_progress.cancel();
            })
            .unwrap_err(),
        ArtifactStoreError::Domain(ArtifactError::Cancelled)
    );
    assert!(repository.list(session).unwrap().artifacts.is_empty());
    assert_eq!(
        fs::read_dir(repository.bucket_path(session, STAGING_DIR))
            .unwrap()
            .count(),
        0
    );

    let changed_source = fixture.path().join("changed.txt");
    fs::write(&changed_source, vec![b'a'; IO_CHUNK_BYTES * 2]).unwrap();
    let source_for_progress = changed_source.clone();
    let mut changed = false;
    assert_eq!(
        repository
            .ingest(
                request(41, session, &changed_source),
                &Default::default(),
                move |_| {
                    if !changed {
                        changed = true;
                        fs::write(&source_for_progress, vec![b'b'; IO_CHUNK_BYTES * 2]).unwrap();
                    }
                }
            )
            .unwrap_err(),
        ArtifactStoreError::Domain(ArtifactError::SourceChanged)
    );
    assert!(repository.list(session).unwrap().artifacts.is_empty());
}

#[test]
fn artifacts_quarantine_restore_export_and_purge_are_recoverable() {
    let fixture = tempfile::tempdir().unwrap();
    let repository = open_repository(&fixture.path().join("sessions"));
    let session = scope(7);
    let source = fixture.path().join("evidence.txt");
    fs::write(&source, b"verified export").unwrap();
    let artifact = repository
        .ingest(request(50, session, source), &Default::default(), |_| {})
        .unwrap();
    assert_eq!(
        repository.quarantine(session, artifact.id).unwrap().state,
        ArtifactState::Quarantined
    );
    assert_eq!(
        repository.list(session).unwrap().artifacts[0].state,
        ArtifactState::Quarantined
    );
    assert_eq!(
        repository
            .read_payload(session, artifact.id, &Default::default())
            .unwrap_err(),
        ArtifactStoreError::Domain(ArtifactError::InvalidState)
    );
    assert_eq!(
        repository
            .restore(session, artifact.id, &Default::default())
            .unwrap()
            .state,
        ArtifactState::Ready
    );
    let export = fixture.path().join("export.txt");
    repository
        .export_copy(session, artifact.id, &export, &Default::default())
        .unwrap();
    assert_eq!(fs::read(&export).unwrap(), b"verified export");
    assert_eq!(
        repository
            .export_copy(session, artifact.id, &export, &Default::default())
            .unwrap_err(),
        ArtifactStoreError::Domain(ArtifactError::Conflict)
    );
    repository.quarantine(session, artifact.id).unwrap();
    repository
        .purge(session, artifact.id, &Default::default())
        .unwrap();
    assert!(repository.list(session).unwrap().artifacts.is_empty());
}

#[test]
fn artifacts_detect_corrupt_bytes_and_metadata_without_exposing_content() {
    let fixture = tempfile::tempdir().unwrap();
    let repository = open_repository(&fixture.path().join("sessions"));
    let session = scope(8);
    let source = fixture.path().join("canary-secret.txt");
    fs::write(&source, b"canary-secret-value").unwrap();
    let artifact = repository
        .ingest(request(60, session, source), &Default::default(), |_| {})
        .unwrap();
    let directory = repository
        .bucket_path(session, READY_DIR)
        .join(artifact.id.to_string());
    fs::write(directory.join(DATA_FILE), b"xxxxxxxxxxxxxxxxxxx").unwrap();
    let error = repository
        .read_payload(session, artifact.id, &Default::default())
        .unwrap_err();
    assert_eq!(error, ArtifactStoreError::Domain(ArtifactError::Corrupt));
    assert!(!format!("{error:?}").contains("canary"));

    fs::write(directory.join(METADATA_FILE), b"not-json").unwrap();
    let listed = repository.list(session).unwrap();
    assert_eq!(listed.artifacts[0].state, ArtifactState::Corrupt);
    assert_eq!(listed.artifacts[0].display_name.as_str(), "artifact");
}

#[test]
fn artifacts_sweep_only_expired_typed_staging_entries() {
    let fixture = tempfile::tempdir().unwrap();
    let repository = open_repository(&fixture.path().join("sessions"));
    let session = scope(9);
    repository.prepare_session_directories(session).unwrap();
    let staging = repository
        .bucket_path(session, STAGING_DIR)
        .join(id(70).to_string());
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join(DATA_FILE), b"partial").unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert_eq!(repository.sweep_staging(now).unwrap().removed_entries, 0);
    let swept = repository
        .sweep_staging(now + STAGING_RETENTION_MILLIS + 1)
        .unwrap();
    assert_eq!(swept.removed_entries, 1);
    assert_eq!(swept.removed_bytes, 7);
    assert!(!staging.exists());
}

#[cfg(unix)]
#[test]
fn artifacts_reject_symlink_sources_and_storage_entries() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("sessions");
    let repository = open_repository(&root);
    let source = fixture.path().join("source");
    fs::write(&source, b"safe").unwrap();
    let linked = fixture.path().join("linked");
    symlink(&source, &linked).unwrap();
    assert_eq!(
        repository
            .ingest(request(80, scope(10), linked), &Default::default(), |_| {})
            .unwrap_err(),
        ArtifactStoreError::Domain(ArtifactError::UnsupportedSource)
    );

    let session = scope(11);
    repository.prepare_session_directories(session).unwrap();
    let outside = fixture.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let entry = repository
        .bucket_path(session, READY_DIR)
        .join(id(81).to_string());
    symlink(&outside, &entry).unwrap();
    assert!(matches!(
        repository.list(session),
        Err(ArtifactStoreError::UnsafeEntry { .. })
    ));
    assert_eq!(
        fs::metadata(root).unwrap().permissions().mode() & 0o777,
        0o700
    );

    let lock_root = fixture.path().join("lock-root");
    fs::create_dir(&lock_root).unwrap();
    let outside_lock = fixture.path().join("outside-lock");
    fs::write(&outside_lock, b"must-not-open").unwrap();
    symlink(&outside_lock, lock_root.join(LOCK_FILE)).unwrap();
    assert!(ArtifactRepository::open(&lock_root).is_err());
    assert_eq!(fs::read(outside_lock).unwrap(), b"must-not-open");
}

#[derive(Debug)]
struct FailingWriter;

impl AtomicWriter for FailingWriter {
    fn write(&self, _target: &Path, _bytes: &[u8]) -> io::Result<Durability> {
        Err(io::Error::new(io::ErrorKind::StorageFull, "synthetic"))
    }
}

#[test]
fn artifacts_metadata_commit_failure_leaves_no_visible_or_staged_artifact() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("sessions");
    let repository =
        ArtifactRepository::open_with(&root, ArtifactLimits::default(), Arc::new(FailingWriter))
            .unwrap();
    let source = fixture.path().join("source.txt");
    fs::write(&source, b"content").unwrap();
    let session = scope(12);
    assert_eq!(
        repository
            .ingest(request(90, session, source), &Default::default(), |_| {})
            .unwrap_err(),
        ArtifactStoreError::Domain(ArtifactError::StorageFull)
    );
    assert!(repository.list(session).unwrap().artifacts.is_empty());
    assert_eq!(
        fs::read_dir(repository.bucket_path(session, STAGING_DIR))
            .unwrap()
            .count(),
        0
    );
}
