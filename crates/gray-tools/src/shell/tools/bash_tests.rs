use super::*;
use gray_core::agent::Tool;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

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
    use std::io::Cursor;
    let img = image::RgbImage::from_pixel(2, 2, image::Rgb([9, 8, 7]));
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    buf
}

#[tokio::test]
async fn cat_shows_media_as_images() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), png_bytes()).unwrap();
    std::fs::write(dir.path().join("b.png"), png_bytes()).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "text").unwrap();
    for (cmd, n) in [
        ("cat a.png", 1),
        ("  cat   a.png  ", 1),
        ("cat a.png b.png", 2),
    ] {
        let out = image_command(cmd, dir.path()).unwrap_or_else(|| panic!("must claim: {cmd}"));
        assert!(!out.is_error, "{cmd}: {}", out.content);
        assert_eq!(out.images.len(), n, "{cmd}");
    }
    // Anything that is not all bare media paths is `cat`'s own job.
    for cmd in [
        "cat -A a.png",
        "cat a.png notes.txt",
        "cat a.png | wc -c",
        "cat a.png > copy.png",
        "head a.png",
        "cat notes.txt",
        "cat missing.png",
        "cat $HOME/a.png",
        "cat *.png",
    ] {
        assert!(
            image_command(cmd, dir.path()).is_none(),
            "must not claim: {cmd}"
        );
    }
}

#[tokio::test]
async fn cat_text_still_runs_in_the_shell() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "plain text").unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        ..ToolContext::default()
    };
    let out = BashTool::default()
        .execute(&ctx, json!({"command": "cat notes.txt"}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("plain text"), "{}", out.content);
}

#[tokio::test]
async fn cat_shows_every_image_it_names() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), png_bytes()).unwrap();
    std::fs::write(dir.path().join("b.png"), png_bytes()).unwrap();
    let out =
        image_command("cat a.png b.png", dir.path()).expect("cat must show the images it names");
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.images.len(), 2, "one vision block per path");
    assert_eq!(out.images[0].media_type, "image/png");
    assert!(
        out.content.contains("a.png") && out.content.contains("b.png"),
        "{}",
        out.content
    );
    assert!(out.content.contains("Shown: "), "{}", out.content);
}

#[tokio::test]
async fn cat_caps_at_the_shared_2000px() {
    // One way to see a picture means one resolution rule: the 2000px cap the
    // `read` tool and pasted attachments already use.
    use base64::Engine as _;
    use image::ImageDecoder;
    use std::io::Cursor;
    let dir = tempfile::tempdir().unwrap();
    let img = image::RgbImage::from_pixel(2400, 100, image::Rgb([1, 2, 3]));
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(dir.path().join("wide.png"), &buf).unwrap();

    let out = image_command("cat wide.png", dir.path()).unwrap();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&out.images[0].data)
        .unwrap();
    let (w, h) = image::ImageReader::with_format(Cursor::new(&raw), image::ImageFormat::Png)
        .into_decoder()
        .unwrap()
        .dimensions();
    assert!(w <= 2000 && h < 100, "cat caps at 2000px, got {w}x{h}");
}

/// A real 2-frame clip, so the contact sheet has something to decode.
/// `None` when ffmpeg is unavailable.
fn tiny_mp4(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = dir.join("real.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=64x64:d=0.2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok.then_some(path)
}

#[tokio::test]
async fn cat_of_a_video_attaches_the_clip_with_its_sheet_as_fallback() {
    let dir = tempfile::tempdir().unwrap();
    if tiny_mp4(dir.path()).is_none() {
        eprintln!("skipping: ffmpeg could not build the fixture");
        return;
    }
    let out = image_command("cat real.mp4", dir.path()).expect("a clip must claim");
    assert!(out.images.is_empty(), "{}", out.content);
    assert_eq!(out.media.len(), 1, "{}", out.content);
    let vid = &out.media[0];
    assert_eq!(vid.media_type, "video/mp4");
    assert!(
        vid.fallback
            .iter()
            .any(|b| matches!(b, gray_core::message::ContentBlock::Image { .. })),
        "a non-video model needs the sheet"
    );
    assert!(out.content.contains("real.mp4 (video)"), "{}", out.content);
}

#[tokio::test]
async fn cat_of_a_pdf_and_audio_attaches_them_with_text_fallbacks() {
    // No decoder runs for audio, and a bogus PDF still claims: its fallback
    // is the pdftotext error note, so the model is told rather than handed
    // nothing.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("doc.pdf"), b"%PDF-1.4 not really").unwrap();
    std::fs::write(dir.path().join("talk.mp3"), b"ID3 fake").unwrap();
    let out = image_command("cat doc.pdf talk.mp3", dir.path()).expect("pdf+audio must claim");
    let types: Vec<&str> = out.media.iter().map(|m| m.media_type.as_str()).collect();
    assert_eq!(types, ["application/pdf", "audio/mpeg"], "{}", out.content);
    for m in &out.media {
        assert!(
            matches!(
                m.fallback.as_slice(),
                [gray_core::message::ContentBlock::Text { .. }]
            ),
            "{} needs a text fallback",
            m.media_type
        );
    }
    assert!(out.content.contains("doc.pdf (pdf)"), "{}", out.content);
    assert!(out.content.contains("talk.mp3 (audio)"), "{}", out.content);
}

#[tokio::test]
async fn cat_skips_a_missing_path_and_keeps_the_rest() {
    // One typo among several paths must not sink the good images: that is the
    // complaint the claim shape exists to avoid. A lone missing path still
    // falls through, so the shell gives the error.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("good.png"), png_bytes()).unwrap();
    let out = image_command("cat good.png typo.png", dir.path())
        .expect("a missing path must not drop the valid one");
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(
        out.images.len(),
        1,
        "only the path that exists: {}",
        out.content
    );
    assert!(out.content.contains("good.png"), "{}", out.content);
    assert!(
        out.content.contains("typo.png: no such file"),
        "the skipped path is named: {}",
        out.content
    );
    // Alone, a missing path is nothing usable, so the shell reports it.
    assert!(image_command("cat typo.png", dir.path()).is_none());
}

#[tokio::test]
async fn cat_caps_the_claim_at_eight_paths() {
    let dir = tempfile::tempdir().unwrap();
    for n in 0..10 {
        std::fs::write(dir.path().join(format!("p{n}.png")), png_bytes()).unwrap();
    }
    let cmd = format!(
        "cat {}",
        (0..10)
            .map(|n| format!("p{n}.png"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let out = image_command(&cmd, dir.path()).expect("the capped prefix must still claim");
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.images.len(), 8, "{}", out.content);
    assert!(
        out.content.contains("showing first 8 of 10 paths"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn gray_view_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), png_bytes()).unwrap();
    // `cat` is the one way in; gray subcommands are ordinary shell commands.
    for cmd in ["gray view a.png", "gray --version", "gray memory list"] {
        assert!(
            image_command(cmd, dir.path()).is_none(),
            "must not claim: {cmd}"
        );
    }
}

#[tokio::test]
async fn cat_through_execute_shows_vision() {
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

#[test]
fn unattached_media_note_covers_every_shape_the_claim_refuses() {
    // Shape refusals: the shell runs `cat` while nothing is attached — the
    // note must say so and name the re-run.
    for cmd in [
        "cd /tmp && cat a.png",
        "cat a.png | wc -c",
        "cat a.png && wc -c",
        "x && cat a.png",
        "cat *.png",
    ] {
        let note = unattached_media_note(cmd).unwrap_or_else(|| panic!("note missing: {cmd}"));
        assert!(note.contains("NOT attached"), "{cmd}: {note}");
        assert!(note.contains("cat "), "{cmd}: {note}");
    }
    // The bare claim shapes: a miss is a missing/undecodable file the shell
    // already reports, a non-media `cat`, or no media command at all.
    for cmd in [
        "gray view a.png",
        "cat a.png",
        "cat missing.png",
        "cat notes.txt",
        "ls -la",
    ] {
        assert!(unattached_media_note(cmd).is_none(), "spurious: {cmd}");
    }
    // The advice names the paths it saw, so one turn is enough to recover.
    let note = unattached_media_note("cd /tmp && cat a.png").unwrap();
    assert!(note.contains("cat a.png"), "{note}");
}

#[tokio::test]
async fn compound_cat_says_the_image_was_not_attached() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("shot.png"), png_bytes()).unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        ..ToolContext::default()
    };
    // cd fails, so `cat` never runs: the note is the tool's own, and the
    // output must not read as a successful view.
    let out = BashTool::default()
        .execute(
            &ctx,
            json!({"command": "cd /nonexistent-9f2a && cat shot.png"}),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.images.is_empty());
    assert!(out.content.contains("NOT attached"), "{}", out.content);
}

#[tokio::test]
async fn cat_expands_a_tilde_the_shell_would_have() {
    // The fast path runs before the shell, so `~` never gets expanded: without
    // this, `cat ~/shot.png` streams binary garbage.
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let named = dir.path().join("named.png");
    std::fs::write(&named, png_bytes()).unwrap();
    assert!(
        std::path::Path::new(&home).is_dir(),
        "$HOME must be a directory for the expansion to resolve"
    );
    assert_eq!(expand_tilde("~/shot.png"), Some(format!("{home}/shot.png")));
    assert_eq!(expand_tilde("~"), Some(home.clone()));
    assert_eq!(expand_tilde("~/a/b.png"), Some(format!("{home}/a/b.png")));
    // Not ours to expand.
    assert_eq!(expand_tilde("~other/shot.png"), None);
    assert_eq!(
        expand_tilde("~/shot.png ".trim()),
        Some(format!("{home}/shot.png"))
    );
    assert_eq!(expand_tilde("plain.png"), None);
    assert_eq!(expand_tilde("./x.png"), None);
}

#[tokio::test]
async fn cat_resolves_a_tilde_path_only_when_the_file_is_there() {
    // The pointer still needs the real file: a `cat ~/typo.png` must fall
    // through to the shell so its own "no such file" is what the model reads.
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() || !std::path::Path::new(&home).is_dir() {
        return;
    }
    let at_home = tempfile::Builder::new()
        .prefix("gray-rs-tilde-probe-")
        .suffix(".png")
        .tempfile_in(&home)
        .expect("unique probe file in $HOME");
    // Create-new semantics: a random name that never overwrites user data.
    // The handle deletes the file on drop, so no manual remove can race it.
    std::fs::write(at_home.path(), png_bytes()).unwrap();
    let name = at_home
        .path()
        .file_name()
        .expect("probe has a file name")
        .to_string_lossy()
        .into_owned();
    let out = image_command(&format!("cat ~/{name}"), Path::new("."))
        .expect("cat ~/probe.png must show the image, not stream bytes");
    assert_eq!(out.images.len(), 1, "{}", out.content);
    assert!(out.content.contains(&name), "{}", out.content);

    // A `~` path that is not there falls through to the shell's own error.
    assert!(
        image_command("cat ~/definitely-not-here-9f2a.png", Path::new(".")).is_none(),
        "a missing path must fall through so the shell reports it"
    );
}

#[tokio::test]
async fn a_cd_carries_carries_into_the_next_command_in_the_same_session() {
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
fn gray_no_jobs_hides_the_managed_job_surface() {
    // GRAY_NO_JOBS=1 must shrink the schema AND refuse the actions it hides,
    // so a model that remembered them from elsewhere gets told, not a job.
    let tool = BashTool::default();
    let full = tool.def();
    let props = |d: &ToolDef| -> serde_json::Value { serde_json::to_value(&d.parameters).unwrap() };
    unsafe { std::env::set_var("GRAY_NO_JOBS", "1") };
    let lean = tool.def();
    unsafe { std::env::remove_var("GRAY_NO_JOBS") };
    let lean_props = props(&lean);
    for gone in ["action", "job_id", "background", "yield_ms", "wait_ms"] {
        assert!(
            lean_props["properties"].get(gone).is_none(),
            "{gone} still exposed: {lean_props}"
        );
        assert!(
            props(&full)["properties"].get(gone).is_some(),
            "{gone} missing with jobs on"
        );
    }
    assert!(
        lean_props["properties"].get("command").is_some(),
        "command must stay"
    );
    assert!(
        lean_props["properties"].get("timeout").is_some(),
        "timeout is the anti-hang knob"
    );
    assert!(lean_props["required"].is_null() || lean_props["required"].as_array().is_some());
    assert!(
        !lean.description.contains("action:list"),
        "{}",
        lean.description
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
fn the_cwd_report_keeps_a_trailing_heredoc_terminator_alone() {
    // `cat > f <<EOF` must stay intact: joined with `; `, the terminator line
    // read `EOF; __gray_rc=$?` and the whole suffix was written INTO the file
    // (silent corruption; 33 of 47 DeepSWE runs in the 2026-09-29 retro).
    let wrapped = with_cwd_report("cat > f <<'EOF'\nbody\nEOF");
    assert!(
        wrapped.starts_with("cat > f <<'EOF'\nbody\nEOF\n"),
        "{wrapped}"
    );
    assert!(!wrapped.contains("EOF;"), "{wrapped}");
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
    // report's `printf` would turn every failure into a success.
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

// ---------------------------------------------------------------------------
// `gray find` / `gray grep` are claimed like a media `cat`: without gray on the
// child's PATH, the model still gets the index
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_command_answers_the_find_and_grep_verbs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "needle one\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn other() {}\n").unwrap();

    let out = search_command("gray find *.txt", dir.path(), &CancellationToken::new())
        .await
        .expect("gray find must be claimed, not handed to a shell that cannot know it");
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("a.txt"), "{}", out.content);
    assert!(
        !out.content.contains("b.rs"),
        "glob must hold: {}",
        out.content
    );

    let out = search_command("gray grep needle", dir.path(), &CancellationToken::new())
        .await
        .expect("gray grep must be claimed");
    assert!(
        out.content.contains("a.txt:1: needle one"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn search_command_leaves_the_shell_its_own() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "needle one\n").unwrap();
    for cmd in [
        "gray find *.txt | wc -l",
        "gray grep --help",
        "find . -name '*.txt'",
        "rg needle",
        "gray grep needle;touch marker",
        "gray grep needle && echo done",
        "gray grep \"$(whoami)\"",
        "gray grep 'unclosed",
        "gray grep --limit",
    ] {
        assert!(
            search_command(cmd, dir.path(), &CancellationToken::new())
                .await
                .is_none(),
            "must fall through to the shell: {cmd}"
        );
    }
}

#[test]
fn search_words_splits_like_the_shell() {
    let w = |s: &str| search_words(s).map(|v| v.join("|"));
    assert_eq!(
        w("gray grep 'TODO' src").as_deref(),
        Some("gray|grep|TODO|src")
    );
    assert_eq!(
        w("gray grep \"fn main\"").as_deref(),
        Some("gray|grep|fn main")
    );
    assert_eq!(
        w("gray grep fn\\ main").as_deref(),
        Some("gray|grep|fn main")
    );
    assert_eq!(w("gray find *.txt").as_deref(), Some("gray|find|*.txt"));
    assert_eq!(w("gray grep 'a|b'").as_deref(), Some("gray|grep|a|b"));
    assert_eq!(w("gray grep a|b"), None);
}

#[tokio::test]
async fn search_command_reads_flag_values_once() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("src/b.txt"), "fn main() {}\n").unwrap();
    let run = |cmd: &'static str| {
        let d = dir.path().to_path_buf();
        async move {
            search_command(cmd, &d, &CancellationToken::new())
                .await
                .unwrap_or_else(|| panic!("must be claimed: {cmd}"))
                .content
        }
    };

    // A quoted pattern keeps its space and loses its quotes.
    let out = run("gray grep 'fn main' src").await;
    assert!(out.contains("a.rs:1: fn main"), "{out}");

    // `--glob=VALUE` takes its own value, not the next word.
    let out = run("gray grep --glob=*.rs main src").await;
    assert!(out.contains("a.rs"), "{out}");
    assert!(!out.contains("b.txt"), "glob must hold: {out}");

    // `--glob VALUE` consumes the next word, so it is not also the pattern.
    let out = run("gray grep --glob *.rs main src").await;
    assert!(out.contains("a.rs"), "{out}");
    assert!(!out.contains("b.txt"), "glob must hold: {out}");

    // `--limit N` must not leave N behind as the pattern.
    let out = run("gray grep --limit 5 main src").await;
    assert!(out.contains("a.rs:1: fn main"), "{out}");
}

#[test]
fn blocking_wait_ceiling_covers_long_suites() {
    // Benchmark probe (10 DeepSWE tasks, Sep 2026): the agent's longest
    // `sleep`-poll waits ran ~600s because the old 30s ceiling made a
    // blocking `output` wait useless for real suites. The ceiling must
    // cover those waits so one blocking call replaces N poll turns.
    assert_eq!(
        crate::shell::contract::MAX_ACTION_WAIT_MS,
        600_000,
        "wait_ms ceiling regressed below the longest observed suite wait"
    );
}

#[tokio::test]
async fn output_accepts_long_blocking_wait() {
    // wait_ms=600000 must pass arg validation (only job lookup may fail).
    let tool = BashTool::default();
    let s = sess("longwait");
    let ctx = ctx_for(&s);
    let out = tool
        .execute(
            &ctx,
            json!({"action": "output", "job_id": "nope", "wait_ms": 600000}),
        )
        .await;
    assert!(
        out.content.contains("unknown job"),
        "600s wait must reach job lookup, got: {}",
        out.content
    );
}

#[tokio::test]
async fn run_still_rejects_wait_ms() {
    // The ceiling raise must not leak wait_ms onto the run surface.
    let tool = BashTool::default();
    let s = sess("runwait");
    let ctx = ctx_for(&s);
    let out = tool
        .execute(&ctx, json!({"command": "echo hi", "wait_ms": 5000}))
        .await;
    assert!(
        out.content.contains("wait_ms is only valid"),
        "run+wait_ms must fail loudly, got: {}",
        out.content
    );
}

#[cfg(unix)]
#[tokio::test]
async fn silent_past_bound_is_handed_to_a_job_not_killed() {
    // The exact incident shape: a command that prints once, then wedges in a
    // library call (Playwright's browser.close()). With no explicit timeout and
    // an injected `bound` of silence, the blocking lane must not wait forever:
    // it stops blocking, hands the STILL-RUNNING child to the background lane
    // (never killed), and the agent's decision to cancel is what reaps it.
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("bash-stall.log");
    // `echo ok` produces output, then the child goes silent — the gap the stall
    // arm measures from there, so `sleep 60` comfortably outlives the 1s bound.
    let command = "echo ok; sleep 60";
    let spawned = spawn(command, Path::new("/"), None, None).expect("spawn");
    let pgid = spawned.pgid;

    let tool = BashTool::default();
    let ctx = ToolContext::default();
    let start = Instant::now();
    let t0 = Instant::now();
    let out = run_command(
        command.to_string(),
        log.clone(),
        None, // no explicit timeout: the stall arm is what bounds this call
        start,
        ctx.clone(),
        spawned,
        crate::shell::kill::GroupGuard::new(pgid),
        Some(&tool.jobs),
        Duration::from_secs(1),
        Duration::ZERO,
        None,
    )
    .await;

    // Stopped blocking well before the 60s child could finish on its own.
    assert!(
        t0.elapsed() < Duration::from_secs(20),
        "a silent command must stop blocking: {:?}",
        t0.elapsed()
    );
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.starts_with("still running"),
        "handoff note leads the result: {}",
        out.content
    );
    assert!(
        out.content.contains("no new output for 1s"),
        "liveness note names the silence: {}",
        out.content
    );

    // Landed in the job registry as a running background job.
    let (cancel, mut rx) = {
        let jobs = tool.jobs.0.lock().unwrap();
        let job = jobs.values().next().expect("handed-off job is registered");
        assert!(
            job.yielded,
            "handed-off job participates in completion notices"
        );
        assert!(
            job.result.borrow().is_none(),
            "the job is still running, not finished"
        );
        (job.cancel.clone(), job.result.clone())
    };

    // Its process group is ALIVE: the handoff never killed the child.
    assert_eq!(
        unsafe { libc::kill(pgid, 0) },
        0,
        "handed-off process group must stay alive"
    );

    // The AGENT's decision to cancel is what reaps it (never an auto-kill).
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(15), rx.wait_for(|v| v.is_some())).await;
    let mut gone = false;
    for _ in 0..80 {
        if unsafe { libc::kill(pgid, 0) } != 0 {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        gone,
        "cancel must reap the process group (no leak, no zombie)"
    );
    let final_out = tool
        .jobs
        .0
        .lock()
        .unwrap()
        .values()
        .next()
        .and_then(|j| j.result.borrow().clone());
    if let Some(out) = final_out {
        assert!(out.content.contains("cancelled"), "{}", out.content);
    }
}

/// Unix-only: the Windows runner resolves a temp path to a different
/// spelling (8.3 short name) between calls, so the ledger key never
/// matches and the repeat is not stubbed. The feature is Linux/macOS
/// today; revisit when the Windows resolver spelling is stable.
#[cfg(unix)]
#[tokio::test]
async fn a_repeated_cat_is_stubbed_once_through_the_tool() {
    // The wiring, not just the helper: the ledger lives on the tool, the stub
    // replaces the whole result, and the arm is consumed.
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\n").expect("write");
    let ctx = ctx_for(&sess("dedup"));
    let tool = BashTool::default().with_ledger(Arc::new(crate::ledger::FileLedger::new()));
    // Forward slashes: the command parser eats a bare Windows backslash path.
    let arg = file.display().to_string().replace('\\', "/");
    let cmd = json!({"command": format!("cat {arg}")});

    let first = tool.execute(&ctx, cmd.clone()).await;
    assert!(
        body(&first.content).contains("gamma"),
        "the first read is whole: {}",
        first.content
    );
    let second = tool.execute(&ctx, cmd.clone()).await;
    assert!(
        !body(&second.content).contains("gamma"),
        "the repeat is stubbed: {}",
        second.content
    );
    assert!(
        second
            .content
            .contains("unchanged since your previous read"),
        "{}",
        second.content
    );
    let third = tool.execute(&ctx, cmd).await;
    assert!(
        body(&third.content).contains("gamma"),
        "consume-on-hit: the third read runs: {}",
        third.content
    );
}

/// Unix-only: the Windows runner resolves a temp path to a different
/// spelling (8.3 short name) between calls, so the ledger key never
/// matches and the repeat is not stubbed. The feature is Linux/macOS
/// today; revisit when the Windows resolver spelling is stable.
#[cfg(unix)]
#[tokio::test]
async fn a_tool_without_a_ledger_never_stubs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "alpha\n").expect("write");
    let ctx = ctx_for(&sess("noledger"));
    let tool = BashTool::default();
    // Forward slashes: the command parser eats a bare Windows backslash path.
    let arg = file.display().to_string().replace('\\', "/");
    let cmd = json!({"command": format!("cat {arg}")});
    for _ in 0..2 {
        let out = tool.execute(&ctx, cmd.clone()).await;
        assert!(body(&out.content).contains("alpha"), "{}", out.content);
    }
}

/// PATH and `$GRAY_HOME` are process-global; these tests change both.
#[cfg(unix)]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A `cargo` on PATH that prints a build log instead of building: 400
/// progress lines, one error, one summary — the shape the cargo rule exists
/// for, without the cost of a real 400-crate build.
#[cfg(unix)]
fn fake_cargo(bin: &std::path::Path) {
    std::fs::create_dir_all(bin).expect("bin dir");
    let script = bin.join("cargo");
    std::fs::write(
        &script,
        concat!(
            "#!/bin/sh\ni=0\n",
            "while [ $i -lt 400 ]; do echo \"   Compiling crate-$i v0.1.0\"; i=$((i+1)); done\n",
            "echo 'error[E0308]: mismatched types'\n",
            "echo '    Finished dev in 42.19s'\n",
        ),
    )
    .expect("write fake cargo");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// Unix-only: the fake `cargo` is a `#!/bin/sh` script joined onto PATH
/// with `:`, neither of which the Windows runner honors.
#[cfg(unix)]
#[tokio::test]
async fn a_noisy_command_is_squeezed_and_its_log_keeps_everything() {
    // The whole contract in one run: what enters the context shrinks and says
    // so, and the raw log the shell already writes is untouched, so the
    // squeeze costs the model nothing it cannot grep back.
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    let bin = dir.path().join("bin");
    fake_cargo(&bin);
    let prev_home = std::env::var("GRAY_HOME").ok();
    let prev_path = std::env::var("PATH").ok();
    unsafe {
        std::env::set_var("GRAY_HOME", &home);
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                prev_path.clone().unwrap_or_default()
            ),
        );
    }
    let session = sess("squeeze");
    let ctx = ctx_for(&session);
    let r = BashTool::default()
        .execute(&ctx, json!({"command": "cargo build"}))
        .await;
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("GRAY_HOME", v),
            None => std::env::remove_var("GRAY_HOME"),
        }
        match prev_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
    }

    assert!(!r.is_error, "{}", r.content);
    assert!(
        r.content.contains("squeezed by cargo"),
        "the squeeze must be disclosed: {}",
        r.content
    );
    assert!(
        r.content.contains("progress ×400"),
        "400 lines must count as one: {}",
        r.content
    );
    // The payload survives compression untouched.
    assert!(r.content.contains("error[E0308]"), "{}", r.content);
    assert!(r.content.contains("Finished dev"), "{}", r.content);

    // …and the log still holds every line, so `grep` on it recovers the rest.
    let log_dir = home.join("shell").join(&session);
    let log: Vec<_> = std::fs::read_dir(&log_dir)
        .expect("session log dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .collect();
    assert_eq!(log.len(), 1, "one log per command: {log_dir:?}");
    let text = std::fs::read_to_string(&log[0]).expect("read log");
    assert_eq!(
        text.lines()
            .filter(|l| l.contains("Compiling crate-"))
            .count(),
        400,
        "the raw log must keep every line compression collapsed"
    );
    assert!(text.contains("crate-399"), "including the last one");
}
