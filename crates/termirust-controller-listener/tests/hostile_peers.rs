use std::net::IpAddr;

use termirust_controller_listener::{
    AuthRateLimiter, BoundedFrameQueue, ListenerErrorCode, QueueClass, SourceBucket,
    SourceBucketKey, read_bounded_frame,
};
use termirust_domain::ConnectionBudget;

#[test]
fn queues_reject_frame_and_count_or_byte_overflow_without_partial_enqueue() {
    let budget = ConnectionBudget::default();
    let mut queue = BoundedFrameQueue::new(budget).unwrap();
    for _ in 0..64 {
        queue.push(QueueClass::Control, vec![1]).unwrap();
    }
    assert_eq!(queue.len(), 64);
    assert_eq!(
        queue.push(QueueClass::Control, vec![1]).unwrap_err().code,
        ListenerErrorCode::QueueFull
    );
    assert_eq!(queue.len(), 64);
    assert_eq!(queue.payload_bytes(), 64);

    let mut queue = BoundedFrameQueue::new(budget).unwrap();
    queue
        .push(
            QueueClass::Terminal,
            vec![0; budget.max_terminal_frame_bytes + 1],
        )
        .unwrap_err();
    assert!(queue.is_empty());
}

#[test]
fn source_buckets_are_redacted_and_five_failures_rate_limit_only_that_bucket() {
    let key = SourceBucketKey::from_random([7; 32]);
    let first = SourceBucket::derive(&key, "192.168.1.20".parse::<IpAddr>().unwrap());
    let second = SourceBucket::derive(&key, "192.168.1.21".parse::<IpAddr>().unwrap());
    assert_eq!(format!("{first:?}"), "SourceBucket([REDACTED])");

    let mut limiter = AuthRateLimiter::new(ConnectionBudget::default()).unwrap();
    for attempt in 0..4 {
        limiter.record_failure(first, attempt).unwrap();
    }
    assert_eq!(
        limiter.record_failure(first, 4).unwrap_err().code,
        ListenerErrorCode::RateLimited
    );
    assert_eq!(
        limiter.check(first, 5).unwrap_err().code,
        ListenerErrorCode::RateLimited
    );
    assert!(limiter.check(second, 5).is_ok());
    assert!(limiter.check(first, 604).is_ok());
}

#[tokio::test]
async fn length_prefix_rejects_zero_and_oversize_before_allocating_payload() {
    let mut zero = &0_u32.to_be_bytes()[..];
    assert_eq!(
        read_bounded_frame(&mut zero, 64).await.unwrap_err().code,
        ListenerErrorCode::MalformedFrame
    );
    let mut oversized = &65_u32.to_be_bytes()[..];
    assert_eq!(
        read_bounded_frame(&mut oversized, 64)
            .await
            .unwrap_err()
            .code,
        ListenerErrorCode::FrameTooLarge
    );
}
