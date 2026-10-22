use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

static NEXT: AtomicU32 = AtomicU32::new(0);

fn have(bin: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn sh(dir: &Path, cmd: &str) {
    let ok = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "command failed: {cmd}");
}

struct Gray {
    name: String,
    root: PathBuf,
    project: PathBuf,
    home: PathBuf,
    state: PathBuf,
    display: String,
    xvfb: Child,
    app: Option<Child>,
    env: Vec<(String, String)>,
}

impl Gray {
    fn start(name: &str) -> Option<Self> {
        Self::start_with(name, "fake", Vec::new())
    }

    fn start_with(name: &str, provider: &str, env: Vec<(String, String)>) -> Option<Self> {
        if !have("Xvfb") || !have("xdotool") {
            eprintln!("skipping {name}: Xvfb and xdotool are required for e2e tests");
            return None;
        }
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("gray-e2e")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        let project = root.join("project");
        let home = root.join("home");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(
            project.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        fs::write(project.join("README.md"), "# demo\n").unwrap();
        sh(
            &project,
            "git init -q -b main && git add -A && git -c user.email=e2e@gray -c user.name=e2e commit -qm init",
        );
        fs::write(
            home.join("events.jsonl"),
            format!("{{\"type\":\"setting_changed\",\"key\":\"default_provider\",\"value\":\"{provider}\"}}\n"),
        )
        .unwrap();
        let mut n = 300 + (std::process::id() % 100) * 16 + NEXT.fetch_add(1, Ordering::SeqCst);
        while Path::new(&format!("/tmp/.X{n}-lock")).exists() {
            n += 1;
        }
        let xvfb = Command::new("Xvfb")
            .arg(format!(":{n}"))
            .args(["-screen", "0", "1600x1000x24", "-nolisten", "tcp"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start Xvfb");
        let socket = format!("/tmp/.X11-unix/X{n}");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !Path::new(&socket).exists() && Instant::now() < deadline {
            sleep(Duration::from_millis(50));
        }
        let state = root.join("state.json");
        let mut g = Self {
            name: name.to_string(),
            root,
            project,
            home,
            state,
            display: format!(":{n}"),
            xvfb,
            app: None,
            env,
        };
        g.launch();
        Some(g)
    }

    fn launch(&mut self) {
        let _ = fs::remove_file(&self.state);
        let xdg = self.root.join("xdg");
        fs::create_dir_all(&xdg).unwrap();
        let log = fs::File::create(self.root.join("app.log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_gray-ide"))
            .arg(&self.project)
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .env("DISPLAY", &self.display)
            .env("VK_ICD_FILENAMES", "/nonexistent.json")
            .env("XDG_RUNTIME_DIR", &xdg)
            .env("GRAY_IDE_HOME", &self.home)
            .env("GRAY_IDE_STATE", &self.state)
            .env("GRAY_IDE_CMDS", self.root.join("cmds.txt"))
            .env(
                "GRAY_IDE_FAKE_DELAY_MS",
                self.env
                    .iter()
                    .find(|(k, _)| k == "GRAY_IDE_FAKE_DELAY_MS")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| "5".into()),
            )
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start gray-ide");
        self.app = Some(child);
        self.wait_for("app ready", |s| s["ready"] == true);
        sleep(Duration::from_millis(600));
    }

    fn restart(&mut self) {
        if let Some(mut app) = self.app.take() {
            let _ = app.kill();
            let _ = app.wait();
        }
        self.launch();
    }

    fn state(&self) -> Value {
        fs::read_to_string(&self.state)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null)
    }

    fn wait_for(&self, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            let s = self.state();
            if !s.is_null() && f(&s) {
                return s;
            }
            if Instant::now() > deadline {
                let shot = self.screenshot("failure");
                panic!(
                    "timed out waiting for {what}\nscreenshot: {}\nstate: {s:#}",
                    shot.display()
                );
            }
            sleep(Duration::from_millis(100));
        }
    }

    fn xdo(&self, args: &[&str]) -> bool {
        for _ in 0..10 {
            let ok = Command::new("xdotool")
                .args(args)
                .env("DISPLAY", &self.display)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                return true;
            }
            sleep(Duration::from_millis(200));
        }
        false
    }

    fn key(&self, k: &str) {
        self.xdo(&["key", "--delay", "40", k]);
        sleep(Duration::from_millis(200));
    }

    fn drag(&self, x: i32, y: i32, dx: i32, dy: i32) {
        let steps = 8;
        self.xdo(&["mousemove", &x.to_string(), &y.to_string()]);
        sleep(Duration::from_millis(150));
        self.xdo(&["mousedown", "1"]);
        sleep(Duration::from_millis(120));
        for i in 1..=steps {
            let xi = x + dx * i / steps;
            let yi = y + dy * i / steps;
            self.xdo(&["mousemove", &xi.to_string(), &yi.to_string()]);
            sleep(Duration::from_millis(40));
        }
        self.xdo(&["mouseup", "1"]);
        sleep(Duration::from_millis(250));
    }

    fn type_text(&self, text: &str) {
        self.xdo(&["type", "--delay", "20", text]);
        sleep(Duration::from_millis(150));
    }

    fn focus_window(&self) {
        let pid = self.app.as_ref().map(|c| c.id()).unwrap_or(0);
        if pid == 0 {
            return;
        }
        let _ = Command::new("xdotool")
            .args(["search", "--onlyvisible", "--pid", &pid.to_string()])
            .env("DISPLAY", &self.display)
            .output()
            .map(|o| {
                for w in String::from_utf8_lossy(&o.stdout).lines() {
                    let _ = Command::new("xdotool")
                        .args(["windowactivate", w])
                        .env("DISPLAY", &self.display)
                        .status();
                    let _ = Command::new("xdotool")
                        .args(["windowfocus", w])
                        .env("DISPLAY", &self.display)
                        .status();
                }
            });
    }

    fn click_composer(&self) {
        let pid = self.app.as_ref().map(|c| c.id()).unwrap_or(0);
        if pid == 0 {
            return;
        }
        let wids = Command::new("xdotool")
            .args(["search", "--onlyvisible", "--pid", &pid.to_string()])
            .env("DISPLAY", &self.display)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        let Some(wid) = wids.lines().next() else {
            return;
        };
        let geo = Command::new("xdotool")
            .args(["getwindowgeometry", "--shell", wid])
            .env("DISPLAY", &self.display)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        let (mut w, mut h) = (0i64, 0i64);
        for l in geo.lines() {
            if let Some(v) = l.strip_prefix("WIDTH=") {
                w = v.parse().unwrap_or(0);
            }
            if let Some(v) = l.strip_prefix("HEIGHT=") {
                h = v.parse().unwrap_or(0);
            }
        }
        if w == 0 || h == 0 {
            return;
        }
        let x = (w / 2).to_string();
        let y = ((h as f64) * 0.82) as i64;
        self.xdo(&[
            "mousemove",
            "--window",
            wid,
            &x,
            &y.to_string(),
            "click",
            "1",
        ]);
    }

    fn send(&self, text: &str) {
        for _ in 0..5 {
            if self.state()["palette_open"] == false {
                break;
            }
            self.key("Escape");
            sleep(Duration::from_millis(200));
        }
        for i in 0..8 {
            self.focus_window();
            if i == 4 {
                self.click_composer();
            }
            self.type_text(text);
            if self.state()["composer"] == text {
                break;
            }
            self.key("ctrl+a");
            self.key("BackSpace");
            sleep(Duration::from_millis(120));
        }
        if self.state()["composer"] == text {
            for _ in 0..12 {
                self.key("Return");
                for _ in 0..6 {
                    if self.state()["composer"] == "" {
                        return;
                    }
                    sleep(Duration::from_millis(200));
                }
            }
        }
        self.key("ctrl+a");
        self.key("BackSpace");
        let cmds = self.root.join("cmds.txt");
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&cmds)
            .unwrap();
        writeln!(f, "{text}").unwrap();
        drop(f);
        self.wait_for("command drained", |_| {
            fs::read_to_string(&cmds)
                .map(|c| c.trim().is_empty())
                .unwrap_or(false)
        });
        sleep(Duration::from_millis(400));
    }

    fn palette(&self, query: &str) {
        for _ in 0..10 {
            if self.state()["palette_open"] == true {
                break;
            }
            self.key("ctrl+k");
            sleep(Duration::from_millis(300));
        }
        self.wait_for("palette open", |s| s["palette_open"] == true);
        for _ in 0..12 {
            if self.state()["palette_query"] == query {
                break;
            }
            self.key("ctrl+a");
            self.key("BackSpace");
            self.type_text(query);
            sleep(Duration::from_millis(400));
        }
        assert_eq!(self.state()["palette_query"], query);
        for _ in 0..12 {
            self.key("Return");
            for _ in 0..6 {
                if self.state()["palette_open"] == false {
                    return;
                }
                sleep(Duration::from_millis(200));
            }
        }
        self.wait_for("palette closed", |s| s["palette_open"] == false);
    }

    fn screenshot(&self, label: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("gray-e2e-screenshots");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(format!("{}-{label}.png", self.name));
        let _ = Command::new("import")
            .args(["-window", "root"])
            .arg(&path)
            .env("DISPLAY", &self.display)
            .status();
        path
    }
}

impl Drop for Gray {
    fn drop(&mut self) {
        if let Some(mut app) = self.app.take() {
            let _ = app.kill();
            let _ = app.wait();
        }
        let _ = self.xvfb.kill();
        let _ = self.xvfb.wait();
    }
}

fn messages(s: &Value) -> Vec<(String, String)> {
    s["messages"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|m| {
                    (
                        m["role"].as_str().unwrap_or("").to_string(),
                        m["text"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn finished(s: &Value) -> bool {
    s["running"] == false
        && messages(s)
            .last()
            .map(|m| m.0 == "assistant")
            .unwrap_or(false)
}

#[test]
fn chat_turn_streams_reply_and_persists_across_restart() {
    let Some(mut g) = Gray::start("chat_turn") else {
        return;
    };
    g.send("hello gray");
    let s = g.wait_for("assistant reply", finished);
    let msgs = messages(&s);
    assert_eq!(msgs[0], ("user".into(), "hello gray".into()));
    assert!(
        msgs[1]
            .1
            .contains("Gray fake agent received: **hello gray**")
    );
    assert_eq!(s["thread"]["title"], "hello gray");
    assert_eq!(s["thread"]["provider"], "fake");
    assert!(
        s["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "checkpoint")
    );
    assert_eq!(s["threads"][0]["status"], "completed");
    assert_eq!(s["composer"], "");
    g.screenshot("reply");
    g.restart();
    let s = g.wait_for("restored thread", |s| messages(s).len() == 2);
    assert_eq!(s["thread"]["title"], "hello gray");
    assert_eq!(s["threads"].as_array().unwrap().len(), 1);
}

#[test]
fn keyboard_shortcuts_toggle_every_surface() {
    let Some(g) = Gray::start("shortcuts") else {
        return;
    };
    assert_eq!(g.state()["sidebar_open"], true);
    g.key("ctrl+b");
    g.wait_for("sidebar hidden", |s| s["sidebar_open"] == false);
    g.key("ctrl+b");
    g.wait_for("sidebar shown", |s| s["sidebar_open"] == true);
    g.key("ctrl+j");
    g.wait_for("terminal open", |s| s["terminal_open"] == true);
    g.key("ctrl+j");
    g.wait_for("terminal closed", |s| s["terminal_open"] == false);
    g.key("ctrl+d");
    g.wait_for("diff panel", |s| s["panel"] == "Diff");
    g.key("ctrl+e");
    g.wait_for("files panel", |s| s["panel"] == "Files");
    g.key("ctrl+e");
    g.wait_for("panel closed", |s| s["panel"].is_null());
    g.key("ctrl+comma");
    g.wait_for("settings open", |s| s["settings_open"] == true);
    g.screenshot("settings");
    g.key("Escape");
    g.wait_for("settings closed", |s| s["settings_open"] == false);
    g.key("ctrl+k");
    g.wait_for("palette open", |s| s["palette_open"] == true);
    g.key("Escape");
    g.wait_for("palette closed", |s| s["palette_open"] == false);
    g.key("ctrl+n");
    g.wait_for("new thread", |s| s["thread"]["title"] == "New thread");
}

#[test]
fn palette_switches_provider_model_and_mode() {
    let Some(g) = Gray::start("palette_model") else {
        return;
    };
    g.send("first");
    g.wait_for("reply", finished);
    g.key("ctrl+shift+m");
    let s = g.wait_for("model picker", |s| s["model_open"] == true);
    assert!(
        s["model_items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i.as_str().unwrap().contains(':'))
    );
    g.key("Escape");
    g.wait_for("picker closed", |s| s["model_open"] == false);
    g.palette("Model: Claude opus");
    g.wait_for("claude opus", |s| {
        s["thread"]["provider"] == "claude" && s["thread"]["model"] == "opus"
    });
    g.palette("Mode: Read only");
    g.wait_for("read only", |s| s["thread"]["mode"] == "readonly");
    g.palette("Provider: Fake");
    let s = g.wait_for("fake provider", |s| s["thread"]["provider"] == "fake");
    assert_eq!(s["thread"]["model"], "fake-1");
    assert_eq!(s["thread"]["mode"], "readonly");
    g.send("second");
    let s = g.wait_for("second reply", |s| finished(s) && messages(s).len() == 4);
    assert!(messages(&s)[3].1.contains("mode `Read only`"));
    g.screenshot("readonly");
}

#[test]
fn agent_edits_show_in_diff_and_checkpoint_restores() {
    let Some(g) = Gray::start("diff_checkpoint") else {
        return;
    };
    g.send("write notes.txt hello from the agent");
    let s = g.wait_for("edit reply", finished);
    assert!(
        s["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["label"] == "Edited notes.txt")
    );
    assert_eq!(
        fs::read_to_string(g.project.join("notes.txt")).unwrap(),
        "hello from the agent\n"
    );
    g.key("ctrl+d");
    let s = g.wait_for("diff lists file", |s| {
        s["panel"] == "Diff"
            && s["diff_files"]
                .as_array()
                .map(|a| a.iter().any(|f| f == "notes.txt"))
                .unwrap_or(false)
    });
    assert!(
        s["tree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "notes.txt")
    );
    g.screenshot("diff");
    g.palette("Restore last checkpoint");
    g.wait_for("diff empty", |s| {
        s["diff_files"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false)
    });
    assert!(!g.project.join("notes.txt").exists());
}

#[test]
fn file_picker_opens_edits_and_saves_files() {
    let Some(g) = Gray::start("editor") else {
        return;
    };
    g.key("ctrl+p");
    g.wait_for("file picker", |s| {
        s["palette_open"] == true && s["palette_mode"] == "Files"
    });
    g.type_text("main");
    let s = g.wait_for("filtered", |s| s["palette_items"][0] == "src/main.rs");
    assert_eq!(s["palette_items"].as_array().unwrap().len(), 1);
    g.key("Return");
    g.wait_for("tab open", |s| {
        s["active_tab"] == "src/main.rs" && s["panel"] == "Files"
    });
    g.key("ctrl+Home");
    g.type_text("// gray e2e");
    g.key("Return");
    g.wait_for("editor changed", |s| {
        s["editor_text"]
            .as_str()
            .unwrap_or("")
            .starts_with("// gray e2e\n")
    });
    g.screenshot("dirty");
    g.key("ctrl+s");
    g.wait_for("saved toast", |s| s["toast"] == "Saved src/main.rs");
    let content = fs::read_to_string(g.project.join("src/main.rs")).unwrap();
    assert!(content.starts_with("// gray e2e\nfn main() {"));
    g.wait_for("diff tracks save", |s| {
        s["diff_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "src/main.rs")
    });
}

#[test]
fn terminal_runs_shell_commands_in_project() {
    let Some(g) = Gray::start("terminal") else {
        return;
    };
    g.key("ctrl+j");
    g.wait_for("terminal", |s| s["terminal_open"] == true);
    g.type_text("echo gray-e2e && pwd");
    g.key("Return");
    let s = g.wait_for("output", |s| {
        s["terminal"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "[exit 0]")
    });
    let lines: Vec<&str> = s["terminal"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.as_str())
        .collect();
    assert!(lines.contains(&"$ echo gray-e2e && pwd"));
    assert!(lines.contains(&"gray-e2e"));
    assert!(lines.iter().any(|l| l.ends_with("/project")));
    g.screenshot("terminal");
    g.type_text("touch made-by-terminal.txt");
    g.key("Return");
    g.wait_for("tree refresh", |s| {
        s["tree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "made-by-terminal.txt")
    });
}

#[test]
fn slash_commands_manage_thread_lifecycle_and_branches() {
    let Some(g) = Gray::start("lifecycle") else {
        return;
    };
    g.send("alpha task");
    g.wait_for("alpha", finished);
    g.send("/rename Alpha renamed");
    g.wait_for("renamed", |s| s["thread"]["title"] == "Alpha renamed");
    g.send("/pin");
    g.wait_for("pinned", |s| s["thread"]["pinned"] == true);
    g.key("ctrl+n");
    g.wait_for("new thread", |s| s["thread"]["title"] == "New thread");
    g.send("beta task");
    let s = g.wait_for("beta", |s| {
        finished(s) && s["thread"]["title"] == "beta task"
    });
    let titles: Vec<&str> = s["threads"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["title"].as_str())
        .collect();
    assert!(titles.contains(&"Alpha renamed") && titles.contains(&"beta task"));
    g.send("/archive");
    let s = g.wait_for("archived", |s| {
        s["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["title"] == "beta task" && t["archived"] == true)
    });
    assert!(s["thread"].is_null());
    g.send("/branch feature-x");
    g.wait_for("branch created", |s| s["branch"] == "feature-x");
    g.send("/checkout main");
    let s = g.wait_for("back on main", |s| s["branch"] == "main");
    assert!(
        s["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "feature-x")
    );
    g.screenshot("lifecycle");
}

#[test]
fn failed_turn_surfaces_error_and_theme_persists() {
    let Some(mut g) = Gray::start("errors_theme") else {
        return;
    };
    g.send("please fail");
    let s = g.wait_for("error", |s| {
        s["running"] == false && s["threads"][0]["status"] == "error"
    });
    assert!(
        s["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "error" && a["detail"] == "fake provider failure requested")
    );
    assert!(s["toast"].as_str().unwrap().starts_with("Turn failed"));
    g.screenshot("error");
    assert_eq!(s["theme"], "dark");
    g.key("ctrl+shift+l");
    g.wait_for("light", |s| s["theme"] == "light");
    g.screenshot("light");
    g.restart();
    g.wait_for("light after restart", |s| s["theme"] == "light");
}

struct MockLlm {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

fn mock_llm() -> MockLlm {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            let mut first = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if first.is_empty() {
                    first = line.clone();
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let mut stream = stream;
            if !first.starts_with("POST") || !first.contains("/chat/completions") {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
                continue;
            }
            let n = {
                let mut l = log.lock().unwrap();
                l.push(String::from_utf8_lossy(&body).to_string());
                l.len()
            };
            let mut resp = String::from(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
            );
            for w in ["Hello ", "from ", "mock ", &format!("turn {n}")] {
                let chunk = serde_json::json!({"id": "c1", "object": "chat.completion.chunk", "model": "mock",
                    "choices": [{"index": 0, "delta": {"content": w}, "finish_reason": null}]});
                resp.push_str(&format!("data: {chunk}\n\n"));
            }
            let done = serde_json::json!({"id": "c1", "object": "chat.completion.chunk", "model": "mock",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 4, "total_tokens": 9}});
            resp.push_str(&format!("data: {done}\n\ndata: [DONE]\n\n"));
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    MockLlm { url, requests }
}

fn gray_bin() -> Option<PathBuf> {
    let path = std::env::var_os("GRAY_IDE_E2E_GRAY_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/gray"));
    path.exists().then_some(path)
}

#[test]
fn fake_provider_session_carries_across_turns() {
    let Some(g) = Gray::start("fake_session") else {
        return;
    };
    g.send("one");
    let s = g.wait_for("first", finished);
    let sid = s["thread"]["session"]
        .as_str()
        .expect("session recorded")
        .to_string();
    assert!(sid.starts_with("fake-"));
    g.send("two");
    let s = g.wait_for("second", |s| finished(s) && messages(s).len() == 4);
    assert!(messages(&s)[3].1.contains(&format!("session `{sid}`")));
    assert_eq!(s["thread"]["session"], sid.as_str());
}

#[test]
fn real_gray_cli_provider_streams_and_resumes_session() {
    let Some(bin) = gray_bin() else {
        eprintln!("skipping: build the gray CLI first (cargo build -p gray at the repo root)");
        return;
    };
    let llm = mock_llm();
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("gray-e2e")
        .join("gray-home");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let env = vec![
        ("GRAY_IDE_GRAY_BIN".to_string(), bin.display().to_string()),
        ("GRAY_HOME".to_string(), root.display().to_string()),
        ("GRAY_BASE_URL".to_string(), llm.url.clone()),
        ("GRAY_API_KEY".to_string(), "e2e-key".to_string()),
        ("GRAY_MODEL".to_string(), "mock".to_string()),
    ];
    let Some(g) = Gray::start_with("gray_cli", "gray", env) else {
        return;
    };
    g.send("first question for gray");
    let s = g.wait_for("gray reply", finished);
    assert_eq!(s["thread"]["provider"], "gray");
    assert_eq!(messages(&s)[1].1, "Hello from mock turn 1");
    let sid = s["thread"]["session"]
        .as_str()
        .expect("gray session id")
        .to_string();
    assert!(!sid.is_empty());
    g.screenshot("gray-reply");
    g.send("second question");
    let s = g.wait_for("second gray reply", |s| {
        finished(s) && messages(s).len() == 4
    });
    assert_eq!(messages(&s)[3].1, "Hello from mock turn 2");
    assert_eq!(s["thread"]["session"], sid.as_str());
    let reqs = llm.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[0].contains("first question for gray"));
    assert!(
        reqs[1].contains("first question for gray")
            && reqs[1].contains("Hello from mock turn 1")
            && reqs[1].contains("second question")
    );
}

#[test]
fn worktree_threads_run_in_isolated_checkout() {
    let Some(g) = Gray::start("worktree") else {
        return;
    };
    g.send("/worktree feat-a");
    let s = g.wait_for("worktree", |s| {
        s["thread"]["worktree"].is_string() && s["branch"] == "feat-a"
    });
    let wt = PathBuf::from(s["thread"]["worktree"].as_str().unwrap());
    assert!(wt.join("README.md").exists());
    g.send("write wt.txt from worktree");
    g.wait_for("edit in worktree", |s| {
        finished(s)
            && s["diff_files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "wt.txt")
    });
    assert!(wt.join("wt.txt").exists());
    assert!(!g.project.join("wt.txt").exists());
    g.screenshot("worktree");
}

#[test]
fn settle_snooze_effort_and_file_mentions() {
    let Some(g) = Gray::start("triage") else {
        return;
    };
    g.send("hello");
    g.wait_for("first", finished);
    g.send("/settle");
    g.wait_for("settled", |s| s["thread"]["settled"] == true);
    g.send("/snooze 30");
    let s = g.wait_for("snoozed", |s| s["thread"]["snoozed"] == true);
    assert_eq!(s["threads"][0]["snoozed"], true);
    g.send("/effort high");
    g.wait_for("effort", |s| s["thread"]["effort"] == "high");
    g.send("summarize @README.md");
    let s = g.wait_for("mention reply", |s| finished(s) && messages(s).len() == 4);
    let msgs = messages(&s);
    assert_eq!(msgs[2].1, "summarize @README.md");
    assert!(msgs[3].1.contains("effort `high`"));
    assert!(msgs[3].1.contains("# demo"));
}

#[test]
fn plugins_panel_browses_hub_and_manages_real_gray_plugins() {
    let Some(bin) = gray_bin() else {
        eprintln!("skipping: build the gray CLI first (cargo build -p gray at the repo root)");
        return;
    };
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("gray-e2e")
        .join("plugin-fixture");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("gray-home")).unwrap();
    let plugin = root.join("gray-hello");
    fs::write(
        &plugin,
        "#!/bin/sh\nif [ \"$1\" = \"manifest\" ]; then\n  echo '{\"name\":\"hello\",\"version\":\"0.1.0\",\"tools\":[],\"commands\":[]}'\n  exit 0\nfi\ncat >/dev/null\n",
    )
    .unwrap();
    sh(&root, "chmod +x gray-hello");
    let hub = root.join("hub.json");
    fs::write(
        &hub,
        r#"{"plugins":{"hello":{"kind":"plugin","version":"0.1.0","description":"Says hello from the hub","author":"official","official":true,"verified":true},"memory":{"kind":"plugin","version":"0.2.0","description":"Curated cross-session memory","author":"official","official":true}}}"#,
    )
    .unwrap();
    let env = vec![
        ("GRAY_IDE_GRAY_BIN".to_string(), bin.display().to_string()),
        (
            "GRAY_HOME".to_string(),
            root.join("gray-home").display().to_string(),
        ),
        ("GRAY_HUB_INDEX".to_string(), hub.display().to_string()),
    ];
    let Some(g) = Gray::start_with("plugins", "fake", env) else {
        return;
    };
    g.key("ctrl+shift+p");
    let s = g.wait_for("hub loaded", |s| {
        s["panel"] == "Plugins" && s["hub"].as_array().map(|h| h.len() == 2).unwrap_or(false)
    });
    assert_eq!(s["hub"][0], "hello");
    assert!(s["plugins"].as_array().unwrap().is_empty());
    g.send(&format!("/install {}", plugin.display()));
    let s = g.wait_for("installed", |s| {
        s["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "hello" && p["enabled"] == true)
    });
    assert!(s["toast"].as_str().unwrap().contains("Registered 'hello'"));
    g.screenshot("installed");
    g.send("/disable hello");
    g.wait_for("disabled", |s| {
        s["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "hello" && p["enabled"] == false)
    });
    g.send("/uninstall hello");
    g.wait_for("removed", |s| {
        s["plugins"].as_array().unwrap().is_empty() && s["plugin_busy"].is_null()
    });
}

fn max_pixel(path: &Path, x: u32, y: u32, w: u32, h: u32) -> u32 {
    let out = Command::new("convert")
        .arg(path)
        .args(["-crop", &format!("{w}x{h}+{x}+{y}")])
        .args(["-format", "%[max]", "info:"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    out.parse().unwrap_or(0)
}

#[test]
fn brand_logo_and_churning_loader_render() {
    let Some(g) = Gray::start_with(
        "brand",
        "fake",
        vec![("GRAY_IDE_FAKE_DELAY_MS".into(), "40".into())],
    ) else {
        return;
    };
    let shot = g.screenshot("brand-hero");
    assert!(
        max_pixel(&shot, 772, 268, 60, 60) > 150,
        "hero logo invisible"
    );
    assert!(
        max_pixel(&shot, 14, 14, 28, 28) > 100,
        "sidebar logo invisible"
    );
    g.send("take your time");
    let s = g.wait_for("turn started", |s| {
        s["running"] == true && s["turn_seconds"].is_number()
    });
    g.screenshot("churning");
    assert!(s["turn_seconds"].as_f64().unwrap() > 0.0);
    g.wait_for("turn done", |s| s["running"] == false);
}

#[test]
fn background_wallpaper_renders_and_persists() {
    let Some(mut g) = Gray::start("bg") else {
        return;
    };
    g.send("/background andromeda");
    g.wait_for("background set", |s| s["background"] == "andromeda");
    let shot = g.screenshot("bg-andromeda");
    assert!(
        max_pixel(&shot, 700, 300, 200, 150) > 45,
        "dithered wallpaper not visible"
    );
    g.restart();
    g.wait_for("background persisted", |s| s["background"] == "andromeda");
    g.send("/bg none");
    g.wait_for("background cleared", |s| s["background"] == "none");
}

#[test]
fn code_viewer_previews_file_and_background_in_palette() {
    let Some(g) = Gray::start("viewer") else {
        return;
    };
    std::fs::write(
        g.root.join("project/hello.rs"),
        "fn main() { println!(\"hi\"); }\n",
    )
    .unwrap();
    g.send("/view hello.rs");
    g.wait_for("viewer open", |s| s["viewer"] == "hello.rs");
    g.screenshot("viewer");
    g.key("ctrl+k");
    g.type_text("Background");
    let s = g.wait_for("background items", |s| {
        s["palette_items"]
            .to_string()
            .contains("Background: Andromeda")
    });
    assert!(s["palette_items"].to_string().contains("Background: None"));
    g.key("Escape");
}

#[test]
fn gray_sessions_panel_lists_and_resumes() {
    let gh = std::env::temp_dir().join("gray-e2e-gh");
    let sd = gh.join("sessions");
    fs::create_dir_all(&sd).unwrap();
    fs::write(
        sd.join("test-session-01.jsonl"),
        "{\"version\":1,\"id\":\"test-session-01\",\"timestamp\":1792331615438,\"cwd\":\"/tmp/proj\",\"model\":\"mock\",\"origin\":\"print\"}\n{\"entry_id\":0,\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"earlier conversation\"}]}}\n",
    )
    .unwrap();
    fs::write(sd.join("test-session-01.open"), "").unwrap();
    let Some(g) = Gray::start_with(
        "gsessions",
        "fake",
        vec![("GRAY_HOME".into(), gh.display().to_string())],
    ) else {
        return;
    };
    g.send("/sessions");
    let _s = g.wait_for("sessions listed", |s| {
        s["panel"] == "Sessions" && s["gray_sessions"].to_string().contains("test-session-01")
    });
    g.screenshot("sessions-panel");
    g.send("/resume test-session-01");
    g.wait_for("resumed", |s| {
        s["thread"]["session"] == "test-session-01" && s["thread"]["provider"] == "gray"
    });
}

#[test]
fn gray_command_registry_lists_and_runs_via_palette() {
    let Some(bin) = gray_bin() else {
        return;
    };
    let Some(g) = Gray::start_with(
        "gray_cmds",
        "fake",
        vec![
            ("GRAY_IDE_GRAY_BIN".into(), bin.display().to_string()),
            ("GRAY_HOME".into(), "/tmp/gray-e2e-gc-home".into()),
        ],
    ) else {
        return;
    };
    g.wait_for("gray commands loaded", |s| {
        s["gray_cmds"]
            .as_array()
            .map(|a| a.iter().any(|i| i == "help"))
            .unwrap_or(false)
    });
    g.palette("gray: help");
    let s = g.wait_for("gray cmd output", |s| {
        s["terminal"]
            .as_array()
            .map(|a| {
                a.iter()
                    .any(|l| l.as_str().unwrap_or("").contains("models"))
            })
            .unwrap_or(false)
    });
    assert_eq!(s["terminal_open"], true);
    g.screenshot("gray-cmd-help");
}

#[test]
fn turn_diff_shows_changes_since_checkpoint() {
    let Some(g) = Gray::start_with(
        "turn_diff",
        "fake",
        vec![("GRAY_IDE_FAKE_DELAY_MS".into(), "5".into())],
    ) else {
        return;
    };
    g.send("write turn.txt hello from the agent");
    g.wait_for("edit reply", |s| {
        finished(s) && s["messages"].to_string().contains("turn.txt")
    });
    g.palette("Diff since last turn");
    let _s = g.wait_for("turn diff open", |s| {
        s["panel"] == "Diff"
            && s["diff_base"].is_string()
            && s["diff_files"]
                .as_array()
                .map(|a| a.iter().any(|f| f == "turn.txt"))
                .unwrap_or(false)
    });
    g.screenshot("turn-diff");
    g.palette("Diff vs HEAD");
    g.wait_for("head diff", |s| {
        s["diff_base"].is_null() && s["panel"] == "Diff"
    });
}

#[test]
fn approval_gates_writes_in_readonly_mode() {
    let Some(g) = Gray::start_with(
        "approvals",
        "fake",
        vec![("GRAY_IDE_FAKE_DELAY_MS".into(), "5".into())],
    ) else {
        return;
    };
    g.send("first");
    g.wait_for("reply", finished);
    g.palette("Mode: Read only");
    g.wait_for("readonly", |s| s["thread"]["mode"] == "readonly");

    g.send("write guarded.txt secret");
    g.wait_for("approval shown", |s| {
        s["approval"]
            .as_str()
            .map(|a| a.contains("Write guarded.txt"))
            .unwrap_or(false)
    });
    g.screenshot("approval-banner");
    assert!(!g.project.join("guarded.txt").exists());
    g.send("/reject");
    g.wait_for("rejected", |s| {
        finished(s) && s["activities"].to_string().contains("Rejected")
    });
    assert!(!g.project.join("guarded.txt").exists());

    g.send("write allowed.txt ok");
    g.wait_for("approval shown again", |s| {
        s["approval"]
            .as_str()
            .map(|a| a.contains("Write allowed.txt"))
            .unwrap_or(false)
    });
    g.send("/approve");
    g.wait_for("approved", |s| {
        finished(s) && s["activities"].to_string().contains("Approved")
    });
    assert_eq!(
        fs::read_to_string(g.project.join("allowed.txt"))
            .unwrap()
            .trim(),
        "ok"
    );
}

#[test]
fn settings_page_layouts_toggles_and_terminal_side() {
    let Some(mut g) = Gray::start_with(
        "settings-page",
        "fake",
        vec![("GRAY_IDE_FAKE_DELAY_MS".into(), "5".into())],
    ) else {
        return;
    };
    g.palette("Open settings");
    g.wait_for("preferences page", |s| {
        s["settings_open"] == true && s["settings_section"] == "preferences"
    });
    g.screenshot("settings-preferences");
    g.palette("Settings: Shortcuts");
    g.wait_for("shortcuts section", |s| {
        s["settings_section"] == "shortcuts"
    });
    g.screenshot("settings-shortcuts");
    g.palette("Settings: Preferences");
    g.wait_for("back to preferences", |s| {
        s["settings_open"] == true && s["settings_section"] == "preferences"
    });

    g.palette("Layout: Pro");
    g.wait_for("pro layout", |s| s["layout"] == "pro");
    g.palette("Terminal: Right");
    g.wait_for("terminal right", |s| s["terminal_placement"] == "right");
    g.palette("Auto-accept permissions: On");
    g.wait_for("autoaccept on", |s| s["auto_accept"] == true);
    g.key("Escape");
    g.wait_for("settings closed", |s| s["settings_open"] == false);
    g.screenshot("pro-rail");
    g.key("ctrl+j");
    g.wait_for("terminal open", |s| s["terminal_open"] == true);
    g.screenshot("terminal-right");
    g.key("ctrl+j");

    g.send("first");
    g.wait_for("reply", finished);
    g.palette("Mode: Read only");
    g.wait_for("readonly", |s| s["thread"]["mode"] == "readonly");
    g.send("write auto.txt ok");
    g.wait_for("auto approved", |s| finished(s) && s["approval"].is_null());
    assert_eq!(
        fs::read_to_string(g.project.join("auto.txt"))
            .unwrap()
            .trim(),
        "ok"
    );

    g.restart();
    g.wait_for("settings persist", |s| {
        s["layout"] == "pro" && s["auto_accept"] == true && s["terminal_placement"] == "right"
    });
}

#[test]
fn resizable_sidebar_drag_widens_pane() {
    let Some(g) = Gray::start("resize") else {
        return;
    };
    g.wait_for("ready", |s| s["ready"] == true);
    for x in [230, 232, 234, 236, 228] {
        g.drag(x, 300, 90, 0);
        if g.state()["pane_sizes"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_f64())
            .map(|w| w > 240.)
            .unwrap_or(false)
        {
            break;
        }
    }
    g.wait_for("sidebar widened", |s| {
        s["pane_sizes"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_f64())
            .map(|w| w > 260.)
            .unwrap_or(false)
    });
    g.screenshot("wide-sidebar");
    let w0 = g.state()["pane_sizes"][0].as_f64().unwrap_or(320.) as i32;
    for x in [w0 - 4, w0, w0 + 4, w0 - 8, w0 + 8] {
        g.drag(x, 300, -140, 0);
        if g.state()["pane_sizes"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_f64())
            .map(|w| w < 240.)
            .unwrap_or(false)
        {
            break;
        }
    }
    g.wait_for("sidebar narrowed", |s| {
        s["pane_sizes"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_f64())
            .map(|w| w < 240.)
            .unwrap_or(false)
    });
}

#[test]
fn image_attachments_chip_send_and_clear() {
    let Some(g) = Gray::start_with(
        "attach",
        "fake",
        vec![("GRAY_IDE_FAKE_DELAY_MS".into(), "5".into())],
    ) else {
        return;
    };
    let png: [u8; 70] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x01, 0x02, 0x0A, 0x54, 0x46, 0x18, 0x4E, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    fs::write(g.project.join("shot.png"), png).unwrap();
    g.send("/attach shot.png");
    g.wait_for("attached", |s| {
        s["attachments"]
            .as_array()
            .map(|a| {
                a.iter()
                    .any(|p| p.as_str().map(|p| p.ends_with("shot.png")).unwrap_or(false))
            })
            .unwrap_or(false)
    });
    g.screenshot("attachment-chip");
    g.send("look at this");
    g.wait_for("sent with image", |s| {
        finished(s)
            && s["attachments"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(false)
            && s["messages"].to_string().contains("Attached image")
    });
}

#[test]
fn skills_panel_lists_and_pastes_skill() {
    let Some(g) = Gray::start_with(
        "skills",
        "fake",
        vec![("GRAY_IDE_FAKE_DELAY_MS".into(), "5".into())],
    ) else {
        return;
    };
    let dir = g.project.join(".gray/skills/test-skill");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: does the test thing\n---\nwrite skill.txt ok\n",
    )
    .unwrap();
    g.send("/skills");
    g.wait_for("skill listed", |s| {
        s["skills"].to_string().contains("test-skill")
    });
    g.screenshot("skills-panel");
    g.palette("Use skill: test-skill");
    g.wait_for("composer has body", |s| {
        s["composer"].to_string().contains("skill.txt")
    });
    g.xdo(&["key", "Return"]);
    g.wait_for("skill run", |s| {
        finished(s) && s["messages"].to_string().contains("write skill.txt")
    });
    assert!(g.project.join("skill.txt").exists());
}
