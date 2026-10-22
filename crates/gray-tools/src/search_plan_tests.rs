use super::*;

fn args(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

#[test]
fn needles_are_the_longest_safe_required_run() {
    let n = |p: &str, s: Syntax, ci: bool| needles_for(p, s, ci);
    assert_eq!(n("error", Syntax::Basic, false), Some(vec!["error".into()]));
    assert_eq!(n("err.r", Syntax::Basic, false), Some(vec!["err".into()]));
    assert_eq!(n("a*bcd", Syntax::Basic, false), Some(vec!["bcd".into()]));
    assert_eq!(
        n("[0-9]+ items", Syntax::Extended, false),
        Some(vec![" items".into()])
    );
    assert_eq!(
        n("foo|barbaz", Syntax::Extended, false),
        Some(vec!["foo".into(), "barbaz".into()])
    );
    assert_eq!(
        n("a.b|x", Syntax::Extended, false),
        None,
        "a branch without a literal declines"
    );
    assert_eq!(
        n("a+b+", Syntax::Basic, false),
        Some(vec!["a+b+".into()]),
        "BRE + is plain"
    );
    assert_eq!(
        n("ab", Syntax::Fixed, false),
        None,
        "shorter than three bytes"
    );
    assert_eq!(
        n("ignore", Syntax::Fixed, true),
        None,
        "-i folds i unsafely: decline"
    );
    assert_eq!(n("Error", Syntax::Fixed, true), Some(vec!["Error".into()]));
    assert_eq!(
        n("(abc)x", Syntax::Extended, false),
        None,
        "a group is not required, and x is too short"
    );
    assert_eq!(
        n("unable\\|handle", Syntax::Basic, false),
        Some(vec!["unable".into(), "handle".into()]),
        "BRE \\| is alternation"
    );
    assert_eq!(
        n("a.b\\|x", Syntax::Basic, false),
        None,
        "a BRE branch without a literal declines"
    );
    assert_eq!(
        n("foo\\|", Syntax::Basic, false),
        None,
        "an empty BRE branch matches everything"
    );
    assert_eq!(
        n("foo|bar", Syntax::Basic, false),
        Some(vec!["foo|bar".into()]),
        "BRE | is literal"
    );
    assert_eq!(
        n("a\\(b\\|c\\)d", Syntax::Basic, false),
        None,
        "alternation in a group is not splittable"
    );
    assert_eq!(
        n("one\\|two", Syntax::Basic, true),
        Some(vec!["one".into(), "two".into()]),
        "-i: ASCII-safe folds still serve"
    );
}

#[test]
fn plans_serve_recursive_greps_and_decline_the_rest() {
    let p = grep_plan(&args("-rn foo dir")).expect("served");
    assert_eq!(p.argv, ["-n", "-e", "foo"]);
    assert_eq!(p.operands, ["dir"]);
    assert_eq!(p.needles, ["foo"]);
    assert!(p.all_needled && p.recursive && p.with_name.is_none());
    let p = grep_plan(&args("-r -h --include=*.rs -e needle .")).expect("served");
    assert_eq!(
        p.argv,
        ["-h", "--include=*.rs", "-e", "needle"],
        "-h keeps the name off; --include rides along"
    );

    let p = grep_plan(&args("foo one.c two.c")).expect("served");
    assert_eq!(p.operands, ["one.c", "two.c"]);
    assert!(!p.recursive);
    let p = grep_plan(&args("-rn foo dir1 dir2")).expect("served");
    assert_eq!(p.operands, ["dir1", "dir2"]);

    assert_eq!(grep_plan(&args("-rn foo")).expect("served").operands, ["."]);

    assert!(!grep_plan(&args("-rE a|b x")).expect("plans").all_needled);
    assert!(!grep_plan(&args("-r fo .")).expect("plans").all_needled);
    for line in ["-v foo .", "-P foo .", "foo", "foo -", "-rl foo . -E -F"] {
        assert!(grep_plan(&args(line)).is_err(), "must decline: {line}");
    }
}

#[test]
fn include_and_exclude_follow_the_last_matching_rule() {
    let p = grep_plan(&args("-r --include=*.rs --exclude=skip.rs needle .")).expect("served");
    assert!(p.walk.keeps_file(b"a.rs"));
    assert!(!p.walk.keeps_file(b"skip.rs"), "later exclude wins");
    assert!(!p.walk.keeps_file(b"a.txt"), "first rule is an include");
    let p = grep_plan(&args("-r --exclude=*.log needle .")).expect("served");
    assert!(p.walk.keeps_file(b"a.txt"));
    assert!(!p.walk.keeps_file(b"a.log"));
    assert!(!p.walk.skips_dir(b"x"));
}
