//! Toolset Distributions — 1:1 port of `toolset_distributions.py` (358 LOC).
//!
//! Defines named distributions of toolsets for data-generation runs.
//! Each distribution maps toolset names to their selection probability (%).
//! Probabilities are independent per-toolset (each rolls 0..100); the system
//! normalizes nothing — the caller samples each entry independently.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Distribution definition
// ---------------------------------------------------------------------------

/// A single distribution: description + per-toolset inclusion probabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Distribution {
    /// Human-readable description.
    pub description: &'static str,
    /// Slice of `(toolset_name, probability_percent)` — `0..=100`.
    pub toolsets: &'static [(&'static str, u8)],
}

/// All named distributions — mirrors `DISTRIBUTIONS` in Python.
pub static DISTRIBUTIONS: &[(&str, Distribution)] = &[
    (
        "default",
        Distribution {
            description: "All available tools, all the time",
            toolsets: &[
                ("web", 100),
                ("vision", 100),
                ("image_gen", 100),
                ("terminal", 100),
                ("file", 100),
                ("browser", 100),
            ],
        },
    ),
    (
        "image_gen",
        Distribution {
            description: "Heavy focus on image generation with vision and web support",
            toolsets: &[("image_gen", 90), ("vision", 90), ("web", 55), ("terminal", 45)],
        },
    ),
    (
        "research",
        Distribution {
            description: "Web research with vision analysis and reasoning",
            toolsets: &[("web", 90), ("browser", 70), ("vision", 50), ("terminal", 10)],
        },
    ),
    (
        "science",
        Distribution {
            description: "Scientific research with web, terminal, file, and browser capabilities",
            toolsets: &[
                ("web", 94),
                ("terminal", 94),
                ("file", 94),
                ("vision", 65),
                ("browser", 50),
                ("image_gen", 15),
            ],
        },
    ),
    (
        "development",
        Distribution {
            description: "Terminal, file tools, and reasoning with occasional web lookup",
            toolsets: &[("terminal", 80), ("file", 80), ("web", 30), ("vision", 10)],
        },
    ),
    (
        "safe",
        Distribution {
            description: "All tools except terminal for safety",
            toolsets: &[("web", 80), ("browser", 70), ("vision", 60), ("image_gen", 60)],
        },
    ),
    (
        "balanced",
        Distribution {
            description: "Equal probability of all toolsets",
            toolsets: &[
                ("web", 50),
                ("vision", 50),
                ("image_gen", 50),
                ("terminal", 50),
                ("file", 50),
                ("browser", 50),
            ],
        },
    ),
    (
        "minimal",
        Distribution {
            description: "Only web tools for basic research",
            toolsets: &[("web", 100)],
        },
    ),
    (
        "terminal_only",
        Distribution {
            description: "Terminal and file tools for code execution tasks",
            toolsets: &[("terminal", 100), ("file", 100)],
        },
    ),
    (
        "terminal_web",
        Distribution {
            description: "Terminal and file tools with web search for documentation lookup",
            toolsets: &[("terminal", 100), ("file", 100), ("web", 100)],
        },
    ),
    (
        "creative",
        Distribution {
            description: "Image generation and vision analysis focus",
            toolsets: &[("image_gen", 90), ("vision", 90), ("web", 30)],
        },
    ),
    (
        "reasoning",
        Distribution {
            description: "Heavy research/reasoning distribution with minimal other tools",
            toolsets: &[("web", 90), ("file", 60), ("terminal", 20)],
        },
    ),
    (
        "browser_use",
        Distribution {
            description: "Full browser-based web interaction with search, vision, and page control",
            toolsets: &[("browser", 100), ("web", 80), ("vision", 70)],
        },
    ),
    (
        "browser_only",
        Distribution {
            description: "Only browser automation tools for pure web interaction tasks",
            toolsets: &[("browser", 100)],
        },
    ),
    (
        "browser_tasks",
        Distribution {
            description: "Browser-focused distribution (browser toolset includes web_search for finding URLs since Google blocks direct browser searches)",
            toolsets: &[("browser", 97), ("vision", 12), ("terminal", 15)],
        },
    ),
    (
        "terminal_tasks",
        Distribution {
            description:
                "Terminal-focused distribution with high terminal/file availability, occasional other tools",
            toolsets: &[
                ("terminal", 97),
                ("file", 97),
                ("web", 97),
                ("browser", 75),
                ("vision", 50),
                ("image_gen", 10),
            ],
        },
    ),
    (
        "mixed_tasks",
        Distribution {
            description:
                "Mixed distribution with high browser, terminal, and file availability for complex tasks",
            toolsets: &[
                ("browser", 92),
                ("terminal", 92),
                ("file", 92),
                ("web", 35),
                ("vision", 15),
                ("image_gen", 15),
            ],
        },
    ),
];

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Known toolset names — mirrors the static `TOOLSETS` keys in `toolsets.py`.
/// Used by `validate_toolset` so sampling can warn on unknown entries
/// (faithful to `from toolsets import validate_toolset`).
const KNOWN_TOOLSETS: &[&str] = &[
    // core 6 used by distributions
    "web",
    "vision",
    "image_gen",
    "terminal",
    "file",
    "browser",
    // full static TOOLSETS inventory
    "search",
    "x_search",
    "video",
    "video_gen",
    "bfl",
    "computer_use",
    "skills",
    "cronjob",
    "tts",
    "todo",
    "memory",
    "context_engine",
    "session_search",
    "project",
    "desktop_ui",
    "clarify",
    "code_execution",
    "delegation",
    "homeassistant",
    "kanban",
    "discord",
    "discord_admin",
    "yuanbao",
    "feishu_doc",
    "feishu_drive",
    "spotify",
    "debugging",
    "safe",
    "coding",
    "hermes-acp",
    "hermes-api-server",
    "hermes-cli",
    "hermes-cron",
    "hermes-telegram",
    "hermes-discord",
    "hermes-whatsapp",
    "hermes-slack",
    "hermes-signal",
    "hermes-bluebubbles",
    "hermes-homeassistant",
    "hermes-email",
    "hermes-mattermost",
    "hermes-matrix",
    "hermes-dingtalk",
    "hermes-feishu",
    "hermes-weixin",
    "hermes-qqbot",
    "hermes-wecom",
    "hermes-wecom-callback",
    "hermes-yuanbao",
    "hermes-sms",
    "hermes-webhook",
    "hermes-gateway",
    // aliases
    "all",
    "*",
];

/// Check if a toolset name is valid — mirrors `toolsets.validate_toolset`.
pub fn validate_toolset(name: &str) -> bool {
    KNOWN_TOOLSETS.contains(&name)
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python functions 1:1
// ---------------------------------------------------------------------------

/// Get a toolset distribution by name.
///
/// Returns `None` if the distribution is not found — mirrors
/// `DISTRIBUTIONS.get(name)` returning `None`.
pub fn get_distribution(name: &str) -> Option<&'static Distribution> {
    DISTRIBUTIONS
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// List all available distributions — mirrors `DISTRIBUTIONS.copy()`.
pub fn list_distributions() -> HashMap<&'static str, &'static Distribution> {
    DISTRIBUTIONS
        .iter()
        .map(|(k, v)| (*k, v))
        .collect()
}

/// Check if a distribution name is valid.
pub fn validate_distribution(name: &str) -> bool {
    DISTRIBUTIONS.iter().any(|(k, _)| *k == name)
}

/// Sample toolsets based on a distribution's probabilities.
///
/// Each toolset rolls independently (`random * 100 < probability`).
/// If no toolset is selected, the highest-probability toolset is forced
/// (if it validates) — exactly as in Python.
///
/// Returns `Err` if the distribution name is unknown (mirrors `ValueError`).
pub fn sample_toolsets_from_distribution(
    distribution_name: &str,
) -> Result<Vec<String>, String> {
    let dist = get_distribution(distribution_name)
        .ok_or_else(|| format!("Unknown distribution: {}", distribution_name))?;

    let mut selected = Vec::new();

    for (toolset_name, probability) in dist.toolsets {
        if !validate_toolset(toolset_name) {
            eprintln!(
                "⚠️  Warning: Toolset '{}' in distribution '{}' is not valid",
                toolset_name, distribution_name
            );
            continue;
        }
        if random_f64() * 100.0 < f64::from(*probability) {
            selected.push(toolset_name.to_string());
        }
    }

    // Fallback: ensure at least one toolset (highest probability).
    if selected.is_empty() && !dist.toolsets.is_empty() {
        if let Some((highest, _)) = dist.toolsets.iter().max_by_key(|(_, p)| *p) {
            if validate_toolset(highest) {
                selected.push(highest.to_string());
            }
        }
    }

    Ok(selected)
}

/// Print detailed information about a distribution — mirrors `print_distribution_info`.
pub fn print_distribution_info(distribution_name: &str) {
    let dist = match get_distribution(distribution_name) {
        Some(d) => d,
        None => {
            println!("❌ Unknown distribution: {}", distribution_name);
            return;
        }
    };

    println!("\n📊 Distribution: {}", distribution_name);
    println!("   Description: {}", dist.description);
    println!("   Toolsets:");
    let mut sorted = dist.toolsets.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (toolset, prob) in sorted {
        println!("     • {:15} : {:3}% chance", toolset, prob);
    }
}

// ---------------------------------------------------------------------------
// Minimal stdlib RNG — no new crate (ponytail ladder rung 3: stdlib first)
// ---------------------------------------------------------------------------

thread_local! {
    static RNG_STATE: Cell<u64> = Cell::new(seed());
}

fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
        ^ 0x123456789ABCDEF0u64
}

fn next_u64() -> u64 {
    RNG_STATE.with(|c| {
        let mut x = c.get();
        if x == 0 {
            x = 0x9E3779B97F4A7C15;
        }
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let res = x.wrapping_mul(0x2545F4914F6CDD1D);
        c.set(x);
        res
    })
}

fn random_f64() -> f64 {
    // 53 bits -> [0,1)
    const SCALE: f64 = 1.0 / ((1u64 << 53) as f64);
    ((next_u64() >> 11) as f64) * SCALE
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_unknown_is_none() {
        assert!(get_distribution("nope").is_none());
        assert!(!validate_distribution("nope"));
    }

    #[test]
    fn get_known_has_description() {
        let d = get_distribution("default").unwrap();
        assert_eq!(d.description, "All available tools, all the time");
        assert_eq!(d.toolsets.len(), 6);
    }

    #[test]
    fn list_contains_all() {
        let all = list_distributions();
        assert_eq!(all.len(), 17);
        assert!(all.contains_key("browser_tasks"));
        assert!(all.contains_key("mixed_tasks"));
    }

    #[test]
    fn validate_distribution_known() {
        assert!(validate_distribution("research"));
        assert!(validate_distribution("minimal"));
    }

    #[test]
    fn sample_unknown_is_err() {
        assert!(sample_toolsets_from_distribution("nope").is_err());
    }

    #[test]
    fn sample_always_returns_at_least_one() {
        // Run many times to cover random path + fallback.
        for _ in 0..20 {
            let s = sample_toolsets_from_distribution("balanced").unwrap();
            assert!(!s.is_empty());
            for name in &s {
                assert!(validate_toolset(name));
            }
        }
    }

    #[test]
    fn sample_minimal_always_web() {
        // minimal is web 100% — must always contain web
        for _ in 0..10 {
            let s = sample_toolsets_from_distribution("minimal").unwrap();
            assert_eq!(s, vec!["web"]);
        }
    }

    #[test]
    fn sample_default_always_all_six() {
        // default is 100% for all — always returns all 6
        let mut s = sample_toolsets_from_distribution("default").unwrap();
        s.sort();
        let mut expected = vec!["browser", "file", "image_gen", "terminal", "vision", "web"];
        expected.sort();
        assert_eq!(s, expected);
    }
}
