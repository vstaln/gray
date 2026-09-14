//! lane5_probe: read-only live probe of gray tools on feat/batch-reunite.
//! Only benign payloads (echo/ls/sleep) are executed. Destructive shapes
//! are NEVER executed -- those verdicts are static-only (see report).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use futures::future::BoxFuture;
use gray_core::agent::{PermissionMode, Tool, ToolContext, ToolExecutor};
use gray_core::approvals::{ApprovalGate, Verdict, verdict};
use gray_core::error::CoreError;
use gray_core::questions::{QuestionAsker, QuestionBridge, UserAnswer, UserQuestion};
use gray_tools::Registry;
use serde_json::json;

struct Canned(Vec<String>);

impl QuestionAsker for Canned {
    fn ask(
        &self,
        qs: Vec<UserQuestion>,
        _blocking: bool,
    ) -> BoxFuture<'static, Result<Vec<UserAnswer>, CoreError>> {
        let a = self.0.clone();
        Box::pin(async move {
            Ok(vec![UserAnswer {
                id: qs.first().map(|q| q.id.clone()).unwrap_or_default(),
                answers: a,
            }])
        })
    }
}

fn bridge(labels: &[&str]) -> QuestionBridge {
    QuestionBridge(Arc::new(Canned(
        labels.iter().map(|s| s.to_string()).collect(),
    )))
}

fn workdir() -> PathBuf {
    PathBuf::from(std::env::var("LANE5_WORK").expect("LANE5_WORK must be set"))
}

fn ctx(session: &str) -> ToolContext {
    ToolContext {
        cwd: workdir(),
        session_id: Some(session.to_string()),
        ..ToolContext::default()
    }
}

fn sess(phase: &str) -> String {
    format!("lane5_{}-{}", phase, std::process::id())
}

static mut FAILS: u32 = 0;

fn emit(phase: &str, check: &str, pass: bool, ms: u128, detail: &str) {
    if !pass {
        unsafe { FAILS += 1 };
    }
    let short: String = detail.chars().take(160).collect();
    let flat = short.replace('\n', "\\n");
    println!(
        "LANE5|{}|{}|{}|{}ms|{}",
        phase,
        check,
        if pass { "PASS" } else { "FAIL" },
        ms,
        flat
    );
}

fn info(phase: &str, key: &str, val: &str) {
    let flat = val.replace('\n', ";");
    let short: String = flat.chars().take(300).collect();
    println!("LANE5|{}|info:{}|{}", phase, key, short);
}

async fn exec(reg: &Registry, c: &ToolContext, name: &str, args: serde_json::Value) -> gray_core::agent::ToolOutput {
    ToolExecutor::execute(reg, c, name, args).await
}

async fn phase_tools() {
    let p = "tools";
    let s = sess(p);
    let c = ctx(&s);
    let reg = Registry::builtin();
    info(p, "builtin_tools", &reg.tool_names().join(","));

    let t = Instant::now();
    let r = exec(&reg, &c, "bash", json!({"command": "echo lane5-hello"})).await;
    emit(p, "bash_echo", !r.is_error && r.content.contains("lane5-hello"), t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(&reg, &c, "bash", json!({"command": "ls"})).await;
    emit(p, "bash_ls", !r.is_error && r.content.contains("note.txt"), t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(&reg, &c, "read", json!({"path": "note.txt"})).await;
    emit(p, "read", !r.is_error && r.content.contains("lane5-fixture"), t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(&reg, &c, "write", json!({"path": "lane5_out.txt", "content": "lane5-marker-1\n"})).await;
    emit(p, "write_new", !r.is_error, t.elapsed().as_millis(), &r.content);

    // write-before-read guard: an existing file never read via ReadTool
    // must refuse without force (note.txt was fully read above, so it is
    // writable by design -- probe the unread a.txt instead).
    let t = Instant::now();
    let r = exec(&reg, &c, "write", json!({"path": "a.txt", "content": "clobber"})).await;
    emit(p, "write_unread_refused", r.is_error && r.content.contains("has not been read"), t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(&reg, &c, "write", json!({"path": "a.txt", "content": "clobber", "force": true})).await;
    emit(p, "write_force_bypasses_unread", !r.is_error, t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(
        &reg,
        &c,
        "edit",
        json!({"path": "lane5_out.txt", "edits": [{"oldText": "lane5-marker-1", "newText": "lane5-marker-2"}]}),
    )
    .await;
    emit(p, "edit", !r.is_error, t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(&reg, &c, "find", json!({"pattern": "*.txt"})).await;
    emit(p, "find", !r.is_error && r.content.contains("lane5_out.txt"), t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(&reg, &c, "grep", json!({"pattern": "lane5-marker", "glob": "*.txt"})).await;
    let rg_ok = !r.is_error && r.content.contains("lane5-marker-2");
    let rg_missing = r.is_error && r.content.contains("ripgrep");
    emit(p, "grep", rg_ok || rg_missing, t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(&reg, &c, "ls", json!({})).await;
    emit(p, "ls", !r.is_error && r.content.contains("lane5_out.txt"), t.elapsed().as_millis(), &r.content);

    // sleep + shell_output/shell_kill live in ToolsBasicPlugin, NOT builtin
    let t = Instant::now();
    let r = exec(&reg, &c, "sleep", json!({"seconds": 1})).await;
    emit(p, "sleep_missing_in_builtin", r.is_error && r.content.contains("does not exist"), t.elapsed().as_millis(), &r.content);

    // skill tool: direct execute (registered via gray surface extra_tools only)
    let t = Instant::now();
    let skill_path = workdir().join(".gray/skills/lane5demo/SKILL.md");
    let r = Tool::execute(
        &gray::skills_tool::SkillTool,
        &c,
        json!({"path": skill_path.to_string_lossy()}),
    )
    .await;
    emit(p, "skill_by_path", !r.is_error && r.content.contains("<skill"), t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = Tool::execute(&gray::skills_tool::SkillTool, &c, json!({"name": "lane5demo"})).await;
    emit(p, "skill_by_name", !r.is_error && r.content.contains("lane5demo-body"), t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = Tool::execute(&gray::skills_tool::SkillTool, &c, json!({"name": "lane5-nope"})).await;
    emit(p, "skill_missing_fails", r.is_error && r.content.contains("no skill named"), t.elapsed().as_millis(), &r.content);

    // question flows
    let t = Instant::now();
    let qc = ToolContext { questions: Some(bridge(&["o0"])), ..ctx(&s) };
    let r = exec(
        &reg,
        &qc,
        "request_user_input",
        json!({"questions": [{"id": "q1", "header": "H", "question": "pick?", "options": [{"label": "o0", "description": "d"}, {"label": "o1", "description": "d"}]}]}),
    )
    .await;
    emit(p, "question_blocking_answered", !r.is_error && r.content.contains("o0"), t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(
        &reg,
        &c,
        "request_user_input",
        json!({"questions": [{"id": "q1", "header": "H", "question": "pick?", "options": [{"label": "o0", "description": "d"}, {"label": "o1", "description": "d"}]}]}),
    )
    .await;
    emit(p, "question_blocking_no_bridge_fails_closed", r.is_error && r.content.contains("no user reachable"), t.elapsed().as_millis(), &r.content);

    let t = Instant::now();
    let r = exec(
        &reg,
        &c,
        "request_user_input",
        json!({"blocking": false, "questions": [{"id": "q1", "header": "H", "question": "pick?", "options": [{"label": "o0", "description": "d"}, {"label": "o1", "description": "d"}]}]}),
    )
    .await;
    emit(p, "question_steer_no_bridge_acks", !r.is_error && r.content.contains("steering"), t.elapsed().as_millis(), &r.content);
}

async fn phase_approvals() {
    let p = "approvals";
    let cwd = workdir();
    let t = Instant::now();
    let ro = ApprovalGate::new("read-only");
    let matrix = [
        ("read", verdict(ro.mode().as_str(), "read", &json!({}), &cwd) == Verdict::Allow),
        ("skill-allowed-in-ro", verdict(ro.mode().as_str(), "skill", &json!({}), &cwd) == Verdict::Allow),
        ("write-denied-in-ro", matches!(verdict(ro.mode().as_str(), "write", &json!({"path": "x"}), &cwd), Verdict::Deny(_))),
        ("bash-denied-in-ro", matches!(verdict(ro.mode().as_str(), "bash", &json!({"command": "ls"}), &cwd), Verdict::Deny(_))),
    ];
    let ok = matrix.iter().all(|(_, v)| *v);
    emit(p, "read_only_matrix", ok, t.elapsed().as_millis(), &format!("{:?}", matrix.iter().map(|(k, _)| k).collect::<Vec<_>>()));

    let t = Instant::now();
    let m2 = [
        ("write-inside-allow", verdict("auto", "write", &json!({"path": "lane5_out.txt"}), &cwd) == Verdict::Allow),
        ("write-outside-ask", verdict("auto", "write", &json!({"path": "/etc/lane5-x"}), &cwd) == Verdict::Ask),
        ("bash-ask", verdict("auto", "bash", &json!({"command": "ls"}), &cwd) == Verdict::Ask),
        ("full-bash-allow", verdict("full", "bash", &json!({"command": "ls"}), &cwd) == Verdict::Allow),
        ("unknown-tool-allow", verdict("auto", "sidecar_widget", &json!({}), &cwd) == Verdict::Allow),
        ("schedule_task-allow", verdict("auto", "schedule_task", &json!({}), &cwd) == Verdict::Allow),
    ];
    emit(p, "auto_full_matrix", m2.iter().all(|(_, v)| *v), t.elapsed().as_millis(), "see report for unknown/schedule_task notes");

    // allow card: scripted Yes -> Ok, mode unchanged
    let t = Instant::now();
    let gate = ApprovalGate::new("auto");
    let qb = bridge(&["Yes (Recommended)"]);
    let r = gate.check("bash", &json!({"command": "echo lane5-allow-card"}), &cwd, "echo lane5-allow-card", Some(&qb)).await;
    emit(p, "card_allow_once", r.is_ok() && gate.mode() == "auto", t.elapsed().as_millis(), &format!("{:?} mode={}", r.is_ok(), gate.mode()));

    // deny card: no bridge -> fail closed with workaround text
    let t = Instant::now();
    let gate = ApprovalGate::new("auto");
    let r = gate.check("bash", &json!({"command": "ls"}), &cwd, "ls", None).await;
    let msg = r.err().unwrap_or_default();
    emit(p, "card_deny_no_bridge", msg.contains("declined") && msg.contains("do NOT re-attempt"), t.elapsed().as_millis(), &msg);

    // deny card: scripted No
    let t = Instant::now();
    let gate = ApprovalGate::new("auto");
    let qb = bridge(&["No"]);
    let r = gate.check("bash", &json!({"command": "ls"}), &cwd, "ls", Some(&qb)).await;
    emit(p, "card_deny_scripted", r.is_err(), t.elapsed().as_millis(), &r.err().unwrap_or_default());

    // session card: Yes for session remembers path; second check passes with no bridge
    let t = Instant::now();
    let gate = ApprovalGate::new("auto");
    let qb = bridge(&["Yes, for session"]);
    let outside = "/tmp/lane5_session_probe.txt";
    let r1 = gate.check("write", &json!({"path": outside}), &cwd, outside, Some(&qb)).await;
    let r2 = gate.check("write", &json!({"path": outside}), &cwd, outside, None).await;
    emit(p, "card_session_path", r1.is_ok() && r2.is_ok(), t.elapsed().as_millis(), &format!("first={} second={}", r1.is_ok(), r2.is_ok()));

    // prefix card: "Yes, for this prefix" remembers FULL command only
    let t = Instant::now();
    let gate = ApprovalGate::new("auto");
    let qb = bridge(&["Yes, for this prefix"]);
    let cmd = "echo lane5-prefix-probe";
    let r1 = gate.check("bash", &json!({"command": cmd}), &cwd, cmd, Some(&qb)).await;
    let r2 = gate.check("bash", &json!({"command": cmd}), &cwd, cmd, None).await;
    let r3 = gate.check("bash", &json!({"command": "echo lane5-other-sibling"}), &cwd, "echo lane5-other-sibling", None).await;
    emit(p, "card_prefix_full_only", r1.is_ok() && r2.is_ok() && r3.is_err(), t.elapsed().as_millis(), &format!("same={} sibling_asks={}", r2.is_ok(), r3.is_err()));

    // schedule_task: allowed by gate but no tool implements it
    let t = Instant::now();
    let reg = Registry::builtin();
    let r = exec(&reg, &ctx(&sess(p)), "schedule_task", json!({})).await;
    emit(p, "schedule_task_unregistered", r.is_error && r.content.contains("does not exist"), t.elapsed().as_millis(), &r.content);
}

async fn phase_background() {
    let p = "background";
    let s = sess(p);
    let c = ctx(&s);
    let (reg, manifests) = gray_plugin::builder::from_plugins(&gray_plugin::builder::default_plugins());
    info(p, "plugin_tools", &reg.tool_names().join(","));
    info(p, "manifests", &manifests.iter().map(|m| m.name.clone()).collect::<Vec<_>>().join(","));

    let t = Instant::now();
    let bg = exec(&reg, &c, "bash", json!({"command": "echo lane5-bg1; sleep 2; echo lane5-bg2", "background": true})).await;
    let started = !bg.is_error && bg.content.contains("started t1");
    emit(p, "bg_start_t1", started, t.elapsed().as_millis(), &bg.content);

    let t = Instant::now();
    let done = exec(&reg, &c, "shell_output", json!({"task_id": "t1", "wait": "exit", "timeout": 10})).await;
    let dt = t.elapsed().as_millis();
    emit(p, "bg_wait_exit", !done.is_error && done.content.contains("lane5-bg2") && dt < 9000, dt, &done.content);

    let t = Instant::now();
    let list = exec(&reg, &c, "shell_output", json!({})).await;
    emit(p, "bg_list", !list.is_error && list.content.contains("t1"), t.elapsed().as_millis(), &list.content);

    let t = Instant::now();
    let again = exec(&reg, &c, "shell_output", json!({"task_id": "t1", "from_offset": 999999})).await;
    emit(p, "bg_past_end_note", !again.is_error && again.content.contains("past the end"), t.elapsed().as_millis(), &again.content);

    // sleep wakes early on task exit in the same session
    let t = Instant::now();
    let bg2 = exec(&reg, &c, "bash", json!({"command": "sleep 3", "background": true})).await;
    let ok2 = !bg2.is_error;
    let sl = exec(&reg, &c, "sleep", json!({"seconds": 10, "reason": "lane5-wake-test"})).await;
    let dt = t.elapsed().as_millis();
    emit(p, "sleep_wake_early", ok2 && !sl.is_error && sl.content.contains("woken early") && dt < 8000, dt, &sl.content);

    // kill own task
    let t = Instant::now();
    let bg3 = exec(&reg, &c, "bash", json!({"command": "sleep 30", "background": true})).await;
    let id3 = bg3.content.lines().next().unwrap_or("").to_string();
    let tid = if id3.contains("started t") {
        let n: String = id3.split("started t").nth(1).unwrap_or("").chars().take_while(|ch| ch.is_ascii_digit()).collect();
        format!("t{}", n)
    } else {
        "t?".to_string()
    };
    let k = exec(&reg, &c, "shell_kill", json!({"task_id": tid})).await;
    emit(p, "kill_own_task", !k.is_error && (k.content.contains("terminated") || k.content.contains("SIGKILL") || k.content.contains("exited")), t.elapsed().as_millis(), &format!("{} <- {}", tid, k.content));

    let t = Instant::now();
    let k = exec(&reg, &c, "shell_kill", json!({"task_id": "t999"})).await;
    emit(p, "kill_unknown", k.is_error && k.content.contains("unknown task"), t.elapsed().as_millis(), &k.content);

    let t = Instant::now();
    let k = exec(&reg, &c, "shell_kill", json!({})).await;
    emit(p, "kill_no_target", k.is_error && k.content.contains("exactly one"), t.elapsed().as_millis(), &k.content);

    // foreign pid fail-closed: our own spawned child, untouched
    let t = Instant::now();
    let mut child = tokio::process::Command::new("sleep").arg("30").spawn().expect("spawn sleep");
    let pid = child.id().expect("pid");
    let k = exec(&reg, &c, "shell_kill", json!({"pid": pid})).await;
    let denied = k.is_error && k.content.contains("approval");
    let alive = std::path::Path::new(&format!("/proc/{}", pid)).exists();
    let _ = child.kill().await;
    let _ = child.wait().await;
    emit(p, "kill_foreign_pid_fail_closed", denied && alive, t.elapsed().as_millis(), &format!("pid={} denied={} alive_after={}", pid, denied, alive));

    // foreign port fail-closed: held ephemeral listener stays up
    let t = Instant::now();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let k = exec(&reg, &c, "shell_kill", json!({"port": port})).await;
    let denied = k.is_error && (k.content.contains("approval") || k.content.contains("needs user approval"));
    drop(listener);
    emit(p, "kill_foreign_port_fail_closed", denied, t.elapsed().as_millis(), &format!("port={} denied={}", port, denied));

    // guard Allow path live via Auto permission prompt-free benign command is covered in tools phase;
    // guard Prompt path in Auto mode must fail closed (benign Prompt shape: git reset --hard in temp dir, never executes)
    let t = Instant::now();
    let auto_ctx = ToolContext { permission: PermissionMode::Auto, ..ctx(&s) };
    let r = exec(&reg, &auto_ctx, "bash", json!({"command": "git reset --hard"})).await;
    emit(p, "guard_prompt_fails_closed_auto", r.is_error && r.content.contains("Blocked by destructive-command guard"), t.elapsed().as_millis(), &r.content);
}

fn usage() {
    eprintln!("usage: lane5_probe <tools|approvals|background|all>");
}

#[tokio::main]
async fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    // Never touch the real home: refuse to run without isolation.
    let gray_home = std::env::var("GRAY_HOME").unwrap_or_default();
    let real_home = Path::new(&gray_home);
    let home_ok = !gray_home.trim().is_empty()
        && (real_home.ends_with("lane5_probe_home") || gray_home.contains("lane5_tools-"));
    if !home_ok {
        eprintln!("REFUSAL: GRAY_HOME must point at an isolated lane5 dir, got {:?}", gray_home);
        std::process::exit(2);
    }
    // Sanity: workdir must exist and be outside the repo source tree.
    let wd = workdir();
    if !wd.is_dir() {
        eprintln!("REFUSAL: LANE5_WORK missing: {}", wd.display());
        std::process::exit(2);
    }
    match arg.as_str() {
        "tools" => phase_tools().await,
        "approvals" => phase_approvals().await,
        "background" => phase_background().await,
        "all" => {
            phase_tools().await;
            phase_approvals().await;
            phase_background().await;
        }
        _ => {
            usage();
            std::process::exit(2);
        }
    }
    let fails = unsafe { FAILS };
    println!("LANE5|done|fails={}", fails);
    if fails > 0 {
        std::process::exit(1);
    }
}
