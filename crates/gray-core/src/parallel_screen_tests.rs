//! Bash-screen tests: stderr redirects stay batchable; inline-code,
//! fetch-and-write, and script execution demote.

use crate::parallel::bash_is_batchable;

#[test]
fn stderr_only_redirects_stay_batchable() {
    assert!(bash_is_batchable("git diff 2>&1"));
    assert!(bash_is_batchable("ls 2>/dev/null"));
    assert!(bash_is_batchable("cargo metadata 2>&1"));
}

#[test]
fn file_redirects_still_demotes() {
    assert!(!bash_is_batchable("git diff > out.txt"));
    assert!(!bash_is_batchable("ls 2> err.txt"));
    assert!(!bash_is_batchable("echo hi &> all.txt"));
    assert!(!bash_is_batchable("ls >> out.txt 2>&1"));
}

#[test]
fn inline_interpreter_code_demotes() {
    assert!(!bash_is_batchable("python -c \"open('f','w').write('x')\""));
    assert!(!bash_is_batchable("python3 -c 'pass'"));
    assert!(!bash_is_batchable("node -e 'console.log(1)'"));
}

#[test]
fn bare_interpreters_stay_allowed() {
    assert!(bash_is_batchable("python --version"));
    assert!(bash_is_batchable("node --version"));
    // `-e` without an interpreter is someone else's flag (`sed -e`).
    assert!(bash_is_batchable("sed -e 's/a/b/' file.txt"));
}

#[test]
fn wget_demotes_curl_only_with_output_flag() {
    assert!(!bash_is_batchable("wget https://example.com/f"));
    assert!(bash_is_batchable("curl https://example.com/f"));
    assert!(!bash_is_batchable("curl -o f https://example.com/f"));
    assert!(!bash_is_batchable("curl -sSO https://example.com/f"));
    assert!(!bash_is_batchable("curl --output f https://example.com/f"));
}

#[test]
fn script_execution_demotes() {
    assert!(!bash_is_batchable("./run.sh"));
    assert!(!bash_is_batchable("../build.sh"));
    assert!(!bash_is_batchable("sh build.sh"));
    assert!(!bash_is_batchable("bash deploy.sh --prod"));
}

#[test]
fn shell_c_string_and_version_stay_allowed() {
    // The `-c` payload is screened as part of the whole command string.
    assert!(bash_is_batchable("bash -c 'ls'"));
    assert!(bash_is_batchable("bash --version"));
    // ...while a mutating payload still demotes via the word screen.
    assert!(!bash_is_batchable("bash -c 'rm -rf x'"));
}
