#![cfg(unix)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use termirust_client::{
    AuthenticatedHostPeer, HostPeerProbe, HostProbeRequest, HostReconciliationError,
    HostReconciliationService,
};
use termirust_domain::{ActivityAggregate, HostInstanceId, HostLifecycle, HostedSessionId};
use termirust_host_protocol::opaque_endpoint_name;
use termirust_store::{HostLease, HostMetadata, RecoveryResult};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct WrongPeer;

impl HostPeerProbe for WrongPeer {
    fn probe<'a>(
        &'a self,
        request: &'a HostProbeRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<AuthenticatedHostPeer>, HostReconciliationError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            Ok(vec![AuthenticatedHostPeer {
                session_id: request.session_id,
                host_instance_id: HostInstanceId::new(),
            }])
        })
    }
}

#[tokio::test]
async fn ambiguous_identity_preserves_metadata_and_unrelated_sentinel() {
    let fixture = tempfile::tempdir().unwrap();
    let runtime_root = fixture.path().join("runtime");
    let session_dir = fixture.path().join("session");
    let sentinel = fixture.path().join("unrelated-process-identity");
    std::fs::write(&sentinel, b"must-remain-identical").unwrap();
    let session_id = HostedSessionId::new();
    let host_instance_id = HostInstanceId::new();
    let lease = HostLease::acquire(&session_dir, host_instance_id).unwrap();
    lease
        .write_metadata(&HostMetadata {
            format_version: HostMetadata::FORMAT_VERSION,
            session_id,
            host_instance_id,
            process_token: None,
            runtime_recognition: None,
            activity: ActivityAggregate::default(),
            lifecycle: HostLifecycle::Ready,
            endpoint_name: opaque_endpoint_name(session_id),
            heartbeat_monotonic_nanos: 8,
            durability_watermark: None,
        })
        .unwrap();
    let original = std::fs::read(session_dir.join("host.json")).unwrap();
    let service = HostReconciliationService::with_probe(runtime_root, Arc::new(WrongPeer));
    let plan = service.plan(&session_dir).await.unwrap();
    assert_eq!(plan.preview_result, RecoveryResult::Ambiguous);
    assert_eq!(
        service
            .reconcile(plan, &CancellationToken::new())
            .unwrap()
            .result,
        RecoveryResult::Ambiguous
    );
    assert_eq!(
        std::fs::read(session_dir.join("host.json")).unwrap(),
        original
    );
    assert_eq!(std::fs::read(sentinel).unwrap(), b"must-remain-identical");
    drop(lease);
}

#[test]
fn recovery_module_has_no_process_control_surface() {
    let source = include_str!("../../termirust-client/src/host_recovery.rs");
    for forbidden in [
        "libc::kill",
        "std::process",
        "Command::new",
        "ProcessToken::",
        "process.kill",
    ] {
        assert!(
            !source.contains(forbidden),
            "forbidden process API: {forbidden}"
        );
    }
}
