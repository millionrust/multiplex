use termirust_ui_contract::{AccessibilityLabModel, AccessibilityLabNode};

fn main() {
    let model = AccessibilityLabModel::try_new(Default::default())
        .expect("default accessibility laboratory model must be valid");
    let snapshot = model
        .semantic_snapshot()
        .expect("accessibility laboratory snapshot must serialize");
    assert!(!snapshot.contains(model.secret_canary()));
    assert!(snapshot.contains(&format!(
        "{} parent=3 role=text_field",
        AccessibilityLabNode::Field.semantic_id().get()
    )));
    print!("{snapshot}");
}
