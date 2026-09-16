//! Shared tab-bar scaffolding for manager modals (`/plugins`).
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

#[path = "tabs_tests.rs"]
#[cfg(test)]
mod tests;
