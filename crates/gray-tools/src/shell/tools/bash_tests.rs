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
        .execute(&ctx, json!({"command": "echo hello | cat"}))
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
async fn cat_keeps_native_resolution() {
    // No resolution cap anywhere: images pass through at native size; only
    // the 5MB provider byte limit can still shrink them.
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
    assert_eq!(
        (w, h),
        (2400, 100),
        "cat passes native resolution, got {w}x{h}"
    );
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
async fn gray_subcommands_are_not_claimed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), png_bytes()).unwrap();
    // `cat` is the one way in; gray subcommands are ordinary shell commands.
    for cmd in [
        "gray plugin install a.png",
        "gray --version",
        "gray memory list",
    ] {
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
        "gray plugin a.png",
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
    let r = tool
        .execute(&ctx, json!({"command": "echo hello | cat"}))
        .await;
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
    let r = tool
        .execute(&ctx, json!({"command": "printf '' | cat"}))
        .await;
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

// ---- timeout semantics: hand off on the jobs lane, kill in bare mode ----

/// Spawn `command` and run it through [`run_command`] directly, so a test
/// picks the lane (jobs or bare) without touching process-global env.
#[cfg(unix)]
async fn run_lane(
    tool: &BashTool,
    ctx: &ToolContext,
    command: &str,
    secs: Option<u64>,
    jobs_lane: bool,
) -> (ToolOutput, i32, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap().keep();
    let log = dir.join("bash-lane.log");
    // Exported like production: the command itself can write the report.
    let report = dir.join("cwd-report.txt");
    let spawned =
        spawn(command, Path::new("/"), ctx.session_id.as_deref(), Some(&report)).expect("spawn");
    let pgid = spawned.pgid;
    let out = run_command(
        command.to_string(),
        log.clone(),
        secs,
        Instant::now(),
        ctx.clone(),
        spawned,
        crate::shell::kill::GroupGuard::new(pgid),
        jobs_lane.then_some(&*tool.jobs),
        report.clone(),
    )
    .await;
    (out, pgid, log, report)
}

#[cfg(unix)]
async fn wait_for_jobs(tool: &BashTool, ctx: &ToolContext) {
    for _ in 0..100 {
        if tool.jobs.running(ctx).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("job never settled");
}

/// The /tmp/gray-cwd-*.txt leak: a stalled command's trailing printf writes
/// the report AFTER the foreground path already cleaned up — the job waiter
/// owns its deletion now.
#[cfg(unix)]
#[tokio::test]
async fn a_stalled_commands_late_cwd_report_is_deleted_when_the_job_settles() {
    let tool = BashTool::default();
    let ctx = ctx_for(&sess("cwd-report-job"));
    let (out, _pgid, _log, report) = run_lane(
        &tool,
        &ctx,
        "sleep 2; printf done > \"$GRAY_CWD_REPORT\"",
        Some(1),
        true,
    )
    .await;
    assert!(out.content.starts_with("still running"), "{}", out.content);
    wait_for_jobs(&tool, &ctx).await;
    assert!(!report.exists(), "cwd report leaked: {report:?}");
}

#[cfg(unix)]
fn group_alive(pgid: i32) -> bool {
    unsafe { libc::kill(-pgid, 0) == 0 }
}

#[cfg(unix)]
async fn wait_group_gone(pgid: i32) -> bool {
    for _ in 0..100 {
        if !group_alive(pgid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[cfg(unix)]
#[tokio::test]
async fn bare_timeout_kills_and_says_how_to_rerun() {
    // GRAY_NO_JOBS is mini-swe-agent proper: a timeout is a kill, the partial
    // output survives, and the note says how to let it finish (the benchmark
    // retro showed agents reading a kill as "the command failed").
    let tool = BashTool::default();
    let ctx = ctx_for(&sess("bare-timeout"));
    let t0 = Instant::now();
    let (r, pgid, _, _) = run_lane(&tool, &ctx, "echo out; sleep 30", Some(1), false).await;
    assert!(t0.elapsed() < Duration::from_secs(15), "a kill, not a wait");
    assert!(!r.is_error, "killed command stays a result: {}", r.content);
    assert!(r.content.starts_with("timed out after 1s"), "{}", r.content);
    assert!(
        r.content.contains("rerun without `timeout`"),
        "{}",
        r.content
    );
    assert!(r.content.contains("out"), "{}", r.content);
    assert!(wait_group_gone(pgid).await, "the whole group is killed");
    assert!(
        tool.jobs.0.lock().unwrap().is_empty(),
        "bare mode makes no job"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_hands_off_to_a_job_never_kills() {
    // The jobs lane: reaching the timeout stops the CALL, not the command.
    // The notice carries the job, pgid, log, the in-band stop command and the
    // output so far; the group stays alive until someone stops it.
    let tool = BashTool::default();
    let ctx = ctx_for(&sess("handoff"));
    let t0 = Instant::now();
    let (out, pgid, log, _report) = run_lane(&tool, &ctx, "echo early; sleep 60", Some(1), true).await;
    assert!(
        t0.elapsed() < Duration::from_secs(15),
        "the call returns at its timeout"
    );
    assert!(!out.is_error, "{}", out.content);
    let first = out.content.lines().next().unwrap_or("");
    assert!(first.starts_with("still running \u{b7} job "), "{first}");
    assert!(first.contains("yielded after 1s"), "{first}");
    assert!(first.contains(&format!("pgid {pgid}")), "{first}");
    assert!(first.contains(&log.display().to_string()), "{first}");
    assert!(out.content.contains("Not killed"), "{}", out.content);
    assert!(out.content.contains("end your turn"), "{}", out.content);
    assert!(
        out.content.contains(&format!("Stop: `kill -- -{pgid}`")),
        "the stop command must take the whole group: {}",
        out.content
    );
    assert!(
        out.content.contains("Partial output (snapshot):"),
        "{}",
        out.content
    );
    assert!(out.content.contains("early"), "{}", out.content);
    for gone in ["action:", "wait_ms", "job_id"] {
        assert!(!out.content.contains(gone), "{gone}: {}", out.content);
    }
    assert!(group_alive(pgid), "handed-off group must stay alive");
    assert_eq!(tool.running_jobs(&ctx).len(), 1);
    assert!(tool.has_unfinished_jobs(&ctx), "the turn-end hold sees it");

    // Cancellation (session cancel or /jobs) still reaps the whole group.
    let id = tool.running_jobs(&ctx)[0].id.clone();
    assert!(tool.cancel_job(&ctx, &id));
    assert!(
        wait_group_gone(pgid).await,
        "cancel reaps the process group"
    );
    let notices = wait_notices(&tool, &ctx).await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains(&format!("Background job {id} finished (cancelled)")),
        "{notices:?}"
    );
}

#[cfg(unix)]
async fn wait_notices(tool: &BashTool, ctx: &ToolContext) -> Vec<String> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let n = tool.drain_notifications(ctx);
            if !n.is_empty() {
                return n;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a finished job must be reported")
}

#[cfg(unix)]
#[tokio::test]
async fn the_advertised_stop_command_kills_the_whole_tree_in_band() {
    // The model has no cancel action: it runs the `kill -- -<pgid>` the
    // notice gave it through the tool itself. That must take the group, not
    // just the `sh` wrapper (a bare `kill <pgid>` leaves the grandchild).
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        cwd: dir.path().into(),
        session_id: Some(sess("inband-kill")),
        ..ToolContext::default()
    };
    let tool = BashTool::default();
    let out = tool
        .execute(
            &ctx,
            json!({"command": "(sleep 2; touch escaped) & sleep 60; wait", "timeout": 1}),
        )
        .await;
    assert!(out.content.starts_with("still running"), "{}", out.content);
    let stop = out
        .content
        .split("Stop: `")
        .nth(1)
        .and_then(|s| s.split('`').next())
        .expect("stop command")
        .to_string();
    assert!(stop.starts_with("kill -- -"), "{stop}");
    let killed = tool.execute(&ctx, json!({"command": stop.clone()})).await;
    assert!(
        killed.content.starts_with("stopped job "),
        "the advertised stop is handled here, never by a (possibly remote) shell: {}",
        killed.content
    );
    // A second run of the same command no longer matches a live job, so it
    // is just a shell command again (and finds no such group).
    let again = tool.execute(&ctx, json!({"command": stop})).await;
    assert!(
        !again.content.starts_with("stopped job"),
        "{}",
        again.content
    );
    let notices = wait_notices(&tool, &ctx).await;
    assert!(notices[0].contains("finished ("), "{notices:?}");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(
        !dir.path().join("escaped").exists(),
        "the grandchild escaped the kill"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_finished_job_reports_its_exit_and_log_never_its_output() {
    let tool = BashTool::default();
    let ctx = ctx_for(&sess("notice"));
    let (out, _, log, _report) = run_lane(
        &tool,
        &ctx,
        "echo IGNORE-PREVIOUS-INSTRUCTIONS; sleep 1.5; exit 3",
        Some(1),
        true,
    )
    .await;
    assert!(out.content.starts_with("still running"), "{}", out.content);
    let notices = wait_notices(&tool, &ctx).await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    let n = &notices[0];
    assert!(n.contains("finished (exit 3)"), "{n}");
    assert!(
        n.contains(&log.display().to_string()),
        "the log is where the output is: {n}"
    );
    assert!(
        !n.contains("IGNORE"),
        "output never becomes a user-role message: {n}"
    );
    assert!(!n.contains("action:"), "{n}");
    assert!(tool.drain_notifications(&ctx).is_empty(), "once only");
    assert!(!tool.has_unfinished_jobs(&ctx));
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("IGNORE-PREVIOUS-INSTRUCTIONS"),
        "the log keeps it"
    );
}

#[tokio::test]
async fn a_finished_command_inside_the_timeout_is_an_ordinary_result() {
    let tool = BashTool::default();
    let ctx = ctx_for(&sess("inline"));
    let out = tool
        .execute(
            &ctx,
            json!({"command": "sleep 0.2; echo done", "timeout": 30}),
        )
        .await;
    assert!(out.content.starts_with("exit 0"), "{}", out.content);
    assert!(out.content.contains("done"), "{}", out.content);
    assert!(tool.running_jobs(&ctx).is_empty());
    assert!(tool.drain_notifications(&ctx).is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn running_jobs_ride_along_on_later_results() {
    // The replacement for action:list: while a job runs, every later bash
    // result names it with its log and stop command, so a model that lost
    // the original notice (compaction, a long detour) can still find it.
    let tool = BashTool::default();
    let ctx = ctx_for(&sess("footer"));
    let (out, pgid, log, _report) = run_lane(&tool, &ctx, "sleep 60", Some(1), true).await;
    assert!(out.content.starts_with("still running"), "{}", out.content);
    let id = tool.running_jobs(&ctx)[0].id.clone();
    let later = tool
        .execute(&ctx, json!({"command": "echo hi | cat"}))
        .await;
    assert!(
        later.content.starts_with("exit 0"),
        "the result itself leads: {}",
        later.content
    );
    let tail = later.content.lines().last().unwrap_or("");
    assert!(
        tail.starts_with(&format!("background job {id} still going")),
        "{tail}"
    );
    assert!(tail.contains(&log.display().to_string()), "{tail}");
    assert!(tail.contains(&format!("stop: `kill -- -{pgid}`")), "{tail}");
    // Another session never sees it.
    let other = ctx_for(&sess("footer-other"));
    let theirs = tool
        .execute(&other, json!({"command": "echo hi | cat"}))
        .await;
    assert!(
        !theirs.content.contains("background job"),
        "{}",
        theirs.content
    );
    // A no-op names it too, and says how to wait.
    let noop = tool.execute(&ctx, json!({"command": "true"})).await;
    assert!(noop.content.contains("no-op"), "{}", noop.content);
    assert!(noop.content.contains("end your turn"), "{}", noop.content);
    assert!(
        noop.content.contains(&format!("background job {id}")),
        "{}",
        noop.content
    );
    tool.cancel_job(&ctx, &id);
    assert!(wait_group_gone(pgid).await);
    wait_notices(&tool, &ctx).await;
    let after = tool
        .execute(&ctx, json!({"command": "echo hi | cat"}))
        .await;
    assert!(
        !after.content.contains("background job"),
        "finished jobs drop off: {}",
        after.content
    );
}

#[tokio::test]
async fn bare_noop_commands_are_steered_not_spawned() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        cwd: dir.path().into(),
        ..ToolContext::default()
    };
    for cmd in [
        "true",
        ":",
        " true ",
        // A lone print is a placeholder too: the model "announces" work
        // (echo "switching tools") and the real calls never arrive.
        "echo hi",
        "echo \"switching to harness tools\"",
        "printf 'done\\n'",
    ] {
        let out = BashTool::default()
            .execute(&ctx, json!({"command": cmd}))
            .await;
        assert!(!out.is_error, "{cmd}: {}", out.content);
        assert!(out.content.contains("no-op"), "{cmd}: {}", out.content);
        assert!(
            out.content.contains("No background jobs"),
            "{cmd}: {}",
            out.content
        );
    }
    // A compound that merely contains a no-op still reaches the shell.
    for cmd in [
        "true && echo ran",
        "echo ran > gray-noop-test-out",
        "echo a; echo b",
        "echo x | wc -c",
        "echo $((40 + 2))",
        "env echo ran",
    ] {
        let out = BashTool::default()
            .execute(&ctx, json!({"command": cmd}))
            .await;
        assert!(out.content.contains("exit 0"), "{cmd}: {}", out.content);
    }
    let out_file = ctx.cwd.join("gray-noop-test-out");
    assert_eq!(std::fs::read_to_string(&out_file).unwrap(), "ran\n");
    std::fs::remove_file(&out_file).ok();
}

#[tokio::test]
async fn removed_arguments_fail_loudly_and_never_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        cwd: dir.path().into(),
        session_id: Some(sess("removed")),
        ..ToolContext::default()
    };
    let tool = BashTool::default();
    for args in [
        json!({"command": "touch BAD", "background": true}),
        json!({"command": "touch BAD", "background": "true"}),
        json!({"command": "touch BAD", "wait": 5}),
        json!({"command": "touch BAD", "action": "output", "job_id": "x"}),
        json!({"action": "list"}),
        json!({"action": "cancel", "job_id": "x"}),
        json!({"command": "touch BAD", "task_id": "t"}),
        json!({"command": "touch BAD", "from_offset": 3}),
        json!({"command": "touch BAD", "notify_on": "exit"}),
        json!({"command": "touch BAD", "run_in_background": true}),
        // Wait/yield windows still fail when nothing is being run: there
        // the window is the whole call, so the pointer text is worth it.
        json!({"wait_ms": 5000}),
        json!({"yield_ms": 1000}),
    ] {
        let out = tool.execute(&ctx, args.clone()).await;
        assert!(out.is_error, "{args}: {}", out.content);
        assert!(
            out.content.contains("is not a bash argument; remove it"),
            "{args}: {}",
            out.content
        );
        assert!(
            out.content.contains("end your turn"),
            "says how to wait instead: {}",
            out.content
        );
    }
    assert!(
        !dir.path().join("BAD").exists(),
        "a rejected call never runs"
    );
    // Values that carry no intent, and a stray job_id, are not worth a
    // failed call (rejecting job_id looped real sessions, 2026-09-17).
    for args in [
        json!({"command": "echo fine | cat", "background": false}),
        json!({"command": "echo fine | cat", "action": "run"}),
        json!({"command": "echo fine | cat", "yield_ms": null}),
        json!({"command": "echo fine | cat", "job_id": "bash-bogus"}),
        // Echoed wait/yield windows on a run drop silently: the call blocks
        // to exit or timeout anyway, and rejecting them looped schema-filling
        // models to death (gpt-6 fills every property on every call).
        json!({"command": "echo fine | cat", "wait_ms": 5000}),
        json!({"command": "echo fine | cat", "yield_ms": 1000}),
        json!({"command": "echo fine | cat", "yield_time_ms": 10000}),
        // The exact full-schema blob gpt-6-sol sent.
        json!({"action":"run","background":false,"command":"echo fine | cat","job_id":"","timeout":10,"wait_ms":1000,"yield_ms":1000}),
    ] {
        let out = tool.execute(&ctx, args.clone()).await;
        assert!(!out.is_error, "{args}: {}", out.content);
        assert!(out.content.contains("fine"), "{args}: {}", out.content);
    }
}

#[test]
fn the_schema_is_command_and_timeout_only() {
    for jobs in [true, false] {
        let def = tool_def(jobs);
        let props = serde_json::to_value(&def.parameters).unwrap();
        let keys: Vec<&String> = props["properties"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["command", "timeout"], "jobs={jobs}");
        assert!(
            !def.description.contains("action"),
            "jobs={jobs}: {}",
            def.description
        );
    }
    let def = tool_def(true);
    for must in [
        "NOT killed",
        "end your turn",
        "a finished job wakes you",
        "no polling needed",
        "tail <log>",
        "pass a `timeout` longer than the sleep",
        "timeout N cmd",
    ] {
        assert!(
            def.description.contains(must),
            "{must}: {}",
            def.description
        );
    }
    #[cfg(not(windows))]
    assert!(
        def.description.contains("--reminder"),
        "{}",
        def.description
    );
    let timeout = def.parameters["properties"]["timeout"]["description"]
        .as_str()
        .unwrap();
    assert!(
        timeout.contains(&format!("default {DEFAULT_TIMEOUT_SECS}")),
        "{timeout}"
    );
    assert!(timeout.contains("never kills"), "{timeout}");
    let bare = tool_def(false);
    assert!(
        bare.description.contains("no default timeout"),
        "{}",
        bare.description
    );
}

#[test]
fn default_timeout_is_finite_and_never_a_kill() {
    // Without the old 30s auto-yield, an omitted `timeout` must still bound
    // the call, or an un-timed `cargo build` blocks the turn forever.
    const { assert!(DEFAULT_TIMEOUT_SECS < MAX_TIMEOUT_SECS) };
    assert_eq!(DEFAULT_TIMEOUT_SECS, 120);
}

#[test]
fn a_leading_sleep_stretches_the_default_bound() {
    // `sleep 300 && tail log` is the in-band wait; with the 120s default it
    // would itself turn into a job (the original bug). It blocks as written.
    let d = DEFAULT_TIMEOUT_SECS;
    let slack = SLEEP_SLACK_SECS;
    for (cmd, want) in [
        ("cargo build", d),
        ("sleep 5", d),
        ("sleep 300", 300 + slack),
        ("sleep 300 && tail log", 300 + slack),
        ("sleep 300; tail log", 300 + slack),
        ("sleep 300 || true", 300 + slack),
        ("  sleep\t200\ntail log", 200 + slack),
        ("sleep 5m && tail log", 300 + slack),
        ("sleep 1.5m", 90 + slack),
        ("sleep 2h", MAX_TIMEOUT_SECS),
        ("sleep 99999", MAX_TIMEOUT_SECS),
        // Not a foreground wait: the sleep is backgrounded or not a sleep.
        ("sleep 300 & cargo build", d),
        ("sleep 300 | cat", d),
        ("sleepy 300", d),
        ("sleep $N", d),
        ("echo x; sleep 300", d),
        // Leading `cd` steps are not the wait; the sleep after them is.
        ("cd /repo && sleep 300 && tail log", 300 + slack),
        ("cd /repo; sleep 300; tail log", 300 + slack),
        ("cd ~/gray && cd crates\nsleep 200", 200 + slack),
        ("cd '/my dir' && sleep 300", d),
        ("cd $(pwd) && sleep 300", d),
        ("cd /repo || sleep 300", d),
        ("cd /repo && cargo build && sleep 300", d),
        ("cdx && sleep 300", d),
    ] {
        assert_eq!(block_bound(cmd), want, "{cmd:?}");
    }
}
