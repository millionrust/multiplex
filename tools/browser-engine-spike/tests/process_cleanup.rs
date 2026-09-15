use std::path::Path;
use termirust_browser_engine_spike::run_process_cleanup_probe;

#[test]
fn cancellation_terminates_only_the_owned_process_group_and_cleans_profile() {
    let root = std::env::temp_dir().join(format!(
        "termirust-browser-process-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let probe =
        run_process_cleanup_probe(Path::new(env!("CARGO_BIN_EXE_browser-engine-spike")), &root)
            .unwrap();
    assert!(probe.empty_child_environment);
    assert!(probe.owned_process_group_verified);
    assert!(probe.unrelated_process_survived);
    assert!(probe.descendant_terminated);
    assert!(probe.temporary_profile_removed);
    std::fs::remove_dir_all(root).unwrap();
}
