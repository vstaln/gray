/**
 * Docs content. Kept as data rather than MDX for now so the whole docs surface
 * stays in the static export with zero extra deps; every page here describes
 * behavior that exists in crates/, and the reference sections are transcribed
 * from the Rust source they name.
 */

export type Block =
  | { t: "p"; text: string }
  | { t: "h"; text: string }
  | { t: "code"; lang: string; text: string }
  | { t: "table"; head: [string, string]; rows: Array<[string, string]> }
  | { t: "note"; text: string };

export type Doc = {
  slug: string;
  title: string;
  summary: string;
  section: string;
  source?: string;
  blocks: Block[];
};

export const sections = ["Getting started", "Using gray", "Surfaces", "Reference"] as const;

export const docs: Doc[] = [
  {
    slug: "installation",
    title: "Installation",
    section: "Getting started",
    summary: "One command on macOS, Linux, WSL or Windows — or build it from source.",
    source: "dist/install.sh",
    blocks: [
      {
        t: "p",
        text: "gray ships as a single statically linked binary. The installer detects your platform, downloads the matching tarball from gray.alignment.id/dl, and drops the binary on your PATH. There is no runtime to install and nothing to configure at install time.",
      },
      { t: "h", text: "macOS · Linux · WSL" },
      {
        t: "code",
        lang: "bash",
        text: "curl -fsSL https://gray.alignment.id/install.sh | sh",
      },
      {
        t: "p",
        text: "The beta channel rebuilds on every push to main. Use it if you want the newest fixes and can tolerate the occasional rough edge.",
      },
      {
        t: "code",
        lang: "bash",
        text: "curl -fsSL https://gray.alignment.id/install.sh | sh -s -- beta",
      },
      { t: "h", text: "Windows" },
      {
        t: "code",
        lang: "powershell",
        text: "iwr https://gray.alignment.id/install.ps1 -UseBasicParsing | iex",
      },
      {
        t: "p",
        text: "Windows runs gray through WSL. The PowerShell script checks for a WSL distribution, installs curl inside it if needed, and then runs the Unix installer.",
      },
      { t: "h", text: "From source" },
      { t: "code", lang: "bash", text: "cargo build --release -p gray" },
      {
        t: "note",
        text: "gray checks for a newer release on startup and can update itself in place with the same installer.",
      },
    ],
  },
  {
    slug: "quickstart",
    title: "Quickstart",
    section: "Getting started",
    summary: "From install to a working conversation, with nothing forced at boot.",
    blocks: [
      {
        t: "p",
        text: "Run gray with no arguments. The first run drops you straight at the prompt — it does not force an onboarding wizard, a login, or a model choice on you.",
      },
      { t: "code", lang: "bash", text: "gray" },
      {
        t: "p",
        text: "Configure whenever you feel like it. /connect walks through providers (free tier, API key, OAuth, or a local endpoint) and /model opens a searchable picker over the bundled models.dev catalog.",
      },
      { t: "h", text: "Other entry points" },
      {
        t: "code",
        lang: "bash",
        text: 'echo "hi" | gray -p "one-line summary of this repo"   # print mode, for scripts\ngray -c                                              # resume the last session',
      },
      {
        t: "p",
        text: "Ctrl-C means cancel mid-turn on the first press and exit at the prompt. An interrupted turn still persists whatever reached memory, so resuming never loses the part that already streamed.",
      },
    ],
  },
  {
    slug: "providers",
    title: "Providers & models",
    section: "Using gray",
    summary: "Any OpenAI-compatible endpoint, plus OAuth sign-in for xAI and Codex.",
    source: "crates/gray/src/setup",
    blocks: [
      {
        t: "p",
        text: "gray speaks the OpenAI chat-completions protocol over SSE, so any compatible endpoint works out of the box: OpenRouter, DeepSeek, Groq, OpenAI, ollama, vLLM and LM Studio among them. The default base URL is OpenRouter.",
      },
      {
        t: "p",
        text: "For xAI/Grok and Codex/ChatGPT accounts there are full OAuth flows with PKCE and a loopback redirect — no API key to paste, and tokens refresh automatically ahead of expiry.",
      },
      { t: "h", text: "Keys" },
      {
        t: "p",
        text: "Keys are stored per provider in ~/.gray/auth.json with 0600 permissions. Environment variables win over stored keys, so CI and one-off shells stay easy.",
      },
      {
        t: "code",
        lang: "bash",
        text: "/connect            # provider menu: free tier · API key · OAuth · local\n/model              # searchable picker from the bundled catalog\n/thinking           # reasoning effort",
      },
    ],
  },
  {
    slug: "sessions",
    title: "Sessions",
    section: "Using gray",
    summary: "JSONL on disk with parent-id branching, resumable at any point.",
    source: "crates/gray-session",
    blocks: [
      {
        t: "p",
        text: "Every session is a JSONL file under ~/.gray/sessions. Each line is one message with a parent id, which makes the history a tree rather than a list — branching from an earlier point does not destroy what came after it.",
      },
      {
        t: "code",
        lang: "bash",
        text: "gray -c                    # resume the most recent session\ngray resume                # pick from a list\ngray resume <id> --all     # resume by id, ignoring the cwd filter",
      },
      {
        t: "p",
        text: "Sessions are filtered by working directory by default, so resuming inside a repo shows that repo's conversations first.",
      },
    ],
  },
  {
    slug: "context-window",
    title: "Context window & auto-compact",
    section: "Using gray",
    summary: "How the window is resolved, and what happens when you approach it.",
    source: "crates/gray/src/compact.rs",
    blocks: [
      {
        t: "p",
        text: "The context window resolves in order: the --context-window flag or GRAY_CONTEXT_WINDOW, then the value auto-fetched from the provider, then a hardcoded per-model fallback.",
      },
      {
        t: "code",
        lang: "bash",
        text: "/context-window            # inspect\n/context-window 128k       # set — 128000, 128k and 1m all parse\n/context-window auto       # clear the override",
      },
      {
        t: "p",
        text: "When usage crosses the window minus a 16k reserve, gray compacts before the next turn: history is summarized into a two-message summary, the same flow as a manual /compact. On a context_length or max_tokens overflow error it compacts and retries once. Auto is the default; no flag needed.",
      },
    ],
  },
  {
    slug: "tools",
    title: "Tools",
    section: "Using gray",
    summary: "The built-in registry the agent calls during a turn.",
    source: "crates/gray-tools",
    blocks: [
      {
        t: "p",
        text: "Tools are a registry of typed handlers. The agent streams a tool call, gray executes it with a cancellation token and a working directory, and the result is fed back into the same turn.",
      },
      {
        t: "table",
        head: ["Tool", "What it does"],
        rows: [
          ["bash", "run a shell command, streamed, cancellable"],
          ["read", "read a file, with truncation for large ones"],
          ["write", "create or replace a file"],
          ["edit", "exact-match replacement with a rendered diff"],
          ["grep", "search file contents"],
          ["find", "search filenames and paths"],
          ["ls", "list a directory"],
          ["delegate_task", "spawn isolated subagents"],
          ["cron", "schedule recurring prompts"],
          ["request_user_input", "ask the operator a question mid-turn"],
        ],
      },
    ],
  },
  {
    slug: "skills",
    title: "Skills",
    section: "Using gray",
    summary: "Markdown procedures the agent can discover and run.",
    source: "crates/gray/src/skills.rs",
    blocks: [
      {
        t: "p",
        text: "A skill is a directory containing SKILL.md, or a plain .md file in a skills root. Discovery walks global (~/.gray/skills) and project (.gray/skills, up to the git root) locations, respecting .gitignore, .ignore and .fdignore.",
      },
      {
        t: "code",
        lang: "bash",
        text: "/skills                    # list what was discovered\n/skills:<name> [args]      # run one",
      },
    ],
  },
  {
    slug: "proxy",
    title: "Proxy",
    section: "Surfaces",
    summary: "Share your Codex, Grok or OpenRouter auth over a local OpenAI endpoint.",
    source: "crates/gray/src/proxy.rs",
    blocks: [
      {
        t: "p",
        text: "gray proxy runs an OpenAI-compatible forwarder on 127.0.0.1:8645. It attaches your existing credential per request — refreshing OAuth tokens when needed — so any tool that speaks the OpenAI protocol can use the account you are already signed into.",
      },
      {
        t: "code",
        lang: "bash",
        text: "gray proxy start --provider openrouter\ngray proxy status\ncurl http://127.0.0.1:8645/health",
      },
      {
        t: "p",
        text: "Only the chat, completions, embeddings, models and responses paths are forwarded; everything else returns 404. Hop-by-hop headers and the inbound Authorization header are stripped before the request goes upstream.",
      },
      {
        t: "note",
        text: "The proxy binds to loopback. It forwards your own credentials — do not expose it on a public interface.",
      },
    ],
  },
  {
    slug: "gateway",
    title: "Messaging gateway",
    section: "Surfaces",
    summary: "The same agent on Telegram, Discord and Slack, as a daemon.",
    source: "crates/gray-gateway",
    blocks: [
      {
        t: "p",
        text: "The gateway maps inbound platform messages to sessions and runs the same agent loop behind them. Sessions can be keyed per user or per thread, so a shared channel does not become one tangled conversation.",
      },
      {
        t: "code",
        lang: "bash",
        text: "gray gateway run          # foreground\ngray gateway install      # systemd user service\ngray gateway status",
      },
      {
        t: "p",
        text: "Configuration lives in ~/.gray/gateway.yaml at 0600. In-chat commands mirror the CLI: /reset starts a fresh session, /status reports the session and model, /stop cancels the running turn.",
      },
    ],
  },
  {
    slug: "cron",
    title: "Cron jobs",
    section: "Surfaces",
    summary: "Run prompts on a schedule, unattended.",
    source: "crates/gray-cron",
    blocks: [
      {
        t: "p",
        text: "Cron jobs are stored prompts with a schedule. A ticker computes what is due, and an inflight guard makes sure a slow run never overlaps with its own next trigger.",
      },
      {
        t: "code",
        lang: "bash",
        text: 'gray cron add "daily 09:00" "summarize yesterday\'s commits"\ngray cron list\ngray cron remove <id>',
      },
    ],
  },
  {
    slug: "slash-commands",
    title: "Slash commands",
    section: "Reference",
    summary: "Every command available at the REPL prompt.",
    source: "crates/gray/src/repl/commands.rs",
    blocks: [
      {
        t: "p",
        text: "Slash commands autocomplete as you type: Enter completes and fires, Tab inserts for editing.",
      },
      {
        t: "table",
        head: ["Command", "Description"],
        rows: [
          ["/connect", "setup provider & API key"],
          ["/model", "switch model"],
          ["/thinking", "reasoning effort"],
          ["/context-window", "set context window (e.g. 128k, auto)"],
          ["/resume", "resume conversation"],
          ["/new", "new conversation"],
          ["/compact", "summarize context"],
          ["/cron", "cron jobs"],
          ["/proxy", "share Codex/Grok/OpenRouter via :8645"],
          ["/gateway", "messaging gateway (Telegram/Discord/Slack)"],
          ["/portal", "portal status"],
          ["/agentsmd", "edit system prompt"],
          ["/skills", "list skills"],
          ["/help", "show commands"],
          ["/quit", "exit"],
        ],
      },
      { t: "h", text: "Aliases" },
      {
        t: "p",
        text: "/clear and /reset map to /new; /exit to /quit; /key, /keys, /provider, /providers and /login to /connect; /effort to /thinking; /compress to /compact; /sys to /agentsmd; /gw to /gateway; /context to /context-window.",
      },
    ],
  },
  {
    slug: "environment",
    title: "Environment variables",
    section: "Reference",
    summary: "Every variable gray reads at startup.",
    source: "crates/gray/src/config.rs",
    blocks: [
      {
        t: "table",
        head: ["Variable", "Meaning"],
        rows: [
          ["GRAY_HOME", "config root (default ~/.gray)"],
          ["GRAY_API_KEY / OPENAI_API_KEY", "API key — env beats stored keys"],
          ["GRAY_MODEL", "default model before ~/.gray/config.json is consulted"],
          ["GRAY_BASE_URL", "default base URL"],
          ["GRAY_CONTEXT_WINDOW", "override the window — 128000, 128k, 1m, or auto"],
          ["GRAY_LOG", "log level: error…trace (default info)"],
        ],
      },
      {
        t: "p",
        text: "Logs are written to ~/.gray/logs/gray.log. Set GRAY_LOG=debug for the firehose.",
      },
    ],
  },
  {
    slug: "architecture",
    title: "Architecture",
    section: "Reference",
    summary: "How the crates fit together.",
    source: "Cargo.toml",
    blocks: [
      {
        t: "table",
        head: ["Crate", "Responsibility"],
        rows: [
          ["gray", "REPL, composer TUI, onboarding, config, CLI"],
          ["gray-core", "the agent loop, typed events, message model"],
          ["gray-provider", "OpenAI-compatible SSE streaming with retries"],
          ["gray-session", "JSONL session store with parent-id branching"],
          ["gray-tools", "the tool registry"],
          ["gray-cron", "schedule parsing, ticker, inflight guards"],
          ["gray-gateway", "Telegram / Discord / Slack daemon"],
          ["gray-markdown", "streaming markdown renderer for the TUI"],
        ],
      },
      {
        t: "p",
        text: "Everything is streaming-first: text deltas, tool calls and usage arrive as typed events, which is what lets the TUI render a turn as it happens rather than after it finishes.",
      },
    ],
  },
];

export function docBySlug(slug: string): Doc | undefined {
  return docs.find((d) => d.slug === slug);
}

export function docsBySection(): Array<{ section: string; items: Doc[] }> {
  return sections.map((section) => ({
    section,
    items: docs.filter((d) => d.section === section),
  }));
}
