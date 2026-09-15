use std::time::Duration;
use termirust_relay_client::{
    MutationDisposition, MutationReconciliation, RelayOperationClass, RelayReconnectDecision,
    RelayReconnectPolicy,
};

#[test]
fn reads_stop_after_eight_attempts_or_ninety_seconds() {
    let policy = RelayReconnectPolicy::default();
    assert!(matches!(
        policy.decide(
            RelayOperationClass::IdempotentRead,
            7,
            Duration::from_secs(89),
            42
        ),
        RelayReconnectDecision::RetryAfter(_)
    ));
    assert_eq!(
        policy.decide(
            RelayOperationClass::IdempotentRead,
            8,
            Duration::from_secs(10),
            42
        ),
        RelayReconnectDecision::Exhausted
    );
    assert_eq!(
        policy.decide(
            RelayOperationClass::IdempotentRead,
            1,
            Duration::from_secs(90),
            42
        ),
        RelayReconnectDecision::Exhausted
    );
}

#[test]
fn mutation_with_lost_ack_is_unknown_and_never_retried() {
    assert_eq!(
        RelayReconnectPolicy::default().decide(RelayOperationClass::Mutation, 0, Duration::ZERO, 0),
        RelayReconnectDecision::UnknownCompletion
    );
    let mut reconciliation = MutationReconciliation::default();
    reconciliation.sent(41_u64);
    reconciliation.sent(42_u64);
    reconciliation.acknowledged(41);
    reconciliation.disconnected();
    assert_eq!(
        reconciliation.disposition(41),
        Some(MutationDisposition::Acknowledged)
    );
    assert_eq!(
        reconciliation.disposition(42),
        Some(MutationDisposition::UnknownCompletion)
    );
}
