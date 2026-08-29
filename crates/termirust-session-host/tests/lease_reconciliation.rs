#![cfg(unix)]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use termirust_client::{
    AuthenticatedHostPeer, HostPeerProbe, HostProbeRequest, HostReconciliationError,
    HostReconciliationPlan, HostReconciliationService, HostRecoveryFaultPoint,
};
use termirust_domain::{ActivityAggregate, HostInstanceId, HostLifecycle, HostedSessionId};
use termirust_host_protocol::opaque_endpoint_name;
use termirust_session_host::{LaunchDescriptor, StopDeadlines, start};
use termirust_store::{HostLease, HostMetadata, JournalLimits, RecoveryResult, read_host_metadata};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct FixedProbe {
    peers: Vec<AuthenticatedHostPeer>,
}

impl HostPeerProbe for FixedProbe {
    fn probe<'a>(
        &'a self,
        _: &'a HostProbeRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<AuthenticatedHostPeer>, HostReconciliationError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { Ok(self.peers.clone()) })
    }
}

fn metadata(session_id: HostedSessionId, host_instance_id: HostInstanceId) -> HostMetadata {
    HostMetadata {
        format_version: HostMetadata::FORMAT_VERSION,
        session_id,
        host_instance_id,
        process_token: None,
        runtime_recognition: None,
        activity: ActivityAggregate::default(),
        lifecycle: HostLifecycle::Ready,
        endpoint_name: opaque_endpoint_name(session_id),
        heartbeat_monotonic_nanos: 42,
        durability_watermark: None,
    }
}

fn released_lease_fixture() -> (tempfile::TempDir, HostedSessionId, HostInstanceId) {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let host_instance_id = HostInstanceId::new();
    let lease = HostLease::acquire(fixture.path().join("session"), host_instance_id).unwrap();
    lease
        .write_metadata(&metadata(session_id, host_instance_id))
        .unwrap();
    drop(lease);
    (fixture, session_id, host_instance_id)
}

fn service(
    fixture: &tempfile::TempDir,
    peers: Vec<AuthenticatedHostPeer>,
) -> HostReconciliationService<FixedProbe> {
    HostReconciliationService::with_probe(
        fixture.path().join("runtime"),
        Arc::new(FixedProbe { peers }),
    )
}

#[tokio::test]
async fn released_lease_is_backed_up_verified_and_marked_orphaned() {
    let (fixture, _, _) = released_lease_fixture();
    let session_dir = fixture.path().join("session");
    let original = std::fs::read(session_dir.join("host.json")).unwrap();
    let service = service(&fixture, Vec::new());
    let plan = service.plan(&session_dir).await.unwrap();
    assert_eq!(plan.preview_result, RecoveryResult::Reconciled);
    let backup = plan.current_backup_path.clone();
    let receipt = service.reconcile(plan, &CancellationToken::new()).unwrap();
    assert_eq!(receipt.result, RecoveryResult::Reconciled);
    assert_eq!(
        read_host_metadata(&session_dir).unwrap().lifecycle,
        HostLifecycle::Orphaned
    );
    assert_eq!(std::fs::read(backup).unwrap(), original);
}

#[tokio::test]
async fn held_lease_requires_one_exact_authenticated_peer() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let host_instance_id = HostInstanceId::new();
    let session_dir = fixture.path().join("session");
    let lease = HostLease::acquire(&session_dir, host_instance_id).unwrap();
    lease
        .write_metadata(&metadata(session_id, host_instance_id))
        .unwrap();
    let matching = AuthenticatedHostPeer {
        session_id,
        host_instance_id,
    };

    let exact = service(&fixture, vec![matching]);
    let plan = exact.plan(&session_dir).await.unwrap();
    assert_eq!(plan.preview_result, RecoveryResult::NoChange);
    assert_eq!(
        exact
            .reconcile(plan, &CancellationToken::new())
            .unwrap()
            .result,
        RecoveryResult::NoChange
    );

    let wrong = service(
        &fixture,
        vec![AuthenticatedHostPeer {
            session_id,
            host_instance_id: HostInstanceId::new(),
        }],
    );
    assert_eq!(
        wrong.plan(&session_dir).await.unwrap().preview_result,
        RecoveryResult::Ambiguous
    );
    let multiple = service(&fixture, vec![matching, matching]);
    let plan = multiple.plan(&session_dir).await.unwrap();
    assert_eq!(plan.preview_result, RecoveryResult::Ambiguous);
    assert_eq!(
        multiple
            .reconcile(plan, &CancellationToken::new())
            .unwrap()
            .result,
        RecoveryResult::Ambiguous
    );
    assert_eq!(
        read_host_metadata(&session_dir).unwrap().lifecycle,
        HostLifecycle::Ready
    );
    drop(lease);
}

#[tokio::test]
async fn journaled_host_crashes_restore_exact_metadata_on_restart() {
    for fault in [
        HostRecoveryFaultPoint::AfterJournal,
        HostRecoveryFaultPoint::AfterApply,
        HostRecoveryFaultPoint::AfterVerification,
    ] {
        let (fixture, _, _) = released_lease_fixture();
        let session_dir = fixture.path().join("session");
        let original = std::fs::read(session_dir.join("host.json")).unwrap();
        let service = service(&fixture, Vec::new());
        let plan = service.plan(&session_dir).await.unwrap();
        assert!(
            service
                .reconcile_with_fault(plan, &CancellationToken::new(), Some(fault))
                .is_err()
        );
        let receipt = service
            .recover_interrupted_reconciliation(&session_dir)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.result, RecoveryResult::RolledBack);
        assert_eq!(
            std::fs::read(session_dir.join("host.json")).unwrap(),
            original
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_probe_authenticates_the_running_host_instance() {
    let fixture = tempfile::tempdir().unwrap();
    let runtime_root = fixture.path().join("runtime");
    let session_dir = fixture.path().join("session");
    std::fs::create_dir(&runtime_root).unwrap();
    let session_id = HostedSessionId::new();
    let host_instance_id = HostInstanceId::new();
    let descriptor = LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id,
        expected_occupant_generation: None,
        runtime_root: runtime_root.clone(),
        session_dir: session_dir.clone(),
        executable: "/bin/sh".into(),
        runtime_detection: None,
        arguments: vec!["-c".into(), "while :; do sleep 1; done".into()],
        environment: BTreeMap::from([
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("TERM".to_string(), "xterm-256color".to_string()),
        ]),
        cwd: Some(fixture.path().to_path_buf()),
        columns: 80,
        rows: 24,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    };
    let handle = start(descriptor).await.unwrap();
    let service = HostReconciliationService::new(&runtime_root);
    let plan: HostReconciliationPlan = service.plan(&session_dir).await.unwrap();
    assert_eq!(plan.preview_result, RecoveryResult::NoChange);
    assert_eq!(plan.authenticated_peers.len(), 1);
    assert_eq!(
        plan.authenticated_peers[0].host_instance_id,
        host_instance_id
    );
    handle.shutdown().await.unwrap();
}
