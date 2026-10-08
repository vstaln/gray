use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_string()).collect()
}

fn tpl(content: &str) -> PromptTemplate {
    PromptTemplate {
        name: "t".into(),
        description: String::new(),
        argument_hint: None,
        content: content.into(),
        path: PathBuf::from("t.md"),
        source: "user",
    }
}

#[test]
fn args_split_on_whitespace_and_group_quotes() {
    assert_eq!(
        parse_args(r#"a "b c" 'd e' f"#),
        s(&["a", "b c", "d e", "f"])
    );
    assert_eq!(parse_args("  "), Vec::<String>::new());
    assert_eq!(parse_args(r#"x"y z"w"#), s(&["xy zw"]));
}

#[test]
fn placeholders_match_pi() {
    let a = s(&["one", "two", "three"]);
    assert_eq!(substitute_args("$1-$2-$4.", &a), "one-two-.");
    assert_eq!(
        substitute_args("$@|$ARGUMENTS", &a),
        "one two three|one two three"
    );
    assert_eq!(substitute_args("${@:2}", &a), "two three");
    assert_eq!(substitute_args("${@:2:1}", &a), "two");
    assert_eq!(substitute_args("${@:9}", &a), "");
    assert_eq!(substitute_args("${4:-four} ${1:-x}", &a), "four one");
    assert_eq!(substitute_args("${@:-none}", &[]), "none");
}

#[test]
fn argument_values_are_not_rescanned() {
    assert_eq!(substitute_args("$1 $2", &s(&["$2", "b"])), "$2 b");
}

#[test]
fn templates_without_placeholders_get_args_appended() {
    assert_eq!(
        expand(&tpl("Review this.\n"), "src/lib.rs"),
        "Review this.\n\nsrc/lib.rs"
    );
    assert_eq!(expand(&tpl("Review this.\n"), "  "), "Review this.");
    assert_eq!(
        expand(&tpl("Review $1 please"), "a.rs b.rs"),
        "Review a.rs please"
    );
}

#[test]
fn frontmatter_and_first_line_descriptions() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("review.md");
    std::fs::write(
        &a,
        "---\ndescription: Review a file\nargument-hint: <path>\n---\nReview $1\n",
    )
    .unwrap();
    let t = load_template(&a, "user").unwrap();
    assert_eq!(t.name, "review");
    assert_eq!(t.description, "Review a file");
    assert_eq!(t.argument_hint.as_deref(), Some("<path>"));
    assert_eq!(t.content.trim(), "Review $1");

    let b = dir.path().join("plain.md");
    std::fs::write(&b, "\n# Summarize the diff\nmore\n").unwrap();
    assert_eq!(
        load_template(&b, "user").unwrap().description,
        "Summarize the diff"
    );
}

#[test]
fn first_directory_wins_on_name_clash() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("p");
    let user = root.path().join("u");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(project.join("x.md"), "project x").unwrap();
    std::fs::write(user.join("x.md"), "user x").unwrap();
    std::fs::write(user.join("y.md"), "user y").unwrap();
    std::fs::write(user.join("notes.txt"), "ignored").unwrap();
    let found = discover_in(&[(project, "project"), (user, "user")]);
    let names: Vec<_> = found.iter().map(|t| (t.name.as_str(), t.source)).collect();
    assert_eq!(names, vec![("x", "project"), ("y", "user")]);
    assert_eq!(found[0].content, "project x");
}

#[test]
fn search_dirs_walk_to_the_git_root_then_user_dirs() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sub = repo.join("a").join("b");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&sub).unwrap();
    let gh = root.path().join("gh");
    let home = root.path().join("home");
    let dirs = search_dirs(&sub, Some(&gh), Some(&home));
    let paths: Vec<_> = dirs.iter().map(|(d, _)| d.clone()).collect();
    assert_eq!(paths[0], sub.join(".gray").join("prompts"));
    assert!(paths.contains(&repo.join(".claude").join("commands")));
    assert!(!paths.contains(&root.path().join(".gray").join("prompts")));
    let user_start = dirs.iter().position(|(_, s)| *s == "user").unwrap();
    assert_eq!(paths[user_start], gh.join("prompts"));
    assert!(paths.contains(&home.join(".pi").join("agent").join("prompts")));
}

#[test]
fn no_repo_means_only_the_cwd_is_project() {
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("a");
    std::fs::create_dir_all(&sub).unwrap();
    let dirs = search_dirs(&sub, None, None);
    assert_eq!(dirs.len(), 3);
    assert!(
        dirs.iter()
            .all(|(d, s)| d.starts_with(&sub) && *s == "project")
    );
}
