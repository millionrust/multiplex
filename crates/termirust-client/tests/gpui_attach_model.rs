use termirust_client::{AttachPhase, GpuiAttachModel, OutputDisposition};
use termirust_domain::OutputSequence;

#[test]
fn replay_is_ordered_deduplicated_and_transitions_to_live() {
    let mut model = GpuiAttachModel::new(OutputSequence::ZERO);
    model.begin_replay();

    for sequence in 1..=1_000 {
        assert_eq!(
            model.observe_output(OutputSequence::new(sequence), 4),
            OutputDisposition::Deliver
        );
    }
    assert_eq!(
        model.observe_output(OutputSequence::new(1_000), 4),
        OutputDisposition::Duplicate
    );
    model.mark_live(true, false);

    assert_eq!(model.phase(), AttachPhase::Live);
    assert_eq!(model.watermark(), OutputSequence::new(1_000));
    assert_eq!(model.replayed_records(), 1_000);
    assert_eq!(model.replayed_bytes(), 4_000);
    assert!(model.has_writer_lease());
}

#[test]
fn snapshot_advances_watermark_and_a_real_gap_is_truthful() {
    let mut model = GpuiAttachModel::new(OutputSequence::new(10));
    assert!(model.apply_snapshot(OutputSequence::new(50)));
    assert_eq!(
        model.observe_output(OutputSequence::new(52), 1),
        OutputDisposition::Gap {
            expected: OutputSequence::new(51),
            received: OutputSequence::new(52),
        }
    );
    assert_eq!(model.phase(), AttachPhase::Gap);
    assert!(!model.apply_snapshot(OutputSequence::new(49)));
}

#[test]
fn detach_and_host_exit_are_distinct_states() {
    let mut model = GpuiAttachModel::new(OutputSequence::new(7));
    model.mark_live(true, false);
    model.mark_offline();
    assert_eq!(model.phase(), AttachPhase::Offline);
    assert_eq!(model.watermark(), OutputSequence::new(7));

    model.mark_dead();
    assert_eq!(model.phase(), AttachPhase::Dead);
    assert_eq!(model.watermark(), OutputSequence::new(7));
}
