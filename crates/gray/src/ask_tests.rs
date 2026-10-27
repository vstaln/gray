use super::{AskModalState, AskOption, AskQuestion, ask_rows};

fn opt(label: &str, description: &str) -> AskOption {
    AskOption {
        label: label.into(),
        description: description.into(),
        opt_in: false,
        freeform: false,
        scopes: Vec::new(),
        muted: false,
    }
}

fn question(question: &str, detail: &str, no_other: bool) -> AskQuestion {
    AskQuestion {
        id: "q".into(),
        header: "Allow?".into(),
        question: question.into(),
        detail: detail.into(),
        options: vec![opt("Yes", "Run this once."), opt("No", "Skip it.")],
        multi: false,
        default_all: false,
        no_other,
        editable: false,
    }
}

fn kinds(rows: &[(String, String, String)]) -> Vec<&str> {
    rows.iter().map(|(k, ..)| k.as_str()).collect()
}

#[test]
fn approval_question_keeps_its_lines_and_skips_other() {
    let mut st = AskModalState::new(1);
    st.load(vec![question(
        "Allow bash?\necho one\necho two",
        "in /tmp/x",
        true,
    )]);
    let (rows, cursor) = ask_rows(&st);
    assert_eq!(
        kinds(&rows),
        ["?", ">", ">", "\u{2502}", " ", "1.", "2.", " ", "\u{b7}"]
    );
    assert_eq!(rows[0].1, "Allow bash?");
    assert_eq!(rows[2].2, "echo two");
    assert_eq!(rows[cursor].1, "Yes");
    assert!(rows.last().unwrap().2.contains("1-2 choose"));
}

#[test]
fn plain_questions_still_offer_other() {
    let mut st = AskModalState::new(1);
    st.load(vec![question("Which branch?", "", false)]);
    st.cursor[0] = 2;
    let (rows, cursor) = ask_rows(&st);
    assert_eq!(kinds(&rows), ["?", " ", "1.", "2.", "3.", " ", "\u{b7}"]);
    assert_eq!(rows[cursor].1, "Other");
    assert_eq!(rows[cursor].2, "type a note");
}

#[test]
fn editable_question_prefills_subject_and_marks_edited() {
    let mut st = AskModalState::new(1);
    let mut q = question("Allow bash?\ngit status", "in /tmp/x", true);
    q.editable = true;
    st.load(vec![q]);
    assert_eq!(super::edit_subject(&st.questions[0]), "git status");
}
