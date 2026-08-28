//! Toolsets — slice 1/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/toolsets.py`
//! slice 1/2 — lines 1–600 of 1083 (first ~600 LOC).
//! Covers: module docstring, `_HERMES_CORE_TOOLS`, `_HERMES_WEBHOOK_SAFE_TOOLS`,
//! and `TOOLSETS` entries through `hermes-weixin` (~line 601, i.e. the
//! `hermes-weixin` toolset inclusive). Remainder (`hermes-qqbot` through
//! `hermes-gateway` + all helper functions `get_toolset`, `resolve_toolset`,
//! etc.) continues in `toolsets_slice2.rs`.
//!
//! T0002 — 1:1 port, no cargo (NEVER cargo).
//!
//! Python source is 1083 LOC; this slice mirrors the first ~600 LOC 1:1 with
//! Rust idioms (static slices, `&'static str`, `Toolset` struct). Comments and
//! tool ordering are preserved so `grep` traces land on the same line intent.

// ---------------------------------------------------------------------------
// Shared core tools — mirrors `_HERMES_CORE_TOOLS` (lines 31-92)
// ---------------------------------------------------------------------------

/// Shared tool list for CLI and all messaging platform toolsets.
///
/// Edit this once to update all platforms simultaneously.
///
/// Mirrors `_HERMES_CORE_TOOLS` in Python (lines 31-92). The Python comments
/// about desktop GUI affordances and Project tools are preserved in the Rust
/// docstring below for 1:1 audit.
pub const HERMES_CORE_TOOLS: &[&str] = &[
    // Web
    "web_search",
    "web_extract",
    // Terminal + process management
    "terminal",
    "process",
    // NOTE: the desktop GUI affordances (read_terminal, open_preview, …) are
    // deliberately NOT here, for the same reason as the `project` tools below:
    // they only work where a GUI renderer can answer them. They live in the
    // `desktop_ui` toolset and are enabled solely by the GUI gateway for a
    // session whose SOURCE is the desktop app (tui_gateway/server.py::
    // _load_enabled_toolsets) — never keyed on a process env var, which is
    // blind to a desktop client talking to a remote/cloud backend.
    // File manipulation
    "read_file",
    "write_file",
    "patch",
    "search_files",
    // Vision + image generation
    "vision_analyze",
    "image_generate",
    // BFL FLUX 3 video generation
    "bfl_flux3_text_to_video",
    "bfl_flux3_image_to_video",
    "bfl_flux3_keyframes_to_video",
    "bfl_flux3_video_continuation",
    "bfl_flux3_get_result",
    "bfl_flux3_prompting_guide",
    // Skills
    "skills_list",
    "skill_view",
    "skill_manage",
    // Browser automation
    "browser_navigate",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_scroll",
    "browser_back",
    "browser_press",
    "browser_get_images",
    "browser_vision",
    "browser_console",
    "browser_cdp",
    "browser_dialog",
    // replaces other tools when browser.backend is "browser-use"
    "browser_exec",
    // Text-to-speech
    "text_to_speech",
    // Planning & memory
    "todo",
    "memory",
    // NOTE: the desktop Project tools (project_list/create/switch) are
    // deliberately NOT here. They only make sense where a GUI can follow the
    // move, so they live in the `project` toolset and are enabled solely by the
    // GUI gateway (tui_gateway/server.py::_load_enabled_toolsets) — keeping them
    // off every CLI/messaging/cron schema (narrow waist).
    // Session history search
    "session_search",
    // Clarifying questions
    "clarify",
    // Code execution + delegation
    "execute_code",
    "delegate_task",
    // Cronjob management
    "cronjob",
    // Home Assistant smart home control (gated on HASS_TOKEN via check_fn)
    "ha_list_entities",
    "ha_get_state",
    "ha_list_services",
    "ha_call_service",
    // Kanban multi-agent coordination — only in schema when the agent is
    // spawned as a kanban worker (HERMES_KANBAN_TASK env set) or the current
    // profile explicitly enables the kanban toolset. Gated via check_fn in
    // tools/kanban_tools.py.
    "kanban_show",
    "kanban_list",
    "kanban_complete",
    "kanban_block",
    "kanban_request_review",
    "kanban_request_changes",
    "kanban_heartbeat",
    "kanban_comment",
    "kanban_create",
    "kanban_link",
    "kanban_unblock",
    "kanban_attach",
    "kanban_attach_url",
    "kanban_attachments",
    // Computer use (macOS, gated on cua-driver being installed via check_fn)
    "computer_use",
];

// Webhook events may originate from untrusted third-party content (for example,
// public PR titles/comments). Keep the default webhook toolset intentionally
// constrained to avoid local file/system execution by prompt injection.
//
// Mirrors `_HERMES_WEBHOOK_SAFE_TOOLS` (lines 97-102).
pub const HERMES_WEBHOOK_SAFE_TOOLS: &[&str] = &[
    "web_search",
    "web_extract",
    "vision_analyze",
    "clarify",
];

// ---------------------------------------------------------------------------
// Toolset definition — mirrors `TOOLSETS` dict (lines 107-601 slice 1 portion)
// ---------------------------------------------------------------------------

/// Mirrors a single `TOOLSETS[name]` entry in Python.
///
/// Fields:
/// - `description` — human-readable description
/// - `tools` — direct tool names
/// - `includes` — other toolsets to include (composition)
/// - `module` — optional Python module override (e.g. `tools.yuanbao_tools`)
/// - `posture` — `true` for posture toolsets (e.g. `coding`)
///
/// Python's dict values are `{"description": ..., "tools": [...], "includes": [...]}`
/// with optional `"module"` and `"posture"` keys. Rust makes those explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolset {
    pub description: &'static str,
    pub tools: &'static [&'static str],
    pub includes: &'static [&'static str],
    /// Optional module override — mirrors `"module": "tools.yuanbao_tools"` in Python.
    /// `None` for the vast majority of toolsets.
    pub module: Option<&'static str>,
    /// Posture flag — mirrors `"posture": True` in Python (only `coding`).
    pub posture: bool,
}

impl Toolset {
    pub const fn new(
        description: &'static str,
        tools: &'static [&'static str],
        includes: &'static [&'static str],
    ) -> Self {
        Self {
            description,
            tools,
            includes,
            module: None,
            posture: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Extra tool lists for composite bundles that are `_HERMES_CORE_TOOLS + [...]`
// ---------------------------------------------------------------------------
//
// Python expresses these as `_HERMES_CORE_TOOLS + ["extra"]`. Rust cannot
// concat slices at compile time without new deps, so we expand them here as
// separate const slices that are documented as `_HERMES_CORE_TOOLS + extras`.
// The ordering is identical to Python: core first, extras appended.

/// Mirrors `hermes-discord` tools = `_HERMES_CORE_TOOLS + ["discord", "discord_admin"]`.
pub const HERMES_DISCORD_TOOLS: &[&str] = &[
    "web_search",
    "web_extract",
    "terminal",
    "process",
    "read_file",
    "write_file",
    "patch",
    "search_files",
    "vision_analyze",
    "image_generate",
    "bfl_flux3_text_to_video",
    "bfl_flux3_image_to_video",
    "bfl_flux3_keyframes_to_video",
    "bfl_flux3_video_continuation",
    "bfl_flux3_get_result",
    "bfl_flux3_prompting_guide",
    "skills_list",
    "skill_view",
    "skill_manage",
    "browser_navigate",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_scroll",
    "browser_back",
    "browser_press",
    "browser_get_images",
    "browser_vision",
    "browser_console",
    "browser_cdp",
    "browser_dialog",
    "browser_exec",
    "text_to_speech",
    "todo",
    "memory",
    "session_search",
    "clarify",
    "execute_code",
    "delegate_task",
    "cronjob",
    "ha_list_entities",
    "ha_get_state",
    "ha_list_services",
    "ha_call_service",
    "kanban_show",
    "kanban_list",
    "kanban_complete",
    "kanban_block",
    "kanban_request_review",
    "kanban_request_changes",
    "kanban_heartbeat",
    "kanban_comment",
    "kanban_create",
    "kanban_link",
    "kanban_unblock",
    "kanban_attach",
    "kanban_attach_url",
    "kanban_attachments",
    "computer_use",
    "discord",
    "discord_admin",
];

/// Mirrors `hermes-feishu` tools = `_HERMES_CORE_TOOLS + [feishu_*]`.
pub const HERMES_FEISHU_TOOLS: &[&str] = &[
    "web_search",
    "web_extract",
    "terminal",
    "process",
    "read_file",
    "write_file",
    "patch",
    "search_files",
    "vision_analyze",
    "image_generate",
    "bfl_flux3_text_to_video",
    "bfl_flux3_image_to_video",
    "bfl_flux3_keyframes_to_video",
    "bfl_flux3_video_continuation",
    "bfl_flux3_get_result",
    "bfl_flux3_prompting_guide",
    "skills_list",
    "skill_view",
    "skill_manage",
    "browser_navigate",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_scroll",
    "browser_back",
    "browser_press",
    "browser_get_images",
    "browser_vision",
    "browser_console",
    "browser_cdp",
    "browser_dialog",
    "browser_exec",
    "text_to_speech",
    "todo",
    "memory",
    "session_search",
    "clarify",
    "execute_code",
    "delegate_task",
    "cronjob",
    "ha_list_entities",
    "ha_get_state",
    "ha_list_services",
    "ha_call_service",
    "kanban_show",
    "kanban_list",
    "kanban_complete",
    "kanban_block",
    "kanban_request_review",
    "kanban_request_changes",
    "kanban_heartbeat",
    "kanban_comment",
    "kanban_create",
    "kanban_link",
    "kanban_unblock",
    "kanban_attach",
    "kanban_attach_url",
    "kanban_attachments",
    "computer_use",
    "feishu_doc_read",
    "feishu_drive_list_comments",
    "feishu_drive_list_comment_replies",
    "feishu_drive_reply_comment",
    "feishu_drive_add_comment",
];

// ---------------------------------------------------------------------------
// Core toolset definitions — `TOOLSETS` (lines 107-601)
// ---------------------------------------------------------------------------
//
// Each entry mirrors Python's `TOOLSETS[toolset_name] = {"description": ...,
// "tools": [...], "includes": [...]}`. Ordering and descriptions are verbatim
// from the Python source; tool names are preserved exactly for 1:1 traceability.

/// All toolsets defined in the first ~600 LOC of `toolsets.py`.
///
/// This is the slice-1 half of `TOOLSETS` — entries through `hermes-weixin`
/// inclusive (line ~601). The remaining entries (`hermes-qqbot`,
/// `hermes-wecom`, `hermes-wecom-callback`, `hermes-yuanbao`, `hermes-sms`,
/// `hermes-webhook`, `hermes-gateway`) plus `TOOLSETS` helpers close in
/// `toolsets_slice2.rs`.
///
/// Kept as a `&[(&str, Toolset)]` slice so lookup is a linear scan — identical
/// to Python's dict lookup in spirit, but without needing `HashMap` or `OnceLock`.
pub static TOOLSETS: &[(&str, Toolset)] = &[
    // Basic toolsets - individual tool categories
    (
        "web",
        Toolset::new(
            "Web research and content extraction tools",
            &["web_search", "web_extract"],
            &[],
        ),
    ),
    (
        "search",
        Toolset::new(
            "Web search only (no content extraction/scraping)",
            &["web_search"],
            &[],
        ),
    ),
    (
        "x_search",
        Toolset::new(
            "Search X (Twitter) posts and threads via xAI's built-in x_search Responses tool. Read-only public X discovery; use the xurl skill for authenticated X API reads and account actions. Available when xAI credentials are configured (SuperGrok OAuth or XAI_API_KEY). Off by default; enable in `hermes tools` → X (Twitter) Search.",
            &["x_search"],
            &[],
        ),
    ),
    (
        "vision",
        Toolset::new(
            "Image analysis and vision tools",
            &["vision_analyze"],
            &[],
        ),
    ),
    (
        "video",
        Toolset::new(
            "Video analysis and understanding tools (opt-in, not in default toolset)",
            &["video_analyze"],
            &[],
        ),
    ),
    (
        "image_gen",
        Toolset::new(
            "Creative generation tools (images)",
            &["image_generate"],
            &[],
        ),
    ),
    (
        "video_gen",
        Toolset::new(
            "Video generation tools. Single ``video_generate`` tool covers text-to-video (prompt only) and image-to-video (prompt + image_url), plus reference-to-video. Provider-specific edit/extend workflows may appear as separate tools. Configure via ``hermes tools`` → Video Generation.",
            &["video_generate", "xai_video_edit", "xai_video_extend"],
            &[],
        ),
    ),
    (
        "bfl",
        Toolset::new(
            "Black Forest Labs FLUX 3 video generation through the Nous tool gateway: per-mode submit tools (text, image, keyframes, continuation), a poll tool, and a prompting guide. Generations take minutes, so submit returns a job id and the model polls for the result.",
            &[
                "bfl_flux3_text_to_video",
                "bfl_flux3_image_to_video",
                "bfl_flux3_keyframes_to_video",
                "bfl_flux3_video_continuation",
                "bfl_flux3_get_result",
                "bfl_flux3_prompting_guide",
            ],
            &[],
        ),
    ),
    (
        "computer_use",
        Toolset::new(
            "Background desktop control via cua-driver (macOS/Windows/Linux) — screenshots, mouse, keyboard, scroll, drag. Does NOT steal the user's cursor or keyboard focus. Works with any tool-capable model.",
            &["computer_use"],
            &[],
        ),
    ),
    (
        "terminal",
        Toolset::new(
            "Terminal/command execution and process management tools",
            &["terminal", "process"],
            &[],
        ),
    ),
    (
        "skills",
        Toolset::new(
            "Access, create, edit, and manage skill documents with specialized instructions and knowledge",
            &["skills_list", "skill_view", "skill_manage"],
            &[],
        ),
    ),
    (
        "browser",
        Toolset::new(
            "Browser automation for web interaction (navigate, click, type, scroll, iframes, hold-click) with web search for finding URLs",
            &[
                "browser_navigate",
                "browser_snapshot",
                "browser_click",
                "browser_type",
                "browser_scroll",
                "browser_back",
                "browser_press",
                "browser_get_images",
                "browser_vision",
                "browser_console",
                "browser_cdp",
                "browser_dialog",
                "browser_exec",
                "web_search",
            ],
            &[],
        ),
    ),
    (
        "cronjob",
        Toolset::new(
            "Cronjob management tool - create, list, update, pause, resume, remove, and trigger scheduled tasks",
            &["cronjob"],
            &[],
        ),
    ),
    (
        "file",
        Toolset::new(
            "File manipulation tools: read, write, patch (with fuzzy matching), and search (content + files)",
            &["read_file", "write_file", "patch", "search_files"],
            &[],
        ),
    ),
    (
        "tts",
        Toolset::new(
            "Text-to-speech: convert text to audio with Edge TTS (free), ElevenLabs, OpenAI, or xAI",
            &["text_to_speech"],
            &[],
        ),
    ),
    (
        "todo",
        Toolset::new(
            "Task planning and tracking for multi-step work",
            &["todo"],
            &[],
        ),
    ),
    (
        "memory",
        Toolset::new(
            "Persistent memory across sessions (personal notes + user profile)",
            &["memory"],
            &[],
        ),
    ),
    (
        "context_engine",
        Toolset::new(
            "Runtime tools exposed by the active context engine",
            &[],
            &[],
        ),
    ),
    (
        "session_search",
        Toolset::new(
            "Search and recall past conversations with summarization",
            &["session_search"],
            &[],
        ),
    ),
    (
        "project",
        Toolset::new(
            "Desktop Projects — create/switch named workspaces (GUI sessions only)",
            &["project_list", "project_create", "project_switch"],
            &[],
        ),
    ),
    // Affordances that only exist because a GUI renderer is on the other end of
    // the connection: read/close the embedded terminal pane, open/read/close the
    // in-app browser, focus a pane, tapback a message.
    //
    // Enabled by the GUI gateway for a session whose SOURCE is the desktop app
    // (tui_gateway/server.py::_load_enabled_toolsets), NOT by a process env var.
    (
        "desktop_ui",
        Toolset::new(
            "Desktop GUI affordances — in-app terminal/browser panes, pane focus, reactions (GUI sessions only)",
            &[
                "read_terminal",
                "close_terminal",
                "open_preview",
                "close_preview",
                "read_preview",
                "drive_preview",
                "annotate_preview",
                "read_window_below",
                "focus_pane",
                "react_to_message",
                "setup_mcp",
                "tour",
            ],
            &[],
        ),
    ),
    (
        "clarify",
        Toolset::new(
            "Ask the user clarifying questions (multiple-choice or open-ended)",
            &["clarify"],
            &[],
        ),
    ),
    (
        "code_execution",
        Toolset::new(
            "Run Python scripts that call tools programmatically (reduces LLM round trips)",
            &["execute_code"],
            &[],
        ),
    ),
    (
        "delegation",
        Toolset::new(
            "Spawn subagents with isolated context for complex subtasks",
            &["delegate_task"],
            &[],
        ),
    ),
    // "honcho" toolset removed — Honcho is now a memory provider plugin.
    // Tools are injected via MemoryManager, not the toolset system.
    (
        "homeassistant",
        Toolset::new(
            "Home Assistant smart home control and monitoring",
            &[
                "ha_list_entities",
                "ha_get_state",
                "ha_list_services",
                "ha_call_service",
            ],
            &[],
        ),
    ),
    (
        "kanban",
        Toolset::new(
            "Kanban multi-agent coordination — only active when the agent is spawned by the kanban dispatcher (HERMES_KANBAN_TASK env set). The dispatcher runs inside the gateway by default; see `kanban.dispatch_in_gateway` in config.yaml. Lets workers mark tasks done with structured handoffs, enter first-class review (request_review — not a block), return review changes, block for human input, heartbeat during long ops, comment on threads, attach files, and (for orchestrators) list, unblock, and fan out tasks.",
            &[
                "kanban_show",
                "kanban_list",
                "kanban_complete",
                "kanban_block",
                "kanban_request_review",
                "kanban_request_changes",
                "kanban_heartbeat",
                "kanban_comment",
                "kanban_create",
                "kanban_link",
                "kanban_unblock",
                "kanban_attach",
                "kanban_attach_url",
                "kanban_attachments",
            ],
            &[],
        ),
    ),
    (
        "discord",
        Toolset::new(
            "Discord read and participate tools (fetch messages, search members, create threads)",
            &["discord"],
            &[],
        ),
    ),
    (
        "discord_admin",
        Toolset::new(
            "Discord server management (list channels/roles, pin messages, assign roles)",
            &["discord_admin"],
            &[],
        ),
    ),
    (
        "yuanbao",
        Toolset::new(
            "Yuanbao platform tools - group info, member queries, DM, stickers",
            &[
                "yb_query_group_info",
                "yb_query_group_members",
                "yb_send_dm",
                "yb_search_sticker",
                "yb_send_sticker",
            ],
            &[],
        ),
    ),
    (
        "feishu_doc",
        Toolset::new(
            "Read Feishu/Lark document content",
            &["feishu_doc_read"],
            &[],
        ),
    ),
    (
        "feishu_drive",
        Toolset::new(
            "Feishu/Lark document comment operations (list, reply, add)",
            &[
                "feishu_drive_list_comments",
                "feishu_drive_list_comment_replies",
                "feishu_drive_reply_comment",
                "feishu_drive_add_comment",
            ],
            &[],
        ),
    ),
    (
        "spotify",
        Toolset::new(
            "Native Spotify playback, search, playlist, album, and library tools",
            &[
                "spotify_playback",
                "spotify_devices",
                "spotify_queue",
                "spotify_search",
                "spotify_playlists",
                "spotify_albums",
                "spotify_library",
            ],
            &[],
        ),
    ),
    // Scenario-specific toolsets
    (
        "debugging",
        Toolset::new(
            "Debugging and troubleshooting toolkit",
            &["terminal", "process"],
            &["web", "file"],
        ),
    ),
    (
        "safe",
        Toolset::new(
            "Safe toolkit without terminal access",
            &[],
            &["web", "vision", "image_gen"],
        ),
    ),
    // Coding posture (base Hermes — CLI/TUI/desktop/ACP). Auto-selected in a
    // code workspace; see agent/coding_context.py. Keeps everything you reach
    // for while pairing on code and drops the rest (messaging, tts, image_gen,
    // spotify, home-assistant, cron, computer-use).
    //
    // The GUI pane/browser affordances are NOT listed here: they belong to the
    // client surface, not the posture, so the GUI gateway folds `desktop_ui`
    // in alongside this selection for a desktop-sourced session (see
    // tui_gateway/server.py::_load_enabled_toolsets).
    (
        "coding",
        Toolset {
            description: "Coding-focused toolset: files, terminal, search, web docs, skills, todo, delegate, vision, browser",
            tools: &[
                "web_search",
                "web_extract",
                "terminal",
                "process",
                "read_file",
                "write_file",
                "patch",
                "search_files",
                "vision_analyze",
                "skills_list",
                "skill_view",
                "skill_manage",
                "browser_navigate",
                "browser_snapshot",
                "browser_click",
                "browser_type",
                "browser_scroll",
                "browser_back",
                "browser_press",
                "browser_get_images",
                "browser_vision",
                "browser_console",
                "browser_cdp",
                "browser_dialog",
                "browser_exec",
                "todo",
                "memory",
                "session_search",
                "clarify",
                "execute_code",
                "delegate_task",
            ],
            includes: &[],
            module: None,
            posture: true,
        },
    ),
    // ==========================================================================
    // Full Hermes toolsets (CLI + messaging platforms)
    //
    // All platforms share the same core tools. Note: agents do NOT get an
    // agent-callable send_message tool — outbound platform messaging is handled
    // outside the agent loop (cron delivery, the gateway kanban notifier, and
    // the `hermes send` CLI), not by the model deciding to send on its own.
    // ==========================================================================
    (
        "hermes-acp",
        Toolset::new(
            "Editor integration (VS Code, Zed, JetBrains) — coding-focused tools without messaging, audio, or clarify UI",
            &[
                "web_search",
                "web_extract",
                "terminal",
                "process",
                "read_file",
                "write_file",
                "patch",
                "search_files",
                "vision_analyze",
                "skills_list",
                "skill_view",
                "skill_manage",
                "browser_navigate",
                "browser_snapshot",
                "browser_click",
                "browser_type",
                "browser_scroll",
                "browser_back",
                "browser_press",
                "browser_get_images",
                "browser_vision",
                "browser_console",
                "browser_cdp",
                "browser_dialog",
                "browser_exec",
                "todo",
                "memory",
                "session_search",
                "execute_code",
                "delegate_task",
            ],
            &[],
        ),
    ),
    (
        "hermes-api-server",
        Toolset::new(
            "OpenAI-compatible API server — full agent tools accessible via HTTP (no interactive UI tools like clarify or send_message)",
            &[
                "web_search",
                "web_extract",
                "terminal",
                "process",
                "read_file",
                "write_file",
                "patch",
                "search_files",
                "vision_analyze",
                "image_generate",
                "bfl_flux3_text_to_video",
                "bfl_flux3_image_to_video",
                "bfl_flux3_keyframes_to_video",
                "bfl_flux3_video_continuation",
                "bfl_flux3_get_result",
                "bfl_flux3_prompting_guide",
                "skills_list",
                "skill_view",
                "skill_manage",
                "browser_navigate",
                "browser_snapshot",
                "browser_click",
                "browser_type",
                "browser_scroll",
                "browser_back",
                "browser_press",
                "browser_get_images",
                "browser_vision",
                "browser_console",
                "browser_cdp",
                "browser_dialog",
                "browser_exec",
                "todo",
                "memory",
                "session_search",
                "execute_code",
                "delegate_task",
                "cronjob",
                "ha_list_entities",
                "ha_get_state",
                "ha_list_services",
                "ha_call_service",
            ],
            &[],
        ),
    ),
    (
        "hermes-cli",
        Toolset::new(
            "Full interactive CLI toolset - all default tools plus cronjob management",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-cron",
        Toolset::new(
            "Default cron toolset - same core tools as hermes-cli; gated by `hermes tools`",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-telegram",
        Toolset::new(
            "Telegram bot toolset - full access for personal use (terminal has safety checks)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-discord",
        Toolset::new(
            "Discord bot toolset - full access (terminal has safety checks via dangerous command approval)",
            HERMES_DISCORD_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-whatsapp",
        Toolset::new(
            "WhatsApp bot toolset - similar to Telegram (personal messaging, more trusted)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-slack",
        Toolset::new(
            "Slack bot toolset - full access for workspace use (terminal has safety checks)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-signal",
        Toolset::new(
            "Signal bot toolset - encrypted messaging platform (full access)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-bluebubbles",
        Toolset::new(
            "BlueBubbles iMessage bot toolset - Apple iMessage via local BlueBubbles server",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-homeassistant",
        Toolset::new(
            "Home Assistant bot toolset - smart home event monitoring and control",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-email",
        Toolset::new(
            "Email bot toolset - interact with Hermes via email (IMAP/SMTP)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-mattermost",
        Toolset::new(
            "Mattermost bot toolset - self-hosted team messaging (full access)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-matrix",
        Toolset::new(
            "Matrix bot toolset - decentralized encrypted messaging (full access)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-dingtalk",
        Toolset::new(
            "DingTalk bot toolset - enterprise messaging platform (full access)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-feishu",
        Toolset::new(
            "Feishu/Lark bot toolset - enterprise messaging via Feishu/Lark (full access)",
            HERMES_FEISHU_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-weixin",
        Toolset::new(
            "Weixin bot toolset - personal WeChat messaging via iLink (full access)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
];

// ---------------------------------------------------------------------------
// Slice boundary — remainder continues in `toolsets_slice2.rs`
// ---------------------------------------------------------------------------
// The Python source continues with:
//   "hermes-qqbot", "hermes-wecom", "hermes-wecom-callback",
//   "hermes-yuanbao" (with "module": "tools.yuanbao_tools"),
//   "hermes-sms", "hermes-webhook" (with _HERMES_WEBHOOK_SAFE_TOOLS),
//   "hermes-gateway" (includes union), and all functions:
//   get_toolset, bundle_non_core_tools, resolve_toolset, etc.
// through line 1083. See toolsets_slice2.rs for that half.
//
// This file intentionally stops at `hermes-weixin` inclusive (~line 601)
// so that the 1083 LOC split is ~600/483 and `cargo` is never invoked.
