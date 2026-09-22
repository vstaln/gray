use super::*;
use gray_core::agent::Tool;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

static SESS_N: AtomicU64 = AtomicU64::new(0);

fn sess(tag: &str) -> String {
    format!(
        "bash-1b-{tag}-{}-{}",
        std::process::id(),
        SESS_N.fetch_add(1, Ordering::Relaxed)
    )
}

fn ctx_for(session: &str) -> ToolContext {
    ToolContext {
        session_id: Some(session.to_string()),
        ..ToolContext::default()
    }
}

#[test]
fn shell_dir_respects_gray_home() {
    // Isolated GRAY_HOME must own shell logs, not real HOME.
    let dir = tempfile::tempdir().expect("tempdir");
    let gray = dir.path().to_string_lossy().into_owned();
    let prev = std::env::var("GRAY_HOME").ok();
    unsafe { std::env::set_var("GRAY_HOME", &gray) };
    let d = shell_dir();
    match prev {
        Some(v) => unsafe { std::env::set_var("GRAY_HOME", v) },
        None => unsafe { std::env::remove_var("GRAY_HOME") },
    }
    assert!(
        d.starts_with(dir.path()),
        "shell_dir must live under GRAY_HOME, got {}",
        d.display()
    );
    assert_eq!(d.file_name().and_then(|s| s.to_str()), Some("shell"));
}

/// Run `pwd` and return the raw output, for the cwd tests.
async fn tool_pwd(tool: &BashTool, ctx: &ToolContext) -> gray_core::agent::ToolOutput {
    tool.execute(ctx, json!({"command": "pwd"})).await
}

/// The command's own output, out of the fenced block the tool wraps it in.
fn body(content: &str) -> String {
    content
        .lines()
        .skip_while(|l| !l.contains("<untrusted-output>"))
        .skip(1)
        .take_while(|l| !l.contains("</untrusted-output>"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

#[tokio::test]
async fn echo_returns_exit_zero_with_output() {
    let session = sess("echo");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "echo hello"}))
        .await;
    assert!(!r.is_error, "{}", r.content);
    let head = r.content.lines().next().unwrap_or("");
    assert!(head.starts_with("exit 0"), "{head}");
    assert!(r.content.contains("hello"), "{}", r.content);
    assert!(r.content.contains("log "), "{}", r.content);
    assert!(!r.content.contains("Read more:"));
    assert!(
        !r.content.contains("started t"),
        "no task ids anymore: {}",
        r.content
    );
}

#[tokio::test]
async fn malformed_background_arg_fails_loud() {
    let session = sess("bgone");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "echo hi", "background": []}))
        .await;
    assert!(r.is_error, "{}", r.content);
    assert!(r.content.contains("background"), "{}", r.content);
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_and_returns_partial_output() {
    // `echo out; sleep 30` with timeout 1: SIGTERM lands, partial
    // output survives, no promotion text.
    let session = sess("timeout");
    let ctx = ctx_for(&session);
    let t0 = Instant::now();
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "echo out; sleep 30", "timeout": 1}))
        .await;
    let dt = t0.elapsed();
    assert!(!r.is_error, "{}", r.content);
    assert!(r.content.contains("timed out after 1s"), "{}", r.content);
    assert!(r.content.contains("out"), "{}", r.content);
    assert!(
        !r.content.contains("promoted"),
        "never promotes: {}",
        r.content
    );
    assert!(
        dt < Duration::from_secs(15),
        "timeout must kill, not wait: {dt:?}"
    );
}

#[tokio::test]
async fn empty_command_is_an_error() {
    let session = sess("empty");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "   "}))
        .await;
    assert!(r.is_error, "{}", r.content);
}

#[test]
fn truncated_log_has_executable_bounded_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log ' $(false) file");
    for raw in [
        b"a\r\n".repeat(20000),
        b"z".repeat(INLINE_BUDGET_BYTES + 4096),
        b"z".repeat(INLINE_BUDGET_BYTES + 1),
    ] {
        std::fs::write(&path, &raw).unwrap();
        let summary = PumpSummary {
            total_bytes: raw.len() as u64,
            total_lines: raw.iter().filter(|&&b| b == b'\n').count(),
            head: raw[..raw.len().min(MEM_HEAD_BYTES)].to_vec(),
            tail: raw[raw.len().saturating_sub(MEM_TAIL_BYTES)..].to_vec(),
            has_cr: raw.contains(&b'\r'),
            log_write_failed: false,
        };
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .status()
            .unwrap();
        let result = finish_inline("probe", &path, status, &summary, Instant::now(), None);
        let command = result
            .content
            .lines()
            .find_map(|l| l.strip_prefix("Read more: "))
            .expect("copyable command");
        let out = std::process::Command::new("sh")
            .args(["-c", command])
            .output()
            .unwrap();
        assert!(out.status.success(), "{:?}", out.stderr);
        assert!(!out.stdout.is_empty());
        assert!(out.stdout.len() <= READ_CHUNK as usize);
        // The marker in the body already names the byte window, so the extra
        // hint only has to say where the next page starts — and only when
        // there is more than one page left.
        let next = result
            .content
            .lines()
            .find_map(|l| l.strip_prefix("Then skip="));
        if raw.len() > INLINE_BUDGET_BYTES + READ_CHUNK as usize {
            assert!(
                next.is_some(),
                "multi-page window must name the next offset"
            );
        } else {
            assert!(next.is_none(), "single page needs no next offset");
        }
        if raw[0] == b'z' {
            assert_eq!(
                out.stdout,
                vec![b'z'; (raw.len() - INLINE_BUDGET_BYTES).min(READ_CHUNK as usize)]
            );
        } else {
            // Run 35229845737 captured the recovered bytes: Git Bash's sed
            // pipes CRLF text to native readers through a text-mode MSYS
            // pipe, folding CRLF to LF (`a\r\n` -> `a\n`). Whether that
            // folding happens is a property of the local MSYS mount, not of
            // gray: the same command returned raw CRLF on windows-runtime
            // (run 35517869669). The disk log stays byte-verbatim either way
            // (asserted in
            // progress_is_line_safe_but_log_retains_carriage_returns), so
            // the invariant that matters here is that recovery starts at the
            // truncated marker byte and stays bounded — not which EOL the
            // local pipe hands back. Emit the bytes in hex on failure.
            assert!(
                out.stdout.starts_with(b"a\r\n") || out.stdout.starts_with(b"a\n"),
                "expected 610d0a (or EOL-folded 610a) recovery, got {:02x?} (command: {command})",
                out.stdout
            );
        }
    }
}

#[tokio::test]
async fn explicit_timeout_says_how_to_rerun() {
    // A killed command must say how to let it finish: the benchmark retro
    // showed agents reading a timeout kill as "the command failed".
    let session = sess("timeout-msg");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "sleep 5", "timeout": 1}))
        .await;
    // A kill is data, not a tool error (non-zero exits are data by design),
    // but the note must be the first thing the model reads.
    assert!(!r.is_error, "killed command stays a result: {}", r.content);
    assert!(
        r.content.starts_with("timed out after 1s"),
        "the timeout note leads the output: {}",
        r.content
    );
    assert!(
        r.content.contains("rerun without `timeout`"),
        "the kill must say how to let it finish: {}",
        r.content
    );
}

#[test]
fn bash_schema_promises_no_default_timeout() {
    // Regression guard for a real mismatch: the tool text promised a 120s
    // default while the code killed at 30s. Agents lost long suites to it.
    assert!(
        DEFAULT_TIMEOUT_SECS.is_none(),
        "commands run until they exit unless a timeout is passed"
    );
    let def = BashTool::default().def();
    let desc = def.parameters["properties"]["timeout"]["description"]
        .as_str()
        .unwrap_or_default();
    assert!(desc.contains("omitted = no limit"), "{desc}");
    assert!(
        def.description.contains("no default"),
        "tool description must not promise a default: {}",
        def.description
    );
}

#[tokio::test]
async fn missing_command_gets_an_actionable_hint() {
    let session = sess("not-found");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(
            &ctx,
            json!({"command": "gray-definitely-not-a-tool --help"}),
        )
        .await;
    assert!(
        r.content
            .contains("`gray-definitely-not-a-tool` is not installed here"),
        "hint names the tool: {}",
        r.content
    );
    assert!(
        r.content.contains("command -v gray-definitely-not-a-tool"),
        "hint says how to confirm: {}",
        r.content
    );
}

#[tokio::test]
async fn ordinary_failures_get_no_not_found_hint() {
    let session = sess("no-hint");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "grep -q nomatch /etc/hostname"}))
        .await;
    assert!(
        !r.content.contains("is not installed here"),
        "ordinary non-zero exit stays clean: {}",
        r.content
    );
}

#[test]
fn not_found_subject_reads_every_shell_wording() {
    for (line, want) in [
        ("bash: line 1: rg: command not found", "rg"),
        ("sh: 1: xxd: not found", "xxd"),
        ("zsh: command not found: goyacc", "goyacc"),
        ("-bash: ruff: command not found", "ruff"),
    ] {
        assert_eq!(not_found_subject(line).as_deref(), Some(want), "{line}");
    }
    assert!(missing_command_hint("exit 0\nall good").is_none());
    assert!(missing_command_hint("curl: (22) 404 not found").is_none());
}

#[test]
fn known_missing_binaries_get_their_real_substitute() {
    // Telemetry-driven table: rg (84 misses) and xxd (30) dominated the
    // campaign; unknown binaries keep the generic list.
    let rg = missing_command_hint("bash: line 1: rg: command not found").unwrap();
    assert!(rg.contains("`grep -r`"), "{rg}");
    let xxd = missing_command_hint("sh: 1: xxd: not found").unwrap();
    assert!(xxd.contains("`od -c`"), "{xxd}");
    let unknown = missing_command_hint("bash: line 1: goyacc: command not found").unwrap();
    assert!(
        unknown.contains("`grep`, `sed`, `awk`, `python3`"),
        "{unknown}"
    );
    assert!(unknown.contains("command -v goyacc"), "{unknown}");
}

fn png_bytes() -> Vec<u8> {
    use image::ImageFormat;
    use std::io::Cursor;
    let img = image::RgbImage::from_pixel(2, 2, image::Rgb([9, 8, 7]));
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .unwrap();
    buf
}

#[tokio::test]
async fn cat_image_shows_png_as_vision_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("plot.png"), png_bytes()).unwrap();
    let out = cat_image("cat plot.png", dir.path()).expect("cat on a png must show the image");
    assert!(!out.is_error);
    assert!(out.content.contains("Image shown"), "{}", out.content);
    assert_eq!(out.images.len(), 1, "one vision block per call");
    assert_eq!(out.images[0].media_type, "image/png");
}

#[tokio::test]
async fn cat_image_only_claims_plain_single_file_cat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), png_bytes()).unwrap();
    std::fs::write(dir.path().join("b.png"), png_bytes()).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "text").unwrap();
    // The claimed shape: exactly cat + one bare path.
    assert!(cat_image("cat a.png", dir.path()).is_some());
    assert!(cat_image("  cat   a.png  ", dir.path()).is_some());
    // Everything else falls through to a normal shell run.
    for cmd in [
        "cat a.png b.png",
        "cat -A a.png",
        "cat a.png | wc -c",
        "cat a.png > copy.png",
        "head a.png",
        "cat notes.txt",
        "cat missing.png",
        "cat $HOME/a.png",
        "cat *.png",
    ] {
        assert!(
            cat_image(cmd, dir.path()).is_none(),
            "must not claim: {cmd}"
        );
    }
}

#[tokio::test]
async fn cat_image_through_execute_shows_vision() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("shot.png"), png_bytes()).unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        ..ToolContext::default()
    };
    let out = BashTool::default()
        .execute(&ctx, json!({"command": "cat shot.png"}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.images.len(), 1, "execute must surface the vision block");
}

#[tokio::test]
async fn cat_image_keeps_full_resolution() {
    use base64::Engine as _;
    use image::ImageDecoder;
    use std::io::Cursor;
    let dir = tempfile::tempdir().unwrap();
    // 2400px wide: past MAX_IMAGE_SIDE (2000), so the downscale path used by
    // the `read` tool would shrink this. `cat` must not.
    let img = image::RgbImage::from_pixel(2400, 100, image::Rgb([1, 2, 3]));
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(dir.path().join("wide.png"), &buf).unwrap();
    let out = cat_image("cat wide.png", dir.path()).unwrap();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&out.images[0].data)
        .unwrap();
    let decoded = image::ImageReader::with_format(Cursor::new(&raw), image::ImageFormat::Png)
        .into_decoder()
        .unwrap();
    assert_eq!(
        decoded.dimensions(),
        (2400, 100),
        "cat must send full resolution, not the 2000px downscale"
    );
}

#[tokio::test]
async fn a_cd_carries_into_the_next_command_in_the_same_session() {
    // A subdirectory of the session's own cwd, not an absolute path: Git Bash
    // speaks MSYS paths, so handing it a Windows `C:/...` path is exactly the
    // trap the windows-runtime job exists to catch. The behaviour under test
    // is the same either way.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ctx = ctx_for(&sess("cd"));
    ctx.cwd = dir.path().to_path_buf();

    let before = body(&tool_pwd(&BashTool::default(), &ctx).await.content);
    let tool = BashTool::default();
    let r = tool
        .execute(&ctx, json!({"command": "mkdir -p sub && cd sub"}))
        .await;
    assert!(!r.is_error, "{}", r.content);
    let after = body(&tool_pwd(&tool, &ctx).await.content);

    assert_ne!(
        before, after,
        "the cd did not carry into the next command: {before} -> {after}"
    );
    assert!(
        after.ends_with("/sub") || after.ends_with("\\sub"),
        "expected the subdirectory, got {after}"
    );
}

#[tokio::test]
async fn the_cwd_report_never_leaks_into_the_output() {
    // The report goes to a file, so stdout — and the durable log — are exactly
    // what the command produced.
    let tool = BashTool::default();
    let ctx = ctx_for(&sess("leak"));
    let r = tool.execute(&ctx, json!({"command": "echo hello"})).await;
    assert!(!r.is_error, "{}", r.content);
    // The only output is what the command produced: no sentinel, no path.
    assert!(!r.content.contains("GRAY_CWD_REPORT"), "{}", r.content);
    assert!(!r.content.contains("$PWD"), "{}", r.content);
    assert!(!r.content.contains("gray-cwd-"), "{}", r.content);
    let body = r
        .content
        .lines()
        .skip_while(|l| !l.contains("<untrusted-output>"))
        .skip(1)
        .take_while(|l| !l.contains("</untrusted-output>"))
        .collect::<Vec<_>>();
    assert_eq!(body, ["hello"], "{}", r.content);
}

#[tokio::test]
async fn a_deleted_directory_falls_back_to_the_context_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ctx = ctx_for(&sess("gone"));
    ctx.cwd = dir.path().to_path_buf();
    let tool = BashTool::default();
    let r = tool
        .execute(&ctx, json!({"command": "mkdir -p sub && cd sub"}))
        .await;
    assert!(!r.is_error, "{}", r.content);
    drop(dir); // the directory vanishes under the recorded cwd

    // The session must still run, from whatever the caller now hands over.
    let mut fresh = ctx.clone();
    fresh.cwd = std::env::temp_dir();
    let out = tool_pwd(&tool, &fresh).await;
    assert!(
        !out.is_error,
        "a vanished cwd must not wedge the session: {}",
        out.content
    );
}

#[tokio::test]
async fn two_sessions_do_not_share_a_working_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut a = ctx_for(&sess("iso-a"));
    a.cwd = dir.path().to_path_buf();
    let mut b = ctx_for(&sess("iso-b"));
    b.cwd = dir.path().to_path_buf();
    let tool = BashTool::default();

    let r = tool
        .execute(&a, json!({"command": "mkdir -p sub && cd sub"}))
        .await;
    assert!(!r.is_error, "{}", r.content);
    let ra = body(&tool_pwd(&tool, &a).await.content);
    let rb = body(&tool_pwd(&tool, &b).await.content);

    assert!(
        ra.ends_with("/sub") || ra.ends_with("\\sub"),
        "session a should have moved: {ra}"
    );
    assert_ne!(
        ra, rb,
        "session b must not inherit session a's directory: {rb}"
    );
}

#[test]
fn the_cwd_report_suffix_is_appended_not_substituted() {
    let wrapped = with_cwd_report("echo hi");
    assert!(wrapped.starts_with("echo hi"), "{wrapped}");
    assert!(wrapped.contains("$GRAY_CWD_REPORT"), "{wrapped}");
    // A command ending in a comment swallows the suffix, so nothing is
    // reported and the cwd simply stays put.
    let commented = with_cwd_report("echo hi # note");
    assert!(commented.starts_with("echo hi # note"), "{commented}");
}

#[test]
fn the_cwd_report_asks_for_a_path_rust_can_resolve() {
    // The report is read back with PathBuf::is_dir, so it must arrive in a form
    // Rust can resolve on the platform that produced it.
    let wrapped = with_cwd_report("echo hi");
    #[cfg(windows)]
    assert!(
        wrapped.contains("pwd -W"),
        "Git Bash's plain pwd is an MSYS path Rust rejects: {wrapped}"
    );
    #[cfg(not(windows))]
    assert!(wrapped.contains("$PWD"), "{wrapped}");
}

#[tokio::test]
async fn the_cwd_report_does_not_mask_the_commands_exit_code() {
    // The suffix must re-raise the command's own status: ending on the
    // report's `printf` would turn every failure into a success and cost the
    // benign-exit table its "no matches" note.
    let tool = BashTool::default();
    let ctx = ctx_for(&sess("exit"));
    let r = tool
        .execute(
            &ctx,
            json!({"command": "grep zzz_no_such_match_xyz /dev/null"}),
        )
        .await;
    assert!(!r.is_error, "{}", r.content);
    assert!(
        r.content.lines().next().unwrap_or("").contains("exit 1"),
        "{}",
        r.content
    );
    assert!(r.content.contains("no matches"), "{}", r.content);

    let r = tool.execute(&ctx, json!({"command": "exit 3"})).await;
    assert!(
        r.content.lines().next().unwrap_or("").contains("exit 3"),
        "{}",
        r.content
    );

    // And a success still reports success.
    let r = tool.execute(&ctx, json!({"command": "true"})).await;
    assert!(
        r.content.lines().next().unwrap_or("").contains("exit 0"),
        "{}",
        r.content
    );
}
