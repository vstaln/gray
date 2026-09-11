//! Shared tab-bar scaffolding for manager modals (`/plugins`, Task 6 store).
//!
//! Pure, unit-testable, no crossterm/ratatui: helpers return plain data and
//! the caller applies styling. The free functions are tab-agnostic (they
//! operate on `&[(&str, Option<usize>)]`) so Task 6 reuses them as-is; the
//! [`Tab`] enum covers this modal's tabs and grows then.

/// Generates the wrapped-index tab machinery for an enum whose variants map
/// to `0..COUNT` in declaration order: `COUNT`, `index`, `from_index`, `next`,
/// `prev`. Keeps every tab enum in this crate on one implementation.
macro_rules! wrapped_tab {
    ($name:ident, $count:literal, $($variant:ident => $idx:literal),+ $(,)?) => {
        impl $name {
            /// Number of tabs in this modal.
            pub const COUNT: usize = $count;

            /// Position of this tab in the tab bar.
            pub fn index(self) -> usize {
                match self {
                    $(Self::$variant => $idx,)+
                }
            }

            /// Tab at position `i` (wraps).
            pub fn from_index(i: usize) -> Self {
                match i % Self::COUNT {
                    $($idx => Self::$variant,)+
                    _ => unreachable!(),
                }
            }

            /// Next tab, wrapping.
            pub fn next(self) -> Self {
                Self::from_index($crate::setup::tabs::next_tab(self.index(), Self::COUNT))
            }

            /// Previous tab, wrapping.
            pub fn prev(self) -> Self {
                Self::from_index($crate::setup::tabs::prev_tab(self.index(), Self::COUNT))
            }
        }
    };
}
pub(crate) use wrapped_tab;

/// Tabs of the plugins manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Installed,
    Errors,
}

wrapped_tab!(Tab, 2, Installed => 0, Errors => 1);

/// One tab label; `active` tells the caller how to style it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabSegment {
    pub text: String,
    pub active: bool,
}

fn label(name: &str, count: Option<usize>) -> String {
    match count {
        Some(n) => format!("{name} ({n})"),
        None => name.to_string(),
    }
}

/// One segment per tab; exactly the `active`-th segment is marked active.
/// The count badge renders `(n)` only when `Some(n)` — pass `None` to hide
/// it (e.g. zero errors).
pub fn tab_segments(tabs: &[(&str, Option<usize>)], active: usize) -> Vec<TabSegment> {
    tabs.iter()
        .enumerate()
        .map(|(i, (name, count))| TabSegment {
            text: label(name, *count),
            active: i == active,
        })
        .collect()
}

/// Next tab index, wrapping around `n`.
pub fn next_tab(cur: usize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    (cur + 1) % n
}

/// Previous tab index, wrapping around `n`.
pub fn prev_tab(cur: usize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    (cur + n - 1) % n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_mark_exactly_one_active() {
        let tabs = [("Installed", None), ("Errors", Some(2))];
        for active in 0..tabs.len() {
            let segs = tab_segments(&tabs, active);
            assert_eq!(segs.len(), tabs.len());
            for (i, seg) in segs.iter().enumerate() {
                assert_eq!(seg.active, i == active, "active={active} i={i}");
            }
        }
    }

    #[test]
    fn next_prev_wrap_at_both_ends() {
        assert_eq!(next_tab(0, 2), 1);
        assert_eq!(next_tab(1, 2), 0);
        assert_eq!(prev_tab(0, 2), 1);
        assert_eq!(prev_tab(1, 2), 0);
    }

    #[test]
    fn count_badge_renders_only_when_some() {
        let segs = tab_segments(&[("Installed", None), ("Errors", Some(2))], 0);
        assert_eq!(segs[0].text, "Installed");
        assert_eq!(segs[1].text, "Errors (2)");
        let segs = tab_segments(&[("Installed", None), ("Errors", None)], 1);
        assert_eq!(segs[0].text, "Installed");
        assert_eq!(segs[1].text, "Errors");
    }

    #[test]
    fn tab_enum_round_trips_through_index() {
        assert_eq!(Tab::from_index(0), Tab::Installed);
        assert_eq!(Tab::from_index(1), Tab::Errors);
        assert_eq!(Tab::Installed.next(), Tab::Errors);
        assert_eq!(Tab::Errors.next(), Tab::Installed);
        assert_eq!(Tab::Installed.prev(), Tab::Errors);
        assert_eq!(Tab::Errors.prev(), Tab::Installed);
    }
}
