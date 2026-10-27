macro_rules! wrapped_tab {
    ($name:ident, $count:literal, $($variant:ident => $idx:literal),+ $(,)?) => {
        impl $name {

            pub const COUNT: usize = $count;

            pub fn index(self) -> usize {
                match self {
                    $(Self::$variant => $idx,)+
                }
            }

            pub fn from_index(i: usize) -> Self {
                match i % Self::COUNT {
                    $($idx => Self::$variant,)+
                    _ => unreachable!(),
                }
            }

            pub fn next(self) -> Self {
                Self::from_index($crate::setup::tabs::next_tab(self.index(), Self::COUNT))
            }

            pub fn prev(self) -> Self {
                Self::from_index($crate::setup::tabs::prev_tab(self.index(), Self::COUNT))
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Installed,
    Errors,
}

wrapped_tab!(Tab, 2, Installed => 0, Errors => 1);

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

pub fn tab_segments(tabs: &[(&str, Option<usize>)], active: usize) -> Vec<TabSegment> {
    tabs.iter()
        .enumerate()
        .map(|(i, (name, count))| TabSegment {
            text: label(name, *count),
            active: i == active,
        })
        .collect()
}

pub fn next_tab(cur: usize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    (cur + 1) % n
}

pub fn prev_tab(cur: usize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    (cur + n - 1) % n
}
