use super::*;

#[test]
fn fresh_conversation_resets_history_to_welcome_and_home() {
    let mut tui = Tui::test_tui(80, 30);
    tui.history_entries = vec![TranscriptEntry::Welcome];

    tui.push_user_prompt("hello", &[], true);
    tui.push_action("some work", None);
    tui.left_home = true;
    assert!(!tui.is_home());

    tui.start_fresh_conversation();

    assert!(
        matches!(tui.history_entries.as_slice(), [TranscriptEntry::Welcome]),
        "history must be exactly [Welcome]: {:?}",
        tui.history_entries
    );
    assert!(tui.is_home());
}

#[test]
fn home_command_reply_is_a_transient_toast() {
    let mut tui = Tui::test_tui(80, 30);
    tui.history_entries = vec![TranscriptEntry::Welcome];

    tui.push_action("theme set", None);
    assert!(matches!(tui.history_entries.as_slice(), [TranscriptEntry::Welcome]));
    assert_eq!(tui.hint_active(), Some("✓ theme set"));

    tui.left_home = true;
    tui.push_action("theme set", None);
    assert!(tui.history_entries.len() > 1, "off home it is a transcript row");
}

#[test]
fn home_dim_line_is_a_toast() {
    let mut tui = Tui::test_tui(80, 30);
    tui.history_entries = vec![TranscriptEntry::Welcome];
    tui.push_dim("ctrl-c again to quit".into());
    assert!(matches!(tui.history_entries.as_slice(), [TranscriptEntry::Welcome]));
    assert_eq!(tui.hint_active(), Some("ctrl-c again to quit"));
}
