#![cfg(windows)]

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const ROWS: u16 = 40;
const COLS: u16 = 120;
const MARKER: &str = "ZXLEAK";

struct Console {
    _pair: portable_pty::PtyPair,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    parser: Arc<Mutex<vt100::Parser>>,
    raw: Arc<Mutex<Vec<u8>>>,
}

impl Console {
    fn spawn_cmd(home: &std::path::Path, prog: &str, args: &[&str]) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(prog);
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(home);
        for (k, _) in std::env::vars() {
            if k.starts_with("GRAY_") || k == "WT_SESSION" {
                cmd.env_remove(&k);
            }
        }
        cmd.env("GRAY_HOME", home);
        cmd.env("GRAY_USER_HOME", home);
        let child = pair.slave.spawn_command(cmd).expect("spawn");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let writer = Arc::new(Mutex::new(
            pair.master.take_writer().expect("writer") as Box<dyn Write + Send>
        ));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
        let p = Arc::clone(&parser);
        let w = Arc::clone(&writer);
        let raw = Arc::new(Mutex::new(Vec::<u8>::new()));
        let r = Arc::clone(&raw);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        r.lock().unwrap().extend_from_slice(&buf[..n]);
                        p.lock().unwrap().process(&buf[..n]);
                        if buf[..n].windows(4).any(|c| c == b"\x1b[6n") {
                            let _ = w.lock().unwrap().write_all(b"\x1b[1;1R");
                            let _ = w.lock().unwrap().flush();
                        }
                    }
                }
            }
        });
        Self {
            _pair: pair,
            child,
            writer,
            parser,
            raw,
        }
    }

    fn spawn(home: &std::path::Path) -> Self {
        Self::spawn_cmd(home, env!("CARGO_BIN_EXE_gray"), &[])
    }

    fn raw(&self) -> Vec<u8> {
        self.raw.lock().unwrap().clone()
    }

    fn contents(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    fn send(&mut self, s: &str) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(s.as_bytes()).unwrap();
        w.flush().unwrap();
    }

    fn wait_for(&self, what: &str, pred: impl Fn(&str) -> bool, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let c = self.contents();
            if pred(&c) {
                return c;
            }
            if Instant::now() > deadline {
                panic!("timed out waiting for {what}\n--- screen ---\n{c}\n--- end ---");
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn wait_idle(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        let mut same = 0;
        loop {
            let c = self.contents();
            if c == last {
                same += 1;
                if same >= 4 {
                    return;
                }
            } else {
                same = 0;
            }
            last = c;
            if Instant::now() > deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

fn composer_up(contents: &str) -> bool {
    contents.contains("E2e Test Model")
}

fn in_box_row(line: &str) -> bool {
    line.starts_with(" >") || line.starts_with("   ")
}

fn scratch_home(tag: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("gray-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.json"),
        r#"{"model":"e2e-test-model","api_key":"e2e-key","base_url":"https://127.0.0.1:1/","context_window":128000,"intro":false,"auto_update":false,"quality_of_life":false}"#,
    )
    .unwrap();
    home
}

fn session_exists(home: &std::path::Path) -> bool {
    home.join("sessions")
        .read_dir()
        .map(|mut d| {
            d.any(|e| {
                e.ok()
                    .is_some_and(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
            })
        })
        .unwrap_or(false)
}

fn run_restart_scenario(home: &std::path::Path, con: &mut Console) {
    con.wait_for("initial composer", composer_up, Duration::from_secs(90));

    con.send("hi\r");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !session_exists(home) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        session_exists(home),
        "no session file created under {home:?}"
    );

    con.send("\x1b");
    con.wait_idle(Duration::from_secs(30));
    con.send("/restart\r");
    con.wait_for(
        "restart message",
        |c| c.contains("restarting"),
        Duration::from_secs(30),
    );

    let mut pre_resumed_marker = false;
    let flood_deadline = Instant::now() + Duration::from_secs(25);
    loop {
        con.send(MARKER);
        let raw = con.raw();
        let resumed_at = raw.windows(7).position(|w| w == b"Resumed");
        let marker_at = raw
            .windows(MARKER.len())
            .position(|w| w == MARKER.as_bytes());
        match (resumed_at, marker_at) {
            (None, Some(_)) => pre_resumed_marker = true,
            (Some(r), Some(m)) if m < r => pre_resumed_marker = true,
            _ => {}
        }
        if resumed_at.is_some() || Instant::now() > flood_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }

    assert!(
        !pre_resumed_marker,
        "console echoed input during restart handoff"
    );

    let _ = con.wait_for(
        "resumed session",
        |c| c.contains("Resumed"),
        Duration::from_secs(90),
    );
    con.wait_for("child composer", composer_up, Duration::from_secs(60));
    con.wait_idle(Duration::from_secs(15));
    let contents = con.contents();

    let stray: Vec<&str> = contents
        .lines()
        .filter(|l| l.contains(MARKER))
        .filter(|l| !in_box_row(l) && !l.chars().all(|c| c.is_ascii_uppercase()))
        .collect();
    assert!(
        stray.is_empty(),
        "input leaked outside the prompt box: {stray:?}\n--- screen ---\n{contents}\n--- end ---"
    );
    assert!(
        contents
            .lines()
            .any(|l| l.contains(MARKER) && in_box_row(l)),
        "marker never reached the prompt box\n--- screen ---\n{contents}\n--- end ---"
    );

    let _ = con.child.kill();
    let _ = con.child.wait();
}

#[test]
fn typed_input_stays_in_prompt_box_across_restart() {
    let home = scratch_home("plain");
    let mut con = Console::spawn(&home);
    run_restart_scenario(&home, &mut con);
}

#[test]
fn typed_input_stays_out_of_powershell_across_restart() {
    let home = scratch_home("ps");
    let mut con = Console::spawn_cmd(
        &home,
        "powershell.exe",
        &["-NoProfile", "-NoLogo", "-NoExit"],
    );
    con.wait_for(
        "powershell prompt",
        |c| {
            c.lines()
                .any(|l| l.starts_with("PS ") && l.trim_end().ends_with('>'))
        },
        Duration::from_secs(60),
    );
    con.send(&format!("& '{}'\r", env!("CARGO_BIN_EXE_gray")));
    run_restart_scenario(&home, &mut con);
}
