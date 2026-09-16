use super::*;

#[test]
fn pipe_splits_but_or_or_and_chain_do_not() {
    assert_eq!(split_pipeline("false | tail -1").len(), 2);
    assert_eq!(split_pipeline("a || b").len(), 1);
    assert_eq!(split_pipeline("echo a && rm -rf /").len(), 1);
    assert_eq!(split_pipeline("sleep 1 & echo done").len(), 1);
    assert_eq!(split_pipeline("a; b").len(), 1);
}

#[test]
fn quoted_operators_and_subshell_depth_stay_literal() {
    assert_eq!(split_pipeline("echo 'a|b'"), ["echo 'a|b'"]);
    assert_eq!(split_pipeline("echo \"a|b\"").len(), 1);
    assert_eq!(split_pipeline("echo $(echo a | b)"), ["echo $(echo a | b)"]);
}

#[test]
fn heredoc_body_is_opaque_but_same_line_pipe_splits() {
    assert_eq!(split_pipeline("cat <<EOF\na && rm -rf /\nEOF").len(), 1);
    assert_eq!(split_pipeline("cat <<EOF | tail -1").len(), 2);
}
