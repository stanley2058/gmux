use std::collections::BTreeSet;

const MATRIX: &str = include_str!("fixtures/terminal_feature_matrix.tsv");

#[test]
fn terminal_feature_matrix_covers_required_features() {
    let mut lines = MATRIX.lines();
    assert_eq!(
        lines.next(),
        Some("feature\tautomated_check\tmanual_local\tmanual_remote\tnotes")
    );

    let rows = lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let columns: Vec<_> = line.split('\t').collect();
            assert_eq!(columns.len(), 5, "invalid matrix row: {line}");
            for column in &columns {
                assert!(!column.trim().is_empty(), "empty matrix field: {line}");
            }
            columns[0]
        })
        .collect::<BTreeSet<_>>();

    let required = BTreeSet::from([
        "osc52_clipboard",
        "osc8_hyperlinks",
        "kitty_keyboard",
        "kitty_graphics",
        "bracketed_paste",
        "mouse_mode",
        "focus_events",
        "cursor_shape",
        "synchronized_output",
        "wide_graphemes",
    ]);

    assert_eq!(rows, required);
}
