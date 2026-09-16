use super::*;

#[test]
fn tokens_are_bytes_over_four() {
    assert_eq!(est_tokens(51_200), 12_800);
    assert_eq!(est_tokens(7), 1);
}

#[test]
fn line_format_matches_spec() {
    let s = ToolStats {
        tool: "read",
        path: "long.txt",
        bytes: 51_200,
        lines: 1846,
        truncated_by: CUT_BYTES,
    };
    assert_eq!(
        s.line(),
        "tool=read path=long.txt bytes=51200 lines=1846 \
             est_tokens=12800 truncated_by=bytes"
    );
}
