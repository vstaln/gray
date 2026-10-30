/// A composer `/`-command: name, description, palette-style group, and
/// whether it needs an argument (then selecting it types `/name ` instead
/// of running it). Mirrors the arms of `slash()`.
#[derive(Clone, Copy)]
pub(super) struct SlashCmd {
    pub(super) name: &'static str,
    pub(super) hint: &'static str,
    pub(super) group: &'static str,
    pub(super) arg: bool,
}

pub(super) const fn sc(
    name: &'static str,
    hint: &'static str,
    group: &'static str,
    arg: bool,
) -> SlashCmd {
    SlashCmd {
        name,
        hint,
        group,
        arg,
    }
}

pub(super) const SLASH_COMMANDS: &[SlashCmd] = &[
    sc("new", "New thread", "Thread", false),
    sc("rename", "Rename thread", "Thread", true),
    sc("pin", "Pin or unpin thread", "Thread", false),
    sc("archive", "Archive thread", "Thread", false),
    sc("settle", "Settle or unsettle thread", "Thread", false),
    sc("snooze", "Snooze thread (minutes)", "Thread", false),
    sc("stop", "Stop turn", "Thread", false),
    sc("approve", "Approve pending action", "Thread", false),
    sc("reject", "Reject pending action", "Thread", false),
    sc("model", "Set model (provider:model)", "Model", true),
    sc("provider", "Set provider", "Model", true),
    sc("mode", "Set access mode", "Model", true),
    sc("effort", "Set effort (low/medium/high)", "Model", true),
    sc("diff", "Toggle diff panel", "View", false),
    sc("terminal", "Toggle terminal", "View", false),
    sc("plugins", "Plugins panel", "View", false),
    sc("sessions", "Gray sessions", "View", false),
    sc("skills", "Gray skills", "View", false),
    sc("layout", "Set layout", "View", true),
    sc("checkpoint", "Create checkpoint", "Git", false),
    sc("restore", "Restore last checkpoint", "Git", false),
    sc("turndiff", "Diff since last turn", "Git", false),
    sc("headdiff", "Diff vs HEAD", "Git", false),
    sc("branch", "Create branch", "Git", true),
    sc("checkout", "Checkout branch", "Git", true),
    sc("worktree", "Create worktree", "Git", true),
    sc("discard", "Discard changes", "Git", false),
    sc("settings", "Open settings", "Settings", false),
    sc("theme", "Toggle theme", "Settings", false),
    sc(
        "autoaccept",
        "Auto-accept permissions on|off",
        "Settings",
        true,
    ),
    sc("background", "Set background", "Settings", true),
    sc("resume", "Resume gray session", "Gray", true),
    sc("gray", "Run a gray command", "Gray", true),
    sc("install", "Install plugin", "Plugins", true),
    sc("uninstall", "Remove plugin", "Plugins", true),
    sc("enable", "Enable plugin", "Plugins", true),
    sc("disable", "Disable plugin", "Plugins", true),
    sc("attach", "Attach image", "Files", true),
    sc("view", "Preview file", "Files", true),
];

/// Bare category hints that a palette group header already says.
pub(super) const PALETTE_CATEGORY_HINTS: &[&str] = &[
    "Thread",
    "Threads",
    "View",
    "Settings",
    "Theme",
    "Git",
    "Gray",
    "Image",
    "Plugin",
    "Effort",
    "Layout",
    "Action",
    "Turn",
    "Provider",
    "Model",
    "Access",
    "Checkout",
    "Switch",
    "Project",
    "Open file",
];
