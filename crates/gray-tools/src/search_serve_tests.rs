#![cfg(unix)]

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use super::{Answer, grep_answer};

fn write_tree(root: &Path) {
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("a.py"),
        "def needle():\n    return 'needle'\n\nsecond needle line\ntrailing  \n",
    )
    .unwrap();
    std::fs::write(src.join("b.py"), "# needle again\nno match here\n").unwrap();
    std::fs::write(src.join("bin.dat"), b"ab\x00needle\x00cd").unwrap();
    std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
    std::fs::write(root.join(".git/hooks/x.sample"), "needle in git\n").unwrap();
    std::fs::create_dir_all(root.join("deep/d1/d2")).unwrap();
    std::fs::write(root.join("deep/d1/d2/c.txt"), "deep needle\n").unwrap();
}

fn real_grep(argv: &[String], cwd: &Path) -> (Vec<u8>, Vec<u8>, i32) {
    let out = Command::new("grep")
        .args(argv)
        .current_dir(cwd)
        .output()
        .unwrap();
    (out.stdout, out.stderr, out.status.code().unwrap_or(-1))
}

#[test]
fn grep_answer_matches_gnu() {
    let work = tempfile::TempDir::new().unwrap();
    let idx = tempfile::TempDir::new().unwrap();
    write_tree(work.path());
    let engine = fsearch::Engine::start(fsearch::Options {
        dir: idx.path().to_path_buf(),
        home: work.path().to_string_lossy().into_owned(),
        root: Some(work.path().to_path_buf()),
        skip: Some(Vec::new()),
    })
    .unwrap();

    let deadline = Instant::now() + std::time::Duration::from_secs(90);
    loop {
        let st = engine.status();
        if st.ready && st.content_pending == 0 && st.content_docs > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "index never became ready");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let cases: Vec<Vec<String>> = vec![
        argv(&["-rn", "needle", "."]),
        argv(&["-rni", "NEEDLE", "."]),
        argv(&["-rl", "needle", "."]),
        argv(&["-ro", "needle", "."]),
        argv(&["-rc", "needle", "."]),
        argv(&["-rcl", "needle", "."]),
        argv(&["-rnC1", "needle", "."]),
        argv(&["-rEn", "nee[a-z]le", "."]),
        argv(&["-rFn", "needle", "."]),
        argv(&["-rn", "needle", "src", "deep"]),
        argv(&["-rn", "needle", "src/a.py"]),
        argv(&["-q", "-r", "needle", "."]),
        argv(&["-rI", "-n", "needle", "."]),
    ];
    let mut served = 0;
    for args in &cases {
        match grep_answer(&engine, args, work.path()) {
            Some(Answer::Print {
                stdout,
                stderr,
                code,
            }) => {
                served += 1;
                let (rout, rerr, rcode) = real_grep(args, work.path());
                assert_eq!(
                    (stdout.as_slice(), stderr.as_slice(), code),
                    (rout.as_slice(), rerr.as_slice(), rcode),
                    "mismatch on {args:?}\n--- ours:\n{}{}\n--- real:\n{}{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr),
                    String::from_utf8_lossy(&rout),
                    String::from_utf8_lossy(&rerr),
                );
            }
            Some(Answer::Exec { .. }) => panic!("grep_answer returned Exec for {args:?}"),
            None => {
                let ok = args.last().map(String::as_str) == Some("src/a.py")
                    || args
                        .iter()
                        .any(|a| a.starts_with('-') && a[1..].chars().any(|c| "oCc".contains(c)));
                assert!(ok, "declined {args:?}");
            }
        }
    }
    assert!(served >= 8, "too few cases served: {served}");
}

fn write_rg_tree(root: &Path) {
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("a.py"),
        "def needle():\n    return 'needle'\n\nsecond needle line\ntrailing  \n",
    )
    .unwrap();
    std::fs::write(src.join("b.py"), "# needle again\nno match here\n").unwrap();
    std::fs::write(src.join("caps.txt"), "NEEDLE\nkelvin\n").unwrap();
    std::fs::write(
        src.join("words.txt"),
        "needlework nope\nneedle exact\nexactly needle here\n",
    )
    .unwrap();
    std::fs::write(src.join("exact.txt"), "needle\nneedlex\nxneedle\n").unwrap();

    std::fs::write(src.join("bin.dat"), b"ab\x00nomatch\x00cd").unwrap();
    std::fs::create_dir_all(root.join("ignored")).unwrap();
    std::fs::write(root.join("ignored/secret.txt"), "needle in ignored\n").unwrap();
    std::fs::write(root.join("skipped.log"), "needle in log\n").unwrap();
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    std::fs::write(root.join(".hidden/x.txt"), "needle in hidden\n").unwrap();
    std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
    std::fs::write(root.join(".git/hooks/x.sample"), "needle in git\n").unwrap();
    std::fs::create_dir_all(root.join("deep/d1/d2")).unwrap();
    std::fs::write(root.join("deep/d1/d2/c.txt"), "deep needle\n").unwrap();

    std::fs::write(root.join(".ignore"), "ignored/\n").unwrap();
    std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
}

fn real_rg(argv: &[String], cwd: &Path) -> (Vec<u8>, Vec<u8>, i32) {
    let out = Command::new("rg")
        .arg("-j1")
        .args(argv)
        .current_dir(cwd)
        .output()
        .unwrap();
    (out.stdout, out.stderr, out.status.code().unwrap_or(-1))
}

#[test]
fn rg_answer_matches_real() {
    let work = tempfile::TempDir::new().unwrap();
    let idx = tempfile::TempDir::new().unwrap();
    write_rg_tree(work.path());
    let engine = fsearch::Engine::start(fsearch::Options {
        dir: idx.path().to_path_buf(),
        home: work.path().to_string_lossy().into_owned(),
        root: Some(work.path().to_path_buf()),
        skip: Some(Vec::new()),
    })
    .unwrap();

    let deadline = Instant::now() + std::time::Duration::from_secs(90);
    loop {
        let st = engine.status();
        if st.ready && st.content_pending == 0 && st.content_docs > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "index never became ready");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let cases: Vec<Vec<String>> = vec![
        argv(&["needle", "."]),
        argv(&["-n", "needle", "."]),
        argv(&["-l", "needle", "."]),
        argv(&["-o", "needle", "."]),
        argv(&["-o", "-n", "needle", "."]),
        argv(&["-o", "-n", "--column", "needle", "."]),
        argv(&["-c", "needle", "."]),
        argv(&["--count-matches", "needle", "."]),
        argv(&["-nC1", "needle", "."]),
        argv(&["-n", "-A1", "needle", "."]),
        argv(&["-n", "-B1", "needle", "."]),
        argv(&["--column", "-n", "needle", "."]),
        argv(&["--vimgrep", "needle", "."]),
        argv(&["--heading", "needle", "."]),
        argv(&["--heading", "-n", "needle", "."]),
        argv(&["-0", "-n", "needle", "."]),
        argv(&["-0", "-l", "needle", "."]),
        argv(&["-w", "-n", "needle", "."]),
        argv(&["-x", "-n", "needle", "."]),
        argv(&["-i", "-n", "NEEDLE", "."]),
        argv(&["-S", "-n", "needle", "."]),
        argv(&["-S", "-n", "NEEDLE", "."]),
        argv(&["-F", "-n", "needle", "."]),
        argv(&["-e", "needle", "-e", "kelvin", "-n", "."]),
        argv(&["-e", "nee[a-z]le", "-n", "."]),
        argv(&["needle", "src", "deep"]),
        argv(&["-q", "needle", "."]),
        argv(&["-m1", "-n", "needle", "."]),
        argv(&["--max-depth", "1", "-n", "needle", "."]),
        argv(&["-g", "*.py", "-n", "needle", "."]),
        argv(&["-g", "!*.py", "-n", "needle", "."]),
        argv(&["--hidden", "-n", "needle", "."]),
        argv(&["-u", "-n", "needle", "."]),
        argv(&["-uu", "-n", "needle", "."]),
        argv(&["--no-ignore", "-n", "needle", "."]),
        argv(&["--no-ignore-vcs", "-n", "needle", "."]),
        argv(&["-I", "-n", "needle", "."]),
        argv(&["needle", "src/a.py"]),
        argv(&["needle", "missing"]),
    ];
    let mut served = 0;
    for args in &cases {
        match super::rg_answer(&engine, args, work.path()) {
            Some(super::Answer::Print {
                stdout,
                stderr,
                code,
            }) => {
                served += 1;
                let (rout, rerr, rcode) = real_rg(args, work.path());
                assert_eq!(
                    (stdout.as_slice(), stderr.as_slice(), code),
                    (rout.as_slice(), rerr.as_slice(), rcode),
                    "mismatch on {args:?}\n--- ours:\n{}{}\n--- real:\n{}{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr),
                    String::from_utf8_lossy(&rout),
                    String::from_utf8_lossy(&rerr),
                );
            }
            Some(super::Answer::Exec { .. }) => panic!("rg_answer returned Exec for {args:?}"),
            None => {
                let last = args.last().map(String::as_str);
                assert!(
                    last == Some("src/a.py") || last == Some("missing"),
                    "declined {args:?}"
                );
            }
        }
    }
    assert!(served >= 25, "too few cases served: {served}");
}
