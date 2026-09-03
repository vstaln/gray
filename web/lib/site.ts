/** Single source of truth for copy that also lives in the Rust CLI. */

export const site = {
  name: "gray",
  tagline: "a minimal agent harness",
  description:
    "A minimal coding agent that runs tools, edits code, and works with any model provider. One Rust binary, no runtime.",
  url: "https://gray.alignment.id",
  repo: "https://github.com/vstaln/gray",
  dl: "https://gray.alignment.id/dl",
} as const;

export const install = {
  unix: "curl -fsSL https://gray.alignment.id/install.sh | sh",
  unixBeta: "curl -fsSL https://gray.alignment.id/install.sh | sh -s -- beta",
  windows: "iwr https://gray.alignment.id/install.ps1 -UseBasicParsing | iex",
  source: "cargo build --release -p gray",
} as const;

/** Mirrors crates/gray/src/repl/commands.rs COMMANDS. */
export const slashCommands: ReadonlyArray<readonly [string, string]> = [
  ["/connect", "setup provider & API key"],
  ["/model", "switch model"],
  ["/thinking", "reasoning effort"],
  ["/context", "set context window (e.g. 128k, auto)"],
  ["/resume", "resume conversation"],
  ["/new", "new conversation"],
  ["/compact", "summarize context"],
  ["/cron", "cron jobs"],
  ["/proxy", "share Codex/Grok/OpenRouter via :8645"],
  ["/gateway", "messaging gateway (Telegram/Discord/Slack)"],
  ["/agentsmd", "edit system prompt"],
  ["/skills", "list skills"],
  ["/help", "show commands"],
  ["/quit", "exit"],
];

/** Section 5 of the landing page. Every claim maps to a crate. */
export const panels = [
  {
    n: "01",
    kicker: "Ship",
    title: "One Binary",
    body: "Rust, statically linked, four platforms. No node_modules, no venv, no runtime to install. It starts in milliseconds and it is the whole product.",
    image: "/space/moon-dither.png",
    alt: "Dithered Apollo 16 lunar module on the lunar surface",
  },
  {
    n: "02",
    kicker: "Connect",
    title: "Any Provider",
    body: "OpenRouter, DeepSeek, Groq, OpenAI, ollama, vLLM, LM Studio — plus OAuth sign-in for xAI/Grok and Codex/ChatGPT accounts. Switch models mid-session.",
    image: "/space/jupiter-dither.png",
    alt: "Dithered Juno image of a Jupiter storm",
  },
  {
    n: "03",
    kicker: "Remember",
    title: "Sessions That Survive",
    body: "Every turn appends to JSONL on disk with parent-id branching. Ctrl-C mid-turn still persists what reached memory. gray -c reopens the latest.",
    image: "/space/helix-dither.png",
    alt: "Dithered Helix Nebula",
  },
  {
    n: "04",
    kicker: "Delegate",
    title: "Subagents",
    body: "delegate_task spawns isolated children with their own registry and cancellation token, ten concurrent, background-durable through a SQLite queue.",
    image: "/space/saturn-dither.png",
    alt: "Dithered backlit Saturn from Cassini",
  },
  {
    n: "05",
    kicker: "Live",
    title: "Lives Everywhere",
    body: "A gateway daemon puts the same agent on Telegram, Discord and Slack, with per-user sessions and /reset /status /stop. systemd unit included.",
    image: "/space/aurora-dither.png",
    alt: "Dithered aurora over North America from orbit",
  },
  {
    n: "06",
    kicker: "Schedule",
    title: "Unattended",
    body: "Cron jobs run prompts on a schedule while you are gone — reports, backups, briefings — with inflight guards so a slow run never doubles up.",
    image: "/space/eclipse-dither.png",
    alt: "Dithered total solar eclipse corona",
  },
] as const;

export const tiers = [
  {
    id: "free",
    name: "Free",
    price: "$0",
    period: "forever",
    image: "/space/bluemarble-dither.png",
    alt: "Dithered Blue Marble Earth",
    features: [
      "The full binary, MIT",
      "Every provider, BYOK",
      "All tools & skills",
      "Local sessions",
      "Community support",
    ],
    cta: "Install",
    href: "#install",
    featured: false,
    available: true,
  },
  {
    id: "pro",
    name: "Pro",
    price: "$20",
    period: "per month",
    image: "/space/carina-dither.png",
    alt: "Dithered Carina Nebula cosmic cliffs",
    features: [
      "Hosted gateway",
      "Telegram · Discord · Slack",
      "Cloud cron",
      "Session sync",
      "Priority builds",
    ],
    cta: "Not yet available",
    href: null,
    featured: true,
    available: false,
  },
  {
    id: "team",
    name: "Team",
    price: "$100",
    period: "per month",
    image: "/space/andromeda-dither.png",
    alt: "Dithered Andromeda Galaxy",
    features: [
      "Everything in Pro",
      "Five seats",
      "Shared skills registry",
      "Audit log",
      "SSO",
    ],
    cta: "Not yet available",
    href: null,
    featured: false,
    available: false,
  },
] as const;
