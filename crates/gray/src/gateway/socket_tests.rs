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
fn unknown_verbs_still_list_all_four() {
    let home = tempfile::tempdir().unwrap();
    let line = handle_request_line(home.path(), br#"{"verb":"nope"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(false));
    for verb in ["identify", "status", "metrics", "logs_tail"] {
        assert!(
            answer["supported_verbs"].to_string().contains(verb),
            "{answer}"
        );
    }
}
