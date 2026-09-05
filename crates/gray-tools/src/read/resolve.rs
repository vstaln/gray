//! T4.1 — Unicode filename retry (7 spellings) before failing.
//!
//! Pure functions + `std::fs` existence checks only (std only, no new deps).
//! UNWIRED: `read/mod.rs` has no `mod resolve;` yet (Wave-D sibling owns the
//! `mod.rs` region — same pattern as `tail.rs`/`args.rs` staging). Intended
//! caller flow in `ReadTool::execute`, right after `resolve_path` and before
//! the T2.3 guard:
//!
//! ```ignore
//! mod resolve;
//! let given = resolve_path(&ctx.cwd, &path);
//! let full = resolve::resolve_existing(&given).unwrap_or(given.clone());
//! let note = (full != given).then(|| {
//!     resolve::repaired_note(&full.display().to_string(), &given.display().to_string())
//! });
//! // ... guard + read `full` as today; prepend `note` to the output.
//! ```
//!
//! Spec: plan.ts T4.1 ("Unicode filename retry (7 spellings) before failing").
//! Only the final path component is ever mutated — a repair never changes
//! directories ([`same_parent`] enforces it).
//!
//! Contract string lives HERE until T1.3's `notices.rs` integrator moves it
//! verbatim (same staging as `tail.rs`/`hygiene.rs` — one owner per string,
//! no duplicate in `notices.rs` first).
//!
//! FOLLOW-UPS (not done here — files outside T4.1 ownership):
//! 1. `read/mod.rs`: add `mod resolve;` + the wiring above (Wave-D owner).
//! 2. `notices.rs` (T1.3 owner): move [`repaired_note`] there verbatim.
//! 3. `write.rs`/`edit.rs`: call the same helper (spec's follow-up).
//! 4. `Cargo.toml`: plan suggests `unicode-normalization`; deliberately NOT
//!    added here (outside ownership). See the `ponytail:` note on [`to_nfc`].
//!
//! // ponytail: minimal Latin accent table (~50 entries) instead of the
//! // `unicode-normalization` crate — covers the spec's café/NFD cases with
//! // zero new deps. Upgrade to the crate if non-Latin scripts need repairs.

use std::path::{Path, PathBuf};

/// `[read: opened <actual> (path repaired from <given>)]` — prepended to the
/// output when a repair hits, so the model learns the real spelling for edit.
pub fn repaired_note(actual: &str, given: &str) -> String {
    format!("[read: opened {actual} (path repaired from {given})]")
}

/// (precomposed, base, combining) for the Latin scripts the retry covers.
/// Combining marks: U+0300 grave, U+0301 acute, U+0302 circumflex,
/// U+0303 tilde, U+0308 diaeresis, U+030A ring, U+0327 cedilla.
const ACCENTS: &[(char, char, char)] = &[
    ('\u{C0}', 'A', '\u{300}'),
    ('\u{C1}', 'A', '\u{301}'),
    ('\u{C2}', 'A', '\u{302}'),
    ('\u{C3}', 'A', '\u{303}'),
    ('\u{C4}', 'A', '\u{308}'),
    ('\u{C5}', 'A', '\u{30A}'),
    ('\u{E0}', 'a', '\u{300}'),
    ('\u{E1}', 'a', '\u{301}'),
    ('\u{E2}', 'a', '\u{302}'),
    ('\u{E3}', 'a', '\u{303}'),
    ('\u{E4}', 'a', '\u{308}'),
    ('\u{E5}', 'a', '\u{30A}'),
    ('\u{C8}', 'E', '\u{300}'),
    ('\u{C9}', 'E', '\u{301}'),
    ('\u{CA}', 'E', '\u{302}'),
    ('\u{CB}', 'E', '\u{308}'),
    ('\u{E8}', 'e', '\u{300}'),
    ('\u{E9}', 'e', '\u{301}'),
    ('\u{EA}', 'e', '\u{302}'),
    ('\u{EB}', 'e', '\u{308}'),
    ('\u{CC}', 'I', '\u{300}'),
    ('\u{CD}', 'I', '\u{301}'),
    ('\u{CE}', 'I', '\u{302}'),
    ('\u{CF}', 'I', '\u{308}'),
    ('\u{EC}', 'i', '\u{300}'),
    ('\u{ED}', 'i', '\u{301}'),
    ('\u{EE}', 'i', '\u{302}'),
    ('\u{EF}', 'i', '\u{308}'),
    ('\u{D2}', 'O', '\u{300}'),
    ('\u{D3}', 'O', '\u{301}'),
    ('\u{D4}', 'O', '\u{302}'),
    ('\u{D5}', 'O', '\u{303}'),
    ('\u{D6}', 'O', '\u{308}'),
    ('\u{F2}', 'o', '\u{300}'),
    ('\u{F3}', 'o', '\u{301}'),
    ('\u{F4}', 'o', '\u{302}'),
    ('\u{F5}', 'o', '\u{303}'),
    ('\u{F6}', 'o', '\u{308}'),
    ('\u{D9}', 'U', '\u{300}'),
    ('\u{DA}', 'U', '\u{301}'),
    ('\u{DB}', 'U', '\u{302}'),
    ('\u{DC}', 'U', '\u{308}'),
    ('\u{F9}', 'u', '\u{300}'),
    ('\u{FA}', 'u', '\u{301}'),
    ('\u{FB}', 'u', '\u{302}'),
    ('\u{FC}', 'u', '\u{308}'),
    ('\u{DD}', 'Y', '\u{301}'),
    ('\u{FD}', 'y', '\u{301}'),
    ('\u{FF}', 'y', '\u{308}'),
    ('\u{D1}', 'N', '\u{303}'),
    ('\u{F1}', 'n', '\u{303}'),
    ('\u{C7}', 'C', '\u{327}'),
    ('\u{E7}', 'c', '\u{327}'),
];

/// Minimal NFD: split covered precomposed chars into base + combining.
/// Anything outside [`ACCENTS`] passes through untouched.
pub fn to_nfd(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match ACCENTS.iter().find(|(p, _, _)| *p == c) {
            Some((_, b, m)) => {
                out.push(*b);
                out.push(*m);
            }
            None => out.push(c),
        }
    }
    out
}

/// Minimal NFC: fold covered base + combining pairs back into precomposed.
/// One left-to-right pass; uncovered sequences pass through untouched.
pub fn to_nfc(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len()
            && let Some((p, _, _)) = ACCENTS
                .iter()
                .find(|(_, b, m)| *b == chars[i] && *m == chars[i + 1])
        {
            out.push(*p);
            i += 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Spelling 4: narrow no-break space (U+202F) and no-break space (U+00A0)
/// become a plain space.
fn nbsp_to_space(s: &str) -> String {
    s.replace('\u{202F}', " ").replace('\u{A0}', " ")
}

/// Spelling 5: every ASCII space may be the macOS screenshot shape (U+202F).
/// Enumerates all 2ⁿ−1 non-trivial combinations for n ≤ 4 spaces; beyond that
/// only the all-NBSP form (combinatorial cap — screenshot names have ≤ 2).
fn space_to_nbsp_variants(name: &str) -> Vec<String> {
    let n = name.chars().filter(|&c| c == ' ').count();
    if n == 0 {
        return Vec::new();
    }
    if n > 4 {
        let all = name.replace(' ', "\u{202F}");
        return if all != name { vec![all] } else { Vec::new() };
    }
    let chars: Vec<char> = name.chars().collect();
    let idx: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(_, &c)| c == ' ')
        .map(|(i, _)| i)
        .collect();
    (1..(1u32 << n))
        .map(|mask| {
            let mut v = chars.clone();
            for (j, &pos) in idx.iter().enumerate() {
                if mask & (1 << j) != 0 {
                    v[pos] = '\u{202F}';
                }
            }
            v.into_iter().collect()
        })
        .collect()
}

/// Spelling 6a: ASCII quotes become curly (`'` → `’`, `"` → `”`).
fn ascii_to_curly(s: &str) -> String {
    s.replace('\'', "\u{2019}").replace('"', "\u{201D}")
}

/// Spelling 6b: curly quotes become ASCII (both single/double directions).
fn curly_to_ascii(s: &str) -> String {
    s.replace(['\u{2019}', '\u{2018}'], "'")
        .replace(['\u{201C}', '\u{201D}'], "\"")
}

fn push_dedup(out: &mut Vec<String>, v: String) {
    if v.is_empty() || v == "." || v == ".." || v.contains('/') || v.contains('\0') {
        return;
    }
    if !out.contains(&v) {
        out.push(v);
    }
}

/// Ordered repair candidates for a file name (final component only), spec
/// order: (1) as given; (2) NFC; (3) NFD; (4) NBSP→space; (5) space→U+202F
/// variants; (6) quote swaps both ways; (7) NFD of each quote swap.
pub fn candidates(name: &str) -> Vec<String> {
    let mut out = vec![name.to_string()];
    push_dedup(&mut out, to_nfc(name));
    push_dedup(&mut out, to_nfd(name));
    push_dedup(&mut out, nbsp_to_space(name));
    for v in space_to_nbsp_variants(name) {
        push_dedup(&mut out, v);
    }
    let to_curly = ascii_to_curly(name);
    let to_ascii = curly_to_ascii(name);
    push_dedup(&mut out, to_curly.clone());
    push_dedup(&mut out, to_ascii.clone());
    push_dedup(&mut out, to_nfd(&to_curly));
    push_dedup(&mut out, to_nfd(&to_ascii));
    out
}

/// Boundary: a repair never changes directories. Lexical parent comparison —
/// sufficient because [`resolve_existing`] builds every candidate as
/// `parent.join(file_name)`; a future caller passing an arbitrary path with a
/// different parent is rejected here.
pub fn same_parent(original: &Path, candidate: &Path) -> bool {
    original.parent() == candidate.parent()
}

/// First candidate that exists (1) as given, then (2)–(7) in order.
/// Non-UTF-8 file names and parent-less edge cases yield `None` when the
/// literal is missing (spec repairs are Unicode spellings of the name).
/// `symlink_metadata` (not `exists`) so symlinks to anything still hit —
/// the T2.3 guard decides refusals later.
pub fn resolve_existing(original: &Path) -> Option<PathBuf> {
    if std::fs::symlink_metadata(original).is_ok() {
        return Some(original.to_path_buf());
    }
    let name = original.file_name()?.to_str()?;
    let parent = original.parent().unwrap_or_else(|| Path::new(""));
    for cand in candidates(name).into_iter().skip(1) {
        let joined = if parent.as_os_str().is_empty() {
            PathBuf::from(&cand)
        } else {
            parent.join(&cand)
        };
        if !same_parent(original, &joined) {
            continue;
        }
        if std::fs::symlink_metadata(&joined).is_ok() {
            return Some(joined);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repaired_note_is_contract_exact() {
        assert_eq!(
            repaired_note("a/café.txt", "a/cafe.txt"),
            "[read: opened a/café.txt (path repaired from a/cafe.txt)]"
        );
    }

    #[test]
    fn nfc_nfd_round_trip_cafe_and_leave_plain_text_alone() {
        let nfc = "caf\u{E9}.txt";
        let nfd = "cafe\u{301}.txt";
        assert_eq!(to_nfd(nfc), nfd);
        assert_eq!(to_nfc(nfd), nfc);
        assert_eq!(to_nfc(nfc), nfc);
        assert_eq!(to_nfd(nfd), nfd);
        assert_eq!(to_nfc("plain.txt"), "plain.txt");
        assert_eq!(to_nfd("plain.txt"), "plain.txt");
    }

    #[test]
    fn candidates_keep_spec_order_and_dedup() {
        let c = candidates("plain.txt");
        assert_eq!(c[0], "plain.txt");
        assert_eq!(c.len(), 1, "no-op spellings dedup to as-given: {c:?}");
        let c = candidates("caf\u{E9} it's.txt");
        assert_eq!(c[0], "caf\u{E9} it's.txt");
        assert!(c.contains(&"cafe\u{301} it's.txt".to_string())); // (3) NFD
        assert!(c.contains(&"caf\u{E9} it\u{2019}s.txt".to_string())); // (6) curly
        assert!(c.contains(&"cafe\u{301} it\u{2019}s.txt".to_string())); // (7) NFD+curly
        assert_eq!(c.len(), c.iter().collect::<std::collections::HashSet<_>>().len());
    }

    /// All 7 spellings, one row each (6 has both quote directions).
    /// Each row lives in its own subdir so NFC/NFD byte shapes never collide.
    #[test]
    fn table_all_seven_spellings_resolve() {
        let rows: &[(&str, &str, &str)] = &[
            ("s1", "hello.txt", "hello.txt"), // (1) as given
            ("s2", "cafe\u{301}.txt", "caf\u{E9}.txt"), // (2) NFC
            ("s3", "caf\u{E9}.txt", "cafe\u{301}.txt"), // (3) NFD
            ("s4", "with\u{202F}space.txt", "with space.txt"), // (4) NBSP→space
            ("s5", "Screenshot 3.04 PM.png", "Screenshot 3.04\u{202F}PM.png"), // (5) space→U+202F
            ("s6", "it's.md", "it\u{2019}s.md"), // (6a) ascii→curly
            ("s6r", "it\u{2019}s-rev.md", "it's-rev.md"), // (6b) curly→ascii
            ("s7", "caf\u{E9}'s.md", "cafe\u{301}\u{2019}s.md"), // (7) NFD+curly
        ];
        let outer = tempfile::tempdir().unwrap();
        for (dir, given, actual) in rows {
            let sub = outer.path().join(dir);
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join(actual), "x").unwrap();
            let got = resolve_existing(&sub.join(given));
            assert_eq!(got, Some(sub.join(actual)), "row {dir}: {given:?}");
        }
    }

    #[test]
    fn nbsp_variant_u00a0_also_maps_to_space() {
        let outer = tempfile::tempdir().unwrap();
        std::fs::write(outer.path().join("with space.txt"), "x").unwrap();
        assert_eq!(
            resolve_existing(&outer.path().join("with\u{A0}space.txt")),
            Some(outer.path().join("with space.txt"))
        );
    }

    #[test]
    fn as_given_wins_and_missing_returns_none() {
        let outer = tempfile::tempdir().unwrap();
        // Both shapes exist: the literal wins (stop at first that exists).
        std::fs::write(outer.path().join("caf\u{E9}.txt"), "nfc").unwrap();
        std::fs::write(outer.path().join("cafe\u{301}.txt"), "nfd").unwrap();
        assert_eq!(
            resolve_existing(&outer.path().join("caf\u{E9}.txt")),
            Some(outer.path().join("caf\u{E9}.txt"))
        );
        assert_eq!(resolve_existing(&outer.path().join("nope.txt")), None);
    }

    #[test]
    fn repair_never_changes_directories() {
        let outer = tempfile::tempdir().unwrap();
        let a = outer.path().join("a");
        let b = outer.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("cafe\u{301}.txt"), "x").unwrap();
        // Same spelling in the sibling dir must NOT leak across.
        assert_eq!(resolve_existing(&a.join("caf\u{E9}.txt")), None);
        assert!(!same_parent(&a.join("x"), &b.join("x")));
        assert!(same_parent(&a.join("x"), &a.join("y")));
    }
}
