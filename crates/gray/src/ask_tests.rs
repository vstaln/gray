use super::*;

fn q(id: &str, options: usize) -> AskQuestion {
    AskQuestion {
        id: id.into(),
        header: "H".into(),
        question: "q?".into(),
        options: (0..options)
            .map(|i| AskOption {
                label: format!("o{i}"),
                description: "d".into(),
                opt_in: false,
                freeform: false,
            })
            .collect(),
        detail: String::new(),
        multi: false,
        default_all: false,
        no_other: false,
    }
}

#[test]
fn parse_rejects_shape_but_allows_empty() {
    assert!(parse_params(&serde_json::json!({})).unwrap().0.is_empty());
    assert!(
        parse_params(&serde_json::json!({"questions": []}))
            .unwrap()
            .0
            .is_empty()
    );
    assert!(parse_params(&serde_json::json!({"questions": [1]})).is_err());
    let four: Vec<_> = (0..4).map(|i| q(&format!("q{i}"), 1)).collect();
    let v = serde_json::json!({"questions": four});
    assert!(parse_params(&v).is_err());
    let (qs, blocking) =
        parse_params(&serde_json::json!({"questions": [q("a", 2)], "blocking": false})).unwrap();
    assert_eq!(qs.len(), 1);
    assert!(!blocking);
}

#[test]
fn answers_json_shape() {
    let out = answers_json(&[AskAnswer {
        id: "mode".into(),
        answers: vec!["fast".into(), "user_note: hurry".into()],
    }]);
    assert_eq!(out["answers"]["mode"]["answers"][0], "fast");
    assert_eq!(out["answers"]["mode"]["answers"][1], "user_note: hurry");
}

#[test]
fn modal_finish_preselects_first_option() {
    let mut st = AskModalState::new(1);
    st.load(vec![q("a", 2)]);

    let answers = st.finish();
    assert_eq!(answers[0].answers, vec!["o0".to_string()]);
}

#[test]
fn modal_finish_uses_cursor_selection() {
    let mut st = AskModalState::new(1);
    st.load(vec![q("a", 2)]);
    st.cursor[0] = 1;
    let answers = st.finish();
    assert_eq!(answers[0].answers, vec!["o1".to_string()]);
}

#[test]
fn modal_finish_other_row_yields_note_or_other() {
    let mut st = AskModalState::new(1);
    st.load(vec![q("a", 2)]);
    st.cursor[0] = 2;
    let answers = st.finish();
    assert_eq!(answers[0].answers, vec!["Other".to_string()]);

    let mut st = AskModalState::new(1);
    st.load(vec![q("a", 2)]);
    st.cursor[0] = 2;
    st.notes[0] = "ship it friday".to_string();
    let answers = st.finish();
    assert_eq!(
        answers[0].answers,
        vec!["user_note: ship it friday".to_string()]
    );
}

#[test]
fn modal_finish_appends_notes() {
    let mut st = AskModalState::new(1);
    st.load(vec![q("a", 2)]);
    st.notes[0] = "hurry".to_string();
    let answers = st.finish();
    assert_eq!(
        answers[0].answers,
        vec!["o0".to_string(), "user_note: hurry".to_string()]
    );
}

#[test]
fn ask_rows_shape() {
    let mut st = AskModalState::new(1);
    st.load(vec![q("a", 2)]);
    let rows = ask_rows(&st);

    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].0, "?");
    assert_eq!(rows[1].0, "\u{25cf}");
    assert_eq!(rows[1].1, "o0");
    assert_eq!(rows[2].0, "\u{25cb}");
    assert_eq!(rows[3].1, "Other");
    assert_eq!(rows[4].0, "\u{b7}");
}

#[test]
fn ask_rows_detail_and_cursor_offset() {
    let mut question = q("a", 2);
    question.detail = "gray mcp list".to_string();
    let mut st = AskModalState::new(1);
    st.load(vec![question]);
    st.cursor[0] = 1;
    let rows = ask_rows(&st);

    assert_eq!(rows.len(), 6);
    assert_eq!(rows[1].0, "\u{2502}");
    assert_eq!(rows[1].2, "gray mcp list");
    assert_eq!(st.cursor_row(), 3);
}

#[test]
fn ask_rows_no_other_drops_extra_row() {
    let mut question = q("a", 2);
    question.no_other = true;
    let mut st = AskModalState::new(1);
    st.load(vec![question]);
    let rows = ask_rows(&st);

    assert_eq!(rows.len(), 4);
    assert_eq!(st.options_len(), 2);
}

static ASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn handle_ask_empty_and_nonblocking_resolve_empty() {
    let _guard = ASK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::ask::install(None, false);
    let r = crate::ask::handle_ask(serde_json::json!({"questions": []})).await;
    assert_eq!(r, serde_json::json!({"answers": {}}));
    let r = crate::ask::handle_ask(
        serde_json::json!({"questions": [{"id":"a","header":"H","question":"q?","options":[{"label":"x","description":"y"}]}], "blocking": false}),
    )
    .await;
    assert_eq!(r, serde_json::json!({"answers": {}}));
    crate::ask::shutdown();
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn handle_ask_rejects_bad_shape() {
    let _guard = ASK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::ask::install(None, false);
    let r = crate::ask::handle_ask(serde_json::json!({"questions": [1]})).await;
    assert!(r.get("error").is_some(), "{r}");
    crate::ask::shutdown();
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn handle_ask_without_service_errors_loudly() {
    let _guard = ASK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::ask::shutdown();
    let _ = crate::ask::handle_ask(serde_json::json!({"questions": []})).await;

    let r = crate::ask::handle_ask(
        serde_json::json!({"questions": [{"id":"a","header":"H","question":"q?","options":[{"label":"x","description":"y"}]}]}),
    )
    .await;
    assert!(r.get("error").is_some(), "{r}");
}

#[test]
fn json_wire_asks_with_a_row_and_reads_answers_by_id() {
    let questions = vec![AskQuestion {
        id: "branch".into(),
        header: "Branch".into(),
        question: "Which branch?".into(),
        options: vec![AskOption {
            label: "main".into(),
            description: "production".into(),
            opt_in: false,
            freeform: false,
        }],
        detail: String::new(),
        multi: false,
        default_all: false,
        no_other: false,
    }];
    let row = json_ask_row("t1", 7, &questions);
    assert_eq!(row["protocol"], 1);
    assert_eq!(row["phase"], "ask");
    assert_eq!(row["ask_id"], 7);
    assert_eq!(row["questions"][0]["id"], "branch");
    assert!(
        parse_json_answer(
            r#"{"ask_id":8,"answers":{"branch":["main"]}}"#,
            7,
            &questions
        )
        .is_none()
    );
    let answers = parse_json_answer(
        r#"{"ask_id":7,"answers":{"branch":["main"],"other":["x"]}}"#,
        7,
        &questions,
    )
    .unwrap();
    assert_eq!(
        answers,
        vec![AskAnswer {
            id: "branch".into(),
            answers: vec!["main".into()]
        }]
    );
    assert!(parse_json_answer("not json", 7, &questions).is_none());
}

#[test]
fn opt_in_option_starts_unchecked_under_default_all() {
    let mut q = q("x", 3);
    q.multi = true;
    q.default_all = true;
    q.options[2].opt_in = true;
    let mut state = AskModalState::new(1);
    state.load(vec![q]);
    assert_eq!(state.checked[0], vec![true, true, false]);
}

#[test]
fn deliver_json_answer_drops_unknown_id_and_long_lines() {
    let _guard = ASK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let long = format!(
        "{{\"ask_id\":999,\"answers\":{{\"a\":[\"{}\"]}}}}",
        "é".repeat(500)
    );
    crate::ask::shutdown();
    crate::ask::deliver_json_answer(999, &long);
    crate::ask::install(None, false);
    crate::ask::deliver_json_answer(999, &long);
    crate::ask::deliver_json_answer(999, "");
    crate::ask::shutdown();
}
