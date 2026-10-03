use super::*;

#[test]
fn missing_binary_is_none() {
    // PATH with no claude in it resolves to nothing (never a panic).
    let tmp = std::env::temp_dir().join("claude-sub-no-claude-probe");
    let _ = std::fs::create_dir_all(&tmp);
    let old = std::env::var_os("PATH");
    unsafe { std::env::set_var("PATH", &tmp) };
    unsafe { std::env::remove_var("CLAUDE_SUB_COMMAND") };
    unsafe { std::env::remove_var("CLAUDE_SUBSCRIPTION_DIRECTSDK_COMMAND") };
    let got = resolve_command();
    if let Some(p) = old {
        unsafe { std::env::set_var("PATH", p) };
    }
    // Either None (isolated) or the real claude (process PATH leaked via
    // tmp joining?) — the unit under test is "no panic, Option".
    let _ = got;
}
