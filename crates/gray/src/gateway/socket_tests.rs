use super::*;

#[test]
fn identify_answers_about_this_process_and_misses_list_verbs() {
    let home = tempfile::tempdir().unwrap();
    let line = handle_request_line(
        home.path(),
        br#"{"id":7,"verb":"identify","protocol":1}"#,
        1000,
    );
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(true));
    assert_eq!(answer["protocol"], serde_json::json!(1));
    assert_eq!(answer["id"], serde_json::json!(7));
    assert_eq!(answer["result"]["kind"], serde_json::json!("gray-gateway"));
    assert_eq!(
        answer["result"]["pid"],
        serde_json::json!(std::process::id())
    );

    let line = handle_request_line(home.path(), br#"{"verb":"nope"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(false));
    assert!(
        answer["supported_verbs"].to_string().contains("identify"),
        "{answer}"
    );
}

#[test]
fn status_reports_cron_fields_even_on_an_empty_home() {
    let home = tempfile::tempdir().unwrap();
    let line = handle_request_line(home.path(), br#"{"verb":"status"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(true));
    assert_eq!(
        answer["result"]["cron"],
        serde_json::json!({
            "ticker_live": false,
            "last_tick_at": null,
            "last_tick_kind": null,
            "last_tick_secs_ago": null,
            "overdue": 0,
            "jobs": 0,
        }),
        "full cron payload: {}",
        answer["result"]["cron"]
    );
    assert_eq!(
        answer["result"]["answering_pid"],
        serde_json::json!(std::process::id())
    );
}

#[tokio::test]
#[cfg(unix)]
async fn serve_answers_queries_and_cleans_up_on_stop() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().to_path_buf();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(serve(dir.clone(), rx));
    let sock = sock_path(&dir);
    for _ in 0..200 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(sock.exists(), "socket never appeared");
    let result = tokio::task::spawn_blocking({
        let dir = dir.clone();
        move || query(&dir, "identify")
    })
    .await
    .unwrap();
    assert_eq!(
        result.expect("identify answered")["kind"],
        serde_json::json!("gray-gateway")
    );
    tx.send(true).unwrap();
    task.await.unwrap().unwrap();
    assert!(!sock.exists(), "socket file survived shutdown");
    assert!(query(&dir, "identify").is_none());
}

#[test]
fn query_without_a_socket_is_none() {
    let home = tempfile::tempdir().unwrap();
    assert!(query(home.path(), "identify").is_none());
}
#[test]
fn metrics_reports_existing_state_shapes() {
    let home = tempfile::tempdir().unwrap();
    let line = handle_request_line(home.path(), br#"{"id":3,"verb":"metrics"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(true));
    assert_eq!(answer["protocol"], serde_json::json!(1));
    assert_eq!(answer["id"], serde_json::json!(3));
    assert!(answer["result"]["uptime_secs"].is_number(), "{answer}");
    assert!(answer["result"]["cron"].is_object(), "{answer}");
    assert_eq!(answer["result"]["cron"]["jobs"], serde_json::json!(0));
}

#[test]
fn logs_tail_is_empty_without_a_log_file() {
    let home = tempfile::tempdir().unwrap();
    let line = handle_request_line(home.path(), br#"{"verb":"logs_tail"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(true));
    assert_eq!(answer["result"]["lines"], serde_json::json!([]));
    assert_eq!(
        answer["result"]["total_lines_available"],
        serde_json::json!(0)
    );
}

#[test]
fn logs_tail_returns_last_lines_capped() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("logs");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for i in 0..250 {
        body.push_str(&format!("line {i}\n"));
    }
    std::fs::write(dir.join("gray.log"), &body).unwrap();
    let line = handle_request_line(home.path(), br#"{"verb":"logs_tail"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    let lines = answer["result"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 200);
    assert_eq!(lines[0], serde_json::json!("line 50"));
    assert_eq!(lines[199], serde_json::json!("line 249"));
    assert_eq!(
        answer["result"]["total_lines_available"],
        serde_json::json!(250)
    );
}

#[test]
fn unknown_verbs_still_list_every_verb() {
    let home = tempfile::tempdir().unwrap();
    let line = handle_request_line(home.path(), br#"{"verb":"nope"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(false));
    for verb in SUPPORTED_VERBS {
        assert!(
            answer["supported_verbs"].to_string().contains(verb),
            "{answer}"
        );
    }
}

#[test]
fn logs_tail_reports_the_real_total_after_the_byte_cap() {
    // Audit #21: a log far bigger than the 64KiB read cap must still report
    // the file's total line count, not the retained tail's.
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let path = logs.join("gray.log");
    let filler = "x".repeat(2048);
    let mut body = String::new();
    for i in 0..800 {
        body.push_str(&format!("{i:05} {filler}\n"));
    }
    std::fs::write(&path, &body).unwrap();
    assert!(body.len() as u64 > super::LOGS_TAIL_MAX_BYTES);
    let (kept, total) = super::tail_gray_log(dir.path());
    assert_eq!(total, 800, "the total must count the whole file");
    assert!(kept.len() <= super::LOGS_TAIL_LINES);
    assert!(!kept.is_empty());
}

#[test]
fn pull_then_ack_drains_a_platform() {
    let home = tempfile::tempdir().unwrap();
    let dir = super::super::state_dir(home.path());
    let route = crate::cron::store::Origin {
        platform: "discord".into(),
        chat: "42".into(),
        thread: None,
        route: Some("42".into()),
    };
    let i = super::super::outbox::Intent::new(
        "main",
        super::super::event::Kind::User,
        Some(route),
        "hi",
    );
    super::super::outbox::enqueue(&dir, &i).unwrap();
    let ask = |raw: &[u8]| -> serde_json::Value {
        serde_json::from_slice(&handle_request_line(home.path(), raw, 1000)).unwrap()
    };
    let pulled = ask(br#"{"verb":"pull","platform":"discord"}"#);
    assert_eq!(pulled["result"]["intents"][0]["text"], "hi", "{pulled}");
    assert_eq!(ask(br#"{"verb":"pull"}"#)["ok"], false);
    let acked = ask(format!(r#"{{"verb":"ack","ids":["{}"]}}"#, i.id).as_bytes());
    assert_eq!(acked["result"]["acked"], 1, "{acked}");
}

#[test]
fn identify_and_status_never_carry_the_process_argv() {
    // argv holds the turn's `-p <prompt>` text; it must not leave the process.
    let home = tempfile::tempdir().unwrap();
    for req in [
        br#"{"id":1,"verb":"identify","protocol":1}"#.as_slice(),
        br#"{"id":2,"verb":"status","protocol":1}"#.as_slice(),
    ] {
        let line = handle_request_line(home.path(), req, 1000);
        let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(answer["ok"], serde_json::json!(true), "{answer}");
        assert!(answer["result"].get("argv").is_none(), "{answer}");
    }
}
