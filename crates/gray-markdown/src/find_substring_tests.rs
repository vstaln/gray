use super::find_substring;
use pulldown_cmark::CowStr;

#[test]
#[ignore = "UNRUN: cargo test banned in X (amdgpu page-flip); run in TTY/CI"]
fn borrowed_subslice_resolves_to_its_range() {
    let hay = String::from("hello [world](url)");
    let sub: &str = &hay[6..13];
    assert_eq!(
        find_substring(&hay, &CowStr::Borrowed(sub), false, false),
        Some(6..13)
    );
}

#[test]
#[ignore = "UNRUN: cargo test banned in X (amdgpu page-flip); run in TTY/CI"]
fn borrowed_str_from_other_allocation_never_matches() {
    // Same bytes, different allocation: raw-pointer subtraction across
    // allocations is UB and could yield a bogus range; address arithmetic
    // plus the byte guard must return `None`.
    let hay = String::from("hello [world](url)");
    let other = String::from("[world](url)");
    assert_eq!(
        find_substring(&hay, &CowStr::Borrowed(other.as_str()), false, false),
        None
    );
}
