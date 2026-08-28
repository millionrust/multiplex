#![cfg(unix)]

use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;

use termirust_client::{
    SshControllerProcess, SshControllerTarget, SshControllerTargetId, ValidatedDnsOrIp,
    ValidatedUser, strict_ssh_command_argv,
};
use tokio_util::sync::CancellationToken;

fn target() -> SshControllerTarget {
    SshControllerTarget::new(
        SshControllerTargetId::new("fixture-target").unwrap(),
        ValidatedDnsOrIp::parse("host.example").unwrap(),
        Some(ValidatedUser::parse("operator").unwrap()),
        Some(2202),
    )
    .unwrap()
}

#[test]
fn exact_argv_ignores_hostile_configuration_and_keeps_remote_command_constant() {
    let expected = [
        "ssh",
        "-F",
        "none",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "LocalCommand=none",
        "-o",
        "ProxyCommand=none",
        "-o",
        "ProxyJump=none",
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-o",
        "RequestTTY=no",
        "-l",
        "operator",
        "-p",
        "2202",
        "host.example",
        "termirust",
        "controller-bridge",
        "--stdio",
    ];
    let actual = strict_ssh_command_argv(&target())
        .into_iter()
        .map(|value| value.into_string().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);

    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    fs::create_dir_all(home.join(".ssh")).unwrap();
    fs::write(
        home.join(".ssh/config"),
        "Host *\n  ProxyCommand touch /tmp/forbidden\n  LocalCommand false\n  ForwardAgent yes\n  RemoteCommand sh\n",
    )
    .unwrap();
    let recorder = fixture.path().join("fake-ssh");
    fs::write(&recorder, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    fs::set_permissions(&recorder, fs::Permissions::from_mode(0o700)).unwrap();
    let mut process = SshControllerProcess::spawn(&recorder, &target()).unwrap();
    let mut recorded = String::new();
    process
        .take_stdout()
        .unwrap()
        .read_to_string(&mut recorded)
        .unwrap();
    process
        .wait_with_cancellation(&CancellationToken::new())
        .unwrap();
    assert_eq!(recorded.lines().collect::<Vec<_>>(), &expected[1..]);
    assert!(!fixture.path().join("forbidden").exists());
}
