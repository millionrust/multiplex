#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::process::{Command, Stdio};
use std::time::Duration;

use termirust_client::{
    SshControllerErrorCode, SshControllerProcess, SshControllerTarget, SshControllerTargetId,
    SshOperationClass, SshReconnectDecision, SshReconnectPolicy, ValidatedDnsOrIp,
};
use tokio_util::sync::CancellationToken;

fn target() -> SshControllerTarget {
    SshControllerTarget::new(
        SshControllerTargetId::new("cancel-target").unwrap(),
        ValidatedDnsOrIp::parse("127.0.0.1").unwrap(),
        None,
        None,
    )
    .unwrap()
}

#[test]
fn cancellation_terminates_only_the_owned_ssh_process_group() {
    let fixture = tempfile::tempdir().unwrap();
    let fake_ssh = fixture.path().join("fake-ssh");
    fs::write(
        &fake_ssh,
        "#!/bin/sh\ntrap 'exit 0' TERM INT\nwhile :; do sleep 0.05; done\n",
    )
    .unwrap();
    fs::set_permissions(&fake_ssh, fs::Permissions::from_mode(0o700)).unwrap();

    let mut unrelated = Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut owned = SshControllerProcess::spawn(&fake_ssh, &target()).unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = owned.wait_with_cancellation(&cancellation).unwrap_err();
    assert_eq!(error.code, SshControllerErrorCode::Cancelled);
    assert!(unrelated.try_wait().unwrap().is_none());
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[test]
fn reconnect_budget_never_retries_mutations_or_exceeds_bounds() {
    let policy = SshReconnectPolicy::default();
    assert_eq!(
        policy.decide(SshOperationClass::Mutation, 0, Duration::ZERO, 7),
        SshReconnectDecision::Stop
    );
    for attempt in 0..8 {
        let decision = policy.decide(
            SshOperationClass::IdempotentRead,
            attempt,
            Duration::from_secs(1),
            u64::MAX - u64::from(attempt),
        );
        let SshReconnectDecision::RetryAfter(delay) = decision else {
            panic!("attempt {attempt} should remain retryable")
        };
        assert!(delay <= Duration::from_secs(10));
    }
    assert_eq!(
        policy.decide(
            SshOperationClass::IdempotentRead,
            8,
            Duration::from_secs(1),
            0,
        ),
        SshReconnectDecision::Stop
    );
}
