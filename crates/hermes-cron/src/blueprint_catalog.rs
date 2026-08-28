//! Automation Blueprints — parameterized automation blueprints with typed slots.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cron/blueprint_catalog.py` (799 lines).
//!
//! A *blueprint* is a one-place definition of an automation that every surface
//! renders natively:
//!
//!   * Dashboard / GUI app  -> a form (one field per slot)
//!   * CLI / TUI / messenger -> a pre-filled ``/blueprint`` slash command
//!   * Agent                 -> a seed prompt; it asks for any blank/ambiguous slot
//!   * Docs catalog          -> a copy-paste command + a ``hermes://`` deep-link
//!
//! The single source of truth is the slot schema below. ``blueprint_form_schema``
//! emits what a form renderer needs; ``blueprint_slash_command`` emits the flattened
//! one-line command; ``fill_blueprint`` validates user-supplied values and turns a
//! blueprint into a ``cron.jobs.create_job`` kwargs dict (so there is no second job
//! engine). The form-where-there's-a-screen / agent-fills-where-there's-a-chat
//! split both consume this same module.
//!
//! Design choice: users never type raw cron. A blueprint carries a fixed recurrence
//! in ``schedule_template`` and parameterizes only the human-friendly parts
//! (time-of-day, weekday set). Blueprints needing full flexibility expose a ``text``
//! slot named ``schedule`` that passes through verbatim.
//!
//! Python source docstring (preserved):
//! ```text
//! Automation Blueprints — parameterized automation blueprints with typed slots.
//! ...
//! Design choice: users never type raw cron. A blueprint carries a fixed recurrence
//! in ``schedule_template`` and parameterizes only the human-friendly parts
//! (time-of-day, weekday set). Blueprints needing full flexibility expose a ``text``
//! slot named ``schedule`` that passes through verbatim.
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// __all__ — mirrors Python `__all__`
// ---------------------------------------------------------------------------

/// Mirrors Python `__all__`.
pub const ALL: &[&str] = &[
    "BlueprintSlot",
    "AutomationBlueprint",
    "CATALOG",
    "get_blueprint",
    "blueprint_form_schema",
    "blueprint_slash_command",
    "blueprint_deeplink",
    "blueprint_catalog_entry",
    "fill_blueprint",
    "BlueprintFillError",
    "WEEKDAY_PRESETS",
];

// ---------------------------------------------------------------------------
// Error — mirrors `class BlueprintFillError(ValueError)`
// ---------------------------------------------------------------------------

/// Raised when supplied slot values fail validation.
/// Mirrors `class BlueprintFillError(ValueError)`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct BlueprintFillError(pub String);

impl BlueprintFillError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

// ---------------------------------------------------------------------------
// Constants — mirrors Python module globals
// ---------------------------------------------------------------------------

/// Slot types the renderers understand.
/// Mirrors `_SLOT_TYPES = frozenset({"time", "enum", "text", "weekdays"})`.
pub const SLOT_TYPES: &[&str] = &["time", "enum", "text", "weekdays"];

/// Named weekday recurrences -> cron day-of-week field.
/// Mirrors `WEEKDAY_PRESETS: Dict[str, str]`.
pub const WEEKDAY_PRESETS: &[(&str, &str)] = &[
    ("everyday", "*"),
    ("weekdays", "1-5"),
    ("weekends", "0,6"),
];

/// Cron day name -> dow field. Mirrors `_DAY_TO_DOW`.
pub const DAY_TO_DOW: &[(&str, &str)] = &[
    ("sunday", "0"),
    ("monday", "1"),
    ("tuesday", "2"),
    ("wednesday", "3"),
    ("thursday", "4"),
    ("friday", "5"),
    ("saturday", "6"),
];

fn weekday_preset_map() -> HashMap<String, String> {
    WEEKDAY_PRESETS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn day_to_dow_map() -> HashMap<String, String> {
    DAY_TO_DOW
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn is_valid_slot_type(t: &str) -> bool {
    SLOT_TYPES.contains(&t)
}

// ---------------------------------------------------------------------------
// Data model — mirrors `@dataclass(frozen=True) class BlueprintSlot`
// ---------------------------------------------------------------------------

/// A single fillable field on a blueprint.
/// Mirrors `@dataclass(frozen=True) class BlueprintSlot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintSlot {
    pub name: String,
    /// Slot type (`time` | `enum` | `text` | `weekdays`).
    #[serde(rename = "type")]
    pub slot_type: String,
    pub label: String,
    /// `None` mirrors Python `default=None`; `Some(Value::String(""))` mirrors `default=""`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub help: String,
    #[serde(default = "default_true")]
    pub strict: bool,
}

fn default_true() -> bool {
    true
}

impl BlueprintSlot {
    /// Create a slot, validating `type` against `_SLOT_TYPES`.
    /// Mirrors `BlueprintSlot.__post_init__`.
    pub fn new(
        name: impl Into<String>,
        slot_type: impl Into<String>,
        label: impl Into<String>,
        default: Option<Value>,
        options: Vec<String>,
        optional: bool,
        help: impl Into<String>,
        strict: bool,
    ) -> Result<Self, String> {
        let name = name.into();
        let slot_type = slot_type.into();
        if !is_valid_slot_type(&slot_type) {
            return Err(format!(
                "unknown slot type {slot_type:?} (slot {name})"
            ));
        }
        Ok(Self {
            name,
            slot_type,
            label: label.into(),
            default,
            options,
            optional,
            help: help.into(),
            strict,
        })
    }

    /// Panicking constructor for static catalog building (valid types only).
    pub fn new_unchecked(
        name: &str,
        slot_type: &str,
        label: &str,
        default: Option<Value>,
        options: Vec<&str>,
        optional: bool,
        help: &str,
        strict: bool,
    ) -> Self {
        if !is_valid_slot_type(slot_type) {
            panic!("unknown slot type {slot_type:?} (slot {name})");
        }
        Self {
            name: name.to_string(),
            slot_type: slot_type.to_string(),
            label: label.to_string(),
            default,
            options: options.into_iter().map(|s| s.to_string()).collect(),
            optional,
            help: help.to_string(),
            strict,
        }
    }
}

// ---------------------------------------------------------------------------
// AutomationBlueprint — mirrors `@dataclass(frozen=True) class AutomationBlueprint`
// ---------------------------------------------------------------------------

/// A parameterized automation blueprint.
/// Mirrors `@dataclass(frozen=True) class AutomationBlueprint`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationBlueprint {
    pub key: String,
    pub title: String,
    pub description: String,
    pub category: String,
    /// Cron expression with `{slot}` placeholders, e.g. "{minute} {hour} * * {dow}".
    pub schedule_template: String,
    /// Seed instruction for the agent / the cron job prompt; may contain {slot}s.
    pub prompt_template: String,
    #[serde(default)]
    pub slots: Vec<BlueprintSlot>,
    #[serde(default = "default_deliver")]
    pub deliver_default: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_deliver() -> String {
    "origin".to_string()
}

// ---------------------------------------------------------------------------
// Slot factories — mirrors `_TIME` / `_DELIVER`
// ---------------------------------------------------------------------------

fn time_slot(default: &str) -> BlueprintSlot {
    BlueprintSlot {
        name: "time".to_string(),
        slot_type: "time".to_string(),
        label: "What time?".to_string(),
        default: Some(Value::String(default.to_string())),
        options: Vec::new(),
        optional: false,
        help: "24h local time, e.g. 08:00".to_string(),
        strict: true,
    }
}

fn deliver_slot() -> BlueprintSlot {
    BlueprintSlot {
        name: "deliver".to_string(),
        slot_type: "enum".to_string(),
        label: "Where to deliver?".to_string(),
        default: Some(Value::String("origin".to_string())),
        options: vec![
            "origin".to_string(),
            "local".to_string(),
            "telegram".to_string(),
            "discord".to_string(),
            "email".to_string(),
        ],
        optional: false,
        help: "origin = the chat you set this up from (or your configured home channel when created from the dashboard); local = save only, no message; or any connected platform name".to_string(),
        strict: false,
    }
}

// ---------------------------------------------------------------------------
// CATALOG — mirrors `CATALOG: List[AutomationBlueprint] = [...]`
// ---------------------------------------------------------------------------

fn build_catalog() -> Vec<AutomationBlueprint> {
    vec![
        AutomationBlueprint {
            key: "morning-brief".to_string(),
            title: "Morning briefing".to_string(),
            description: "A short daily briefing: today's calendar, weather, and anything urgent waiting on you.".to_string(),
            category: "daily".to_string(),
            schedule_template: "{minute} {hour} * * *".to_string(),
            prompt_template: "Produce a concise morning briefing for the user: today's calendar events, the local weather, and any urgent items. When Gmail/Google Calendar are connected, follow the google-workspace skill's references/daily-brief.md procedure (exact day window, conflict detection, meeting prep, mail-to-meeting links). Keep it short and scannable. If no data sources are connected, give a brief good-morning with the date and offer to connect calendar/email.".to_string(),
            slots: vec![time_slot("08:00"), deliver_slot()],
            deliver_default: "origin".to_string(),
            skills: vec!["google-workspace".to_string()],
            tags: vec!["daily".to_string(), "briefing".to_string()],
        },
        AutomationBlueprint {
            key: "important-mail".to_string(),
            title: "Important-mail monitor".to_string(),
            description: "Check your inbox periodically and ping you ONLY about mail that actually needs attention.".to_string(),
            category: "email".to_string(),
            schedule_template: "*/{interval_min} * * * *".to_string(),
            prompt_template: "Check the user's inbox for new messages since the last run. Surface ONLY mail matching: {criteria}. Score candidates with the urgency classifier and deliver only what clears the bar; if nothing does, respond with [SILENT]. Requires a connected mail source; if none is configured, explain how to connect one and stop.".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("interval_min", "enum", "How often?", Some(json!("30")), vec!["15", "30", "60"], false, "minutes between checks", true),
                BlueprintSlot::new_unchecked("criteria", "text", "Only notify me if the mail…", Some(json!("needs a reply today, is from my manager or family, or mentions a deadline")), vec![], false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec!["email-inbox-triage".to_string()],
            tags: vec!["email".to_string(), "monitor".to_string()],
        },
        AutomationBlueprint {
            key: "weekly-review".to_string(),
            title: "Weekly review".to_string(),
            description: "A weekly recap: what got done, what's still open, and what's coming up.".to_string(),
            category: "weekly".to_string(),
            schedule_template: "{minute} {hour} * * {dow}".to_string(),
            prompt_template: "Run the weekly-review-planning skill's procedure for the user: review the completed week and coming 1-2 weeks across connected calendar, tasks, notes, and email; surface commitments, stalled projects, and waiting items; build a capacity-aware plan for next week. Recommendations and drafts only — no mutations without approval. Keep the output in the skill's seven-section shape.".to_string(),
            slots: vec![
                time_slot("18:00"),
                BlueprintSlot::new_unchecked("day", "enum", "Which day?", Some(json!("sunday")), vec!["sunday", "monday", "friday", "saturday"], false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec!["weekly-review-planning".to_string()],
            tags: vec!["weekly".to_string(), "review".to_string()],
        },
        AutomationBlueprint {
            key: "workday-start".to_string(),
            title: "Workday start reminder".to_string(),
            description: "A weekday nudge with your agenda and top priorities.".to_string(),
            category: "daily".to_string(),
            schedule_template: "{minute} {hour} * * 1-5".to_string(),
            prompt_template: "Give the user a brief weekday start-of-day nudge: today's calendar and the 1-3 highest-priority things to focus on, inferred from recent context and any task tools. Encouraging, short, one message.".to_string(),
            slots: vec![time_slot("09:00"), deliver_slot()],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["daily".to_string(), "focus".to_string()],
        },
        AutomationBlueprint {
            key: "custom-reminder".to_string(),
            title: "Custom reminder".to_string(),
            description: "A recurring reminder in your own words, on your schedule.".to_string(),
            category: "general".to_string(),
            schedule_template: "{minute} {hour} * * {dow}".to_string(),
            prompt_template: "Remind the user: {what}".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("what", "text", "Remind me to…", Some(json!("take a break and stretch")), vec![], false, "", true),
                time_slot("14:00"),
                BlueprintSlot::new_unchecked("recurrence", "weekdays", "Repeat on", Some(json!("everyday")), WEEKDAY_PRESETS.iter().map(|(k, _)| *k).collect(), false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["reminder".to_string()],
        },
        AutomationBlueprint {
            key: "evening-winddown".to_string(),
            title: "Evening wind-down".to_string(),
            description: "An end-of-day check-in: tomorrow's calendar at a glance and anything you should prep tonight.".to_string(),
            category: "daily".to_string(),
            schedule_template: "{minute} {hour} * * *".to_string(),
            prompt_template: "Give the user a short evening wind-down: tomorrow's calendar, any early commitments to prep for, and one gentle nudge to wrap up loose ends from today. Keep it calm and brief — one message. If no calendar is connected, just offer a friendly sign-off and the weather for tomorrow.".to_string(),
            slots: vec![time_slot("21:00"), deliver_slot()],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["daily".to_string(), "evening".to_string()],
        },
        AutomationBlueprint {
            key: "news-digest".to_string(),
            title: "Topic news digest".to_string(),
            description: "A recurring digest on a topic you care about — deduped against what was already sent, so only genuinely new items land.".to_string(),
            category: "general".to_string(),
            schedule_template: "{minute} {hour} * * {dow}".to_string(),
            prompt_template: "Search the web for new and noteworthy items about: {topic}. Dedupe against what you sent in previous runs — only include genuinely new developments. Deliver a tight digest of at most {count} bullets, each one line with a link. If nothing new since last run, respond with [SILENT].".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("topic", "text", "What topic?", Some(json!("AI and technology")), vec![], false, "a subject, product, person, or search phrase", true),
                time_slot("18:00"),
                BlueprintSlot::new_unchecked("recurrence", "weekdays", "Repeat on", Some(json!("weekdays")), WEEKDAY_PRESETS.iter().map(|(k, _)| *k).collect(), false, "", true),
                BlueprintSlot::new_unchecked("count", "enum", "How many bullets?", Some(json!("5")), vec!["3", "5", "8"], false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["digest".to_string(), "research".to_string()],
        },
        AutomationBlueprint {
            key: "bill-renewal-watch".to_string(),
            title: "Bills & renewals reminder".to_string(),
            description: "A heads-up before a recurring payment, subscription renewal, or due date — so nothing auto-charges by surprise.".to_string(),
            category: "general".to_string(),
            schedule_template: "{minute} {hour} * * {dow}".to_string(),
            prompt_template: "Remind the user about an upcoming payment or renewal: {what}. Phrase it as an actionable heads-up (e.g. 'review or cancel before it renews'), not just a notification. One short message.".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("what", "text", "What's due?", Some(json!("my streaming subscription renews soon")), vec![], false, "", true),
                time_slot("10:00"),
                BlueprintSlot::new_unchecked("recurrence", "weekdays", "Repeat on", Some(json!("everyday")), WEEKDAY_PRESETS.iter().map(|(k, _)| *k).collect(), false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["reminder".to_string(), "finance".to_string()],
        },
        AutomationBlueprint {
            key: "price-watch".to_string(),
            title: "Price & availability watch".to_string(),
            description: "Watch an exact product, flight, hotel, or listing and alert when your price or availability condition is met.".to_string(),
            category: "general".to_string(),
            schedule_template: "0 */{interval_h} * * *".to_string(),
            prompt_template: "Load the product-price-monitor skill and run the tick for this watch: {item}. Alert condition: {condition}. Compare the normalized all-in price/availability against stored state, suppress duplicate alerts, and never overwrite last-known-good state with a failed fetch. If no condition is met, respond with [SILENT]. On the first run, execute the skill's setup phase first: pin the exact item, verify one live fetch, and write the watch contract state file.".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("item", "text", "What exactly to watch?", Some(json!("a product URL or exact flight/hotel/listing description")), vec![], false, "URL or precise description — variant, dates, seller", true),
                BlueprintSlot::new_unchecked("condition", "text", "Alert me when…", Some(json!("the all-in price drops below my target")), vec![], false, "threshold price (state the currency), availability, or terms change", true),
                BlueprintSlot::new_unchecked("interval_h", "enum", "How often?", Some(json!("6")), vec!["1", "3", "6", "12", "24"], false, "hours between checks — be gentle with rate limits", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec!["product-price-monitor".to_string()],
            tags: vec!["prices".to_string(), "shopping".to_string(), "travel".to_string(), "monitor".to_string()],
        },
        AutomationBlueprint {
            key: "competitor-watch".to_string(),
            title: "Competitor news watch".to_string(),
            description: "Track named companies for material news — launches, pricing, funding, filings — with a cited digest.".to_string(),
            category: "general".to_string(),
            schedule_template: "{minute} {hour} * * {dow}".to_string(),
            prompt_template: "Load the competitor-news-monitor skill and run the tick for this watch: companies {companies}; event categories {categories}. Collect incrementally from the last cutoff, deduplicate by underlying event, score materiality against the watch contract, and deliver a cited digest of material events only. If there are no material events, respond with [SILENT]. On the first run, execute the skill's setup phase first: freeze the watchlist, build source coverage, and write the watch contract state file.".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("companies", "text", "Which companies?", Some(json!("two or three competitors, by canonical name")), vec![], false, "canonical names and domains; aliases help dedup", true),
                BlueprintSlot::new_unchecked("categories", "text", "Which events matter?", Some(json!("product launches, pricing changes, funding, partnerships, executive moves, incidents")), vec![], false, "", true),
                time_slot("09:00"),
                BlueprintSlot::new_unchecked("recurrence", "weekdays", "Repeat on", Some(json!("monday")), WEEKDAY_PRESETS.iter().map(|(k, _)| *k).collect(), false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec!["competitor-news-monitor".to_string()],
            tags: vec!["competitors".to_string(), "news".to_string(), "monitor".to_string(), "research".to_string()],
        },
        AutomationBlueprint {
            key: "habit-checkin".to_string(),
            title: "Habit check-in".to_string(),
            description: "A recurring nudge to keep a habit on track and reflect on whether you did it.".to_string(),
            category: "general".to_string(),
            schedule_template: "{minute} {hour} * * {dow}".to_string(),
            prompt_template: "Nudge the user about their habit: {habit}. Ask whether they did it today, keep it warm and non-judgmental, and offer a one-line word of encouragement. One short message.".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("habit", "text", "Which habit?", Some(json!("20 minutes of reading")), vec![], false, "", true),
                time_slot("20:00"),
                BlueprintSlot::new_unchecked("recurrence", "weekdays", "Repeat on", Some(json!("everyday")), WEEKDAY_PRESETS.iter().map(|(k, _)| *k).collect(), false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["habit".to_string(), "wellbeing".to_string()],
        },
        AutomationBlueprint {
            key: "hydration-move".to_string(),
            title: "Hydration & movement nudge".to_string(),
            description: "A periodic nudge during the day to drink water, stand up, and stretch.".to_string(),
            category: "general".to_string(),
            schedule_template: "0 {start_hour}-{end_hour}/{interval_hours} * * 1-5".to_string(),
            prompt_template: "Send the user a brief, friendly nudge to drink some water, stand up, and stretch for a moment. Vary the wording each time so it doesn't feel robotic. One short line.".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("interval_hours", "enum", "How often?", Some(json!("1")), vec!["1", "2", "3"], false, "hours between nudges", true),
                BlueprintSlot::new_unchecked("start_hour", "enum", "Start hour", Some(json!("9")), vec!["7", "8", "9", "10"], false, "first hour of the active window (24h)", true),
                BlueprintSlot::new_unchecked("end_hour", "enum", "End hour", Some(json!("17")), vec!["16", "17", "18", "19"], false, "last hour of the active window (24h)", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["wellbeing".to_string(), "focus".to_string()],
        },
        AutomationBlueprint {
            key: "meal-plan".to_string(),
            title: "Weekly meal plan".to_string(),
            description: "A weekly meal plan plus a consolidated grocery list, tuned to your diet and how much time you have to cook.".to_string(),
            category: "weekly".to_string(),
            schedule_template: "{minute} {hour} * * {dow}".to_string(),
            prompt_template: "Build the user a meal plan for the coming week: {meals} per day, suited to a {diet} diet and roughly {effort} cooking effort. Include a consolidated grocery list grouped by aisle. Keep blueprints simple and skimmable.".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("diet", "enum", "Diet?", Some(json!("no restrictions")), vec!["no restrictions", "vegetarian", "vegan", "high-protein", "low-carb"], false, "", true),
                BlueprintSlot::new_unchecked("meals", "enum", "Meals per day?", Some(json!("dinner only")), vec!["dinner only", "lunch and dinner", "all three"], false, "", true),
                BlueprintSlot::new_unchecked("effort", "enum", "Cooking effort?", Some(json!("quick")), vec!["quick", "medium", "ambitious"], false, "", true),
                time_slot("17:00"),
                BlueprintSlot::new_unchecked("day", "enum", "Which day?", Some(json!("sunday")), vec!["sunday", "monday", "friday", "saturday"], false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["weekly".to_string(), "food".to_string()],
        },
        AutomationBlueprint {
            key: "learn-daily".to_string(),
            title: "Daily learning drip".to_string(),
            description: "One bite-sized lesson a day on a topic you want to learn, building progressively over time.".to_string(),
            category: "daily".to_string(),
            schedule_template: "{minute} {hour} * * {dow}".to_string(),
            prompt_template: "Teach the user one bite-sized lesson about: {topic}. Build on earlier lessons so it progresses rather than repeating. Keep it to a couple of short paragraphs with one concrete example, and end with a single question to check understanding.".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("topic", "text", "Learn about…", Some(json!("Spanish vocabulary")), vec![], false, "", true),
                time_slot("08:30"),
                BlueprintSlot::new_unchecked("recurrence", "weekdays", "Repeat on", Some(json!("weekdays")), WEEKDAY_PRESETS.iter().map(|(k, _)| *k).collect(), false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["learning".to_string(), "daily".to_string()],
        },
        AutomationBlueprint {
            key: "gratitude-journal".to_string(),
            title: "Gratitude & reflection prompt".to_string(),
            description: "A gentle evening prompt to reflect on the day and note what went well.".to_string(),
            category: "general".to_string(),
            schedule_template: "{minute} {hour} * * {dow}".to_string(),
            prompt_template: "Send the user a short, warm reflection prompt for the end of the day — invite them to note one thing that went well, one thing they are grateful for, and one small win. If they reply, acknowledge it kindly. One message.".to_string(),
            slots: vec![
                time_slot("21:30"),
                BlueprintSlot::new_unchecked("recurrence", "weekdays", "Repeat on", Some(json!("everyday")), WEEKDAY_PRESETS.iter().map(|(k, _)| *k).collect(), false, "", true),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["wellbeing".to_string(), "reflection".to_string()],
        },
        AutomationBlueprint {
            key: "on-this-day".to_string(),
            title: "On-this-day discovery".to_string(),
            description: "A daily dose of curiosity: a notable historical event, fact, or word for the day.".to_string(),
            category: "daily".to_string(),
            schedule_template: "{minute} {hour} * * *".to_string(),
            prompt_template: "Give the user one interesting '{flavor}' item for today — keep it short, surprising, and genuinely interesting. One or two sentences, no filler.".to_string(),
            slots: vec![
                BlueprintSlot::new_unchecked("flavor", "enum", "What kind?", Some(json!("on this day in history")), vec!["on this day in history", "word of the day", "science fact", "quote of the day"], false, "", true),
                time_slot("07:30"),
                deliver_slot(),
            ],
            deliver_default: "origin".to_string(),
            skills: vec![],
            tags: vec!["daily".to_string(), "curiosity".to_string()],
        },
    ]
}

static CATALOG_CACHE: OnceLock<Vec<AutomationBlueprint>> = OnceLock::new();

/// The curated set, mirroring Python `CATALOG: List[AutomationBlueprint]`.
pub fn catalog() -> &'static [AutomationBlueprint] {
    CATALOG_CACHE.get_or_init(build_catalog)
}

/// Owned copy of the catalog. Mirrors direct use of `CATALOG` in Python.
pub fn catalog_owned() -> Vec<AutomationBlueprint> {
    catalog().to_vec()
}

/// Alias matching the Python constant name for callers that prefer `CATALOG`.
pub fn get_catalog() -> &'static [AutomationBlueprint] {
    catalog()
}

// ---------------------------------------------------------------------------
// Lookup — mirrors `get_blueprint` / `_CATALOG_BY_KEY`
// ---------------------------------------------------------------------------

/// Return the blueprint with the given key, or `None`.
/// Mirrors `def get_blueprint(key: str) -> Optional[AutomationBlueprint]`.
pub fn get_blueprint(key: &str) -> Option<&'static AutomationBlueprint> {
    catalog().iter().find(|b| b.key == key)
}

// ---------------------------------------------------------------------------
// Helpers — value stringification
// ---------------------------------------------------------------------------

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => serde_json::to_string(&Value::Array(a.clone())).unwrap_or_default(),
        Value::Object(o) => serde_json::to_string(&Value::Object(o.clone())).unwrap_or_default(),
    }
}

fn is_empty_value(v: Option<&Value>) -> bool {
    match v {
        None => true,
        Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Renderers — mirrors `blueprint_form_schema`, `blueprint_slash_command`,
// `blueprint_deeplink`, `_humanize_schedule`, `blueprint_catalog_entry`
// ---------------------------------------------------------------------------

/// Emit the JSON a form renderer (dashboard / GUI) needs for this blueprint.
/// Mirrors `def blueprint_form_schema(blueprint: AutomationBlueprint) -> Dict[str, Any]`.
pub fn blueprint_form_schema(blueprint: &AutomationBlueprint) -> Value {
    json!({
        "key": blueprint.key,
        "title": blueprint.title,
        "description": blueprint.description,
        "category": blueprint.category,
        "tags": blueprint.tags,
        "fields": blueprint.slots.iter().map(|s| {
            json!({
                "name": s.name,
                "type": s.slot_type,
                "label": s.label,
                "default": s.default.clone().unwrap_or(Value::Null),
                "options": s.options,
                "optional": s.optional,
                "strict": s.strict,
                "help": s.help,
            })
        }).collect::<Vec<_>>(),
    })
}

/// Build the flattened ``/blueprint <key> slot=val …`` command string.
/// Uses each slot's default when `values` is omitted.
/// Mirrors `def blueprint_slash_command(blueprint, values=None) -> str`.
pub fn blueprint_slash_command(
    blueprint: &AutomationBlueprint,
    values: Option<&HashMap<String, Value>>,
) -> String {
    let mut parts = vec![format!("/blueprint {}", blueprint.key)];
    for s in &blueprint.slots {
        let val_opt: Option<Value> = values
            .and_then(|m| m.get(&s.name).cloned())
            .or_else(|| s.default.clone());
        // Mirrors: if val is None or val == "": if s.optional: continue; val = ""
        let is_none_or_empty = match &val_opt {
            None => true,
            Some(Value::Null) => true,
            Some(Value::String(st)) if st.is_empty() => true,
            _ => false,
        };
        if is_none_or_empty {
            if s.optional {
                continue;
            }
            // val stays as empty string
            let sval = String::new();
            // type text or space in sval: empty string not quoted unless type text? Python says `s.type == "text" or " " in sval` -> text true even for empty -> would quote
            let needs_quote = s.slot_type == "text" || sval.contains(' ');
            let final_sval = if needs_quote {
                format!("\"{}\"", sval.replace('"', "\\\""))
            } else {
                sval
            };
            parts.push(format!("{}={}", s.name, final_sval));
            continue;
        }
        let sval_raw = value_to_string(val_opt.as_ref().unwrap());
        let needs_quote = s.slot_type == "text" || sval_raw.contains(' ');
        let sval = if needs_quote {
            format!("\"{}\"", sval_raw.replace('"', "\\\""))
        } else {
            sval_raw
        };
        parts.push(format!("{}={}", s.name, sval));
    }
    parts.join(" ")
}

/// Convenience overload accepting `Option<&HashMap<String, String>>` for ergonomic calls.
pub fn blueprint_slash_command_str_values(
    blueprint: &AutomationBlueprint,
    values: Option<&HashMap<String, String>>,
) -> String {
    let converted: Option<HashMap<String, Value>> = values.map(|m| {
        m.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect()
    });
    blueprint_slash_command(blueprint, converted.as_ref())
}

// -- URL encoding helpers (mirrors urllib.parse.quote / urlencode) --

fn percent_encode_byte(b: u8) -> String {
    format!("%{b:02X}")
}

fn is_unreserved(b: u8) -> bool {
    matches!(b,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'
    )
}

/// Mirrors `urllib.parse.quote(s, safe='')` for path segment.
/// Keeps unreserved + '/' (Python default safe='/').
fn quote(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if is_unreserved(b) || b == b'/' {
            out.push(b as char);
        } else {
            out.push_str(&percent_encode_byte(b));
        }
    }
    out
}

/// Encode a query component using `quote_plus` semantics (space -> '+').
fn quote_plus(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if b == b' ' {
            out.push('+');
        } else if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&percent_encode_byte(b));
        }
    }
    out
}

fn urlencode(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Build the ``hermes://blueprint/<key>?slot=val`` deep-link URL.
/// Mirrors `def blueprint_deeplink(blueprint, values=None) -> str`.
pub fn blueprint_deeplink(
    blueprint: &AutomationBlueprint,
    values: Option<&HashMap<String, Value>>,
) -> String {
    let mut query: Vec<(String, String)> = Vec::new();
    for s in &blueprint.slots {
        let val_opt: Option<Value> = values
            .and_then(|m| m.get(&s.name).cloned())
            .or_else(|| s.default.clone());
        if let Some(ref v) = val_opt {
            let s_val = value_to_string(v);
            // Python: if val not in (None, ""):
            if !is_empty_value(Some(v)) {
                query.push((s.name.clone(), s_val));
            } else if let Value::String(st) = v {
                if !st.is_empty() {
                    query.push((s.name.clone(), value_to_string(v)));
                }
            } else if !matches!(v, Value::Null) {
                // non-string truthy values still included if not empty string
                // already handled by is_empty check above; keep if not empty
            }
        }
    }
    // De-duplicate check: Python includes value if val not in (None, "")
    // Our filter above matches: skip None/Null/empty-string.
    // Rebuild strictly to match Python's order (slot order).
    let mut strict_query: Vec<(String, String)> = Vec::new();
    for s in &blueprint.slots {
        let val_opt: Option<Value> = values
            .and_then(|m| m.get(&s.name).cloned())
            .or_else(|| s.default.clone());
        let include = match &val_opt {
            None => false,
            Some(Value::Null) => false,
            Some(Value::String(st)) if st.is_empty() => false,
            Some(_) => true,
        };
        if include {
            strict_query.push((s.name.clone(), value_to_string(val_opt.as_ref().unwrap())));
        }
    }
    let qs = if strict_query.is_empty() {
        String::new()
    } else {
        format!("?{}", urlencode(&strict_query))
    };
    format!("hermes://blueprint/{}{}", quote(&blueprint.key), qs)
}

/// Convenience overload for string-valued maps.
pub fn blueprint_deeplink_str_values(
    blueprint: &AutomationBlueprint,
    values: Option<&HashMap<String, String>>,
) -> String {
    let converted: Option<HashMap<String, Value>> = values.map(|m| {
        m.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect()
    });
    blueprint_deeplink(blueprint, converted.as_ref())
}

/// A short human-readable description of when a blueprint runs (defaults).
/// Mirrors `def _humanize_schedule(blueprint: AutomationBlueprint) -> str`.
pub fn humanize_schedule(blueprint: &AutomationBlueprint) -> String {
    let sched = &blueprint.schedule_template;
    if sched.starts_with("*/") {
        let iv = blueprint.slots.iter().find(|s| s.name == "interval_min");
        let every = iv
            .and_then(|s| s.default.as_ref().map(|v| value_to_string(v)))
            .filter(|s| !s.is_empty())
            .or_else(|| {
                // sched.split("/")[1].split()[0]
                sched.split('/').nth(1).and_then(|rest| {
                    rest.split_whitespace().next().map(|s| s.to_string())
                })
            })
            .unwrap_or_default();
        return format!("every {every} minutes");
    }
    if sched.contains("{interval_hours}") {
        let iv = blueprint.slots.iter().find(|s| s.name == "interval_hours");
        let every = iv
            .and_then(|s| s.default.as_ref().map(|v| value_to_string(v)))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "1".to_string());
        let scope = if sched.contains("* * 1-5") { "weekdays, " } else { "" };
        return if every == "1" {
            format!("{scope}every hour")
        } else {
            format!("{scope}every {every} hours")
        };
    }
    let time_slot = blueprint.slots.iter().find(|s| s.slot_type == "time");
    let when = time_slot
        .and_then(|s| s.default.as_ref().map(|v| value_to_string(v)))
        .filter(|s| !s.is_empty());
    if sched.contains("* * 1-5") {
        return match when {
            Some(w) => format!("weekdays at {w}"),
            None => "every weekday".to_string(),
        };
    }
    if sched.contains("{dow}") {
        let day_slot = blueprint
            .slots
            .iter()
            .find(|s| s.name == "day" || s.name == "recurrence");
        let scope = day_slot
            .and_then(|s| s.default.as_ref().map(|v| value_to_string(v)))
            .unwrap_or_default();
        if !scope.is_empty() {
            if let Some(ref w) = when {
                return format!("{scope} at {w}");
            }
        }
        return match when {
            Some(w) => format!("at {w}"),
            None => "on a schedule".to_string(),
        };
    }
    match when {
        Some(w) => format!("daily at {w}"),
        None => "on a schedule".to_string(),
    }
}

/// Unified serializable shape for a blueprint — used by the docs generator
/// and the dashboard API. Combines the form schema, the ready-to-paste slash
/// command, the deep-link URL, and a human-readable schedule.
/// Mirrors `def blueprint_catalog_entry(blueprint: AutomationBlueprint) -> Dict[str, Any]`.
pub fn blueprint_catalog_entry(blueprint: &AutomationBlueprint) -> Value {
    let mut obj = match blueprint_form_schema(blueprint) {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    obj.insert(
        "schedule".to_string(),
        Value::String(blueprint.schedule_template.clone()),
    );
    obj.insert(
        "scheduleHuman".to_string(),
        Value::String(humanize_schedule(blueprint)),
    );
    obj.insert(
        "command".to_string(),
        Value::String(blueprint_slash_command(blueprint, None)),
    );
    obj.insert(
        "appUrl".to_string(),
        Value::String(blueprint_deeplink(blueprint, None)),
    );
    Value::Object(obj)
}

// ---------------------------------------------------------------------------
// Fill + validate + translate to a create_job spec
// ---------------------------------------------------------------------------

fn parse_time_hhmm(s: &str) -> Option<(u32, u32)> {
    let t = s.trim();
    let colon = t.find(':')?;
    let (h_str, m_str) = t.split_at(colon);
    let m_str = &m_str[1..];
    if h_str.is_empty() || m_str.is_empty() {
        return None;
    }
    // Reject extra colon
    if m_str.contains(':') {
        return None;
    }
    // Hours 0-23, minutes 0-59
    let hour: u32 = h_str.parse().ok()?;
    let minute: u32 = m_str.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    // Also enforce digit-only (parse already ensures) and handle leading zeros
    // Ensure original strings are digits only
    if !h_str.chars().all(|c| c.is_ascii_digit()) || !m_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // Enforce hour length: "0"-"23" or "00"-"23" (1-2 digits). Already covered.
    // Python regex ^([01]?\d|2[0-3]):([0-5]\d)$ allows e.g. "8:00" or "08:00" but not "008:00"
    // We add length guard: h_str len must be 1-2, m_str len must be 2? Actually regex allows m [0-5]\d => exactly 2 digits.
    // But Python's int conversion later would handle "8:0"? No, would fail regex for "8:0".
    // So enforce m_str len ==2.
    if m_str.len() != 2 {
        return None;
    }
    if h_str.len() > 2 {
        return None;
    }
    Some((hour, minute))
}

fn extract_placeholders(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = template[i..].find('}') {
                let inner = &template[i + 1..i + end];
                if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.push(inner.to_string());
                }
                i += end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn resolve_schedule(
    blueprint: &AutomationBlueprint,
    values: &HashMap<String, Value>,
) -> Result<String, BlueprintFillError> {
    let sched = blueprint.schedule_template.clone();

    // A free-text `schedule` slot passes through verbatim (full flexibility).
    if let Some(v) = values.get("schedule") {
        let s = value_to_string(v);
        if !s.is_empty() {
            return Ok(s);
        }
    }

    let mut repl: HashMap<String, String> = HashMap::new();

    // time -> minute/hour
    if sched.contains("{minute}") || sched.contains("{hour}") {
        let time_val = values.get("time");
        let time_str = match time_val {
            Some(v) => value_to_string(v),
            None => String::new(),
        };
        if time_str.trim().is_empty() {
            return Err(BlueprintFillError::new("a time is required"));
        }
        let trimmed = time_str.trim().to_string();
        match parse_time_hhmm(&trimmed) {
            None => {
                return Err(BlueprintFillError::new(format!(
                    "invalid time {time_str:?} — use HH:MM (24h)"
                )))
            }
            Some((h, m)) => {
                repl.insert("hour".to_string(), h.to_string());
                repl.insert("minute".to_string(), m.to_string());
            }
        }
    }

    // weekday set -> dow
    if sched.contains("{dow}") {
        if values.contains_key("recurrence") {
            let preset_raw = values
                .get("recurrence")
                .map(|v| value_to_string(v).to_lowercase())
                .unwrap_or_else(|| "everyday".to_string());
            let map = weekday_preset_map();
            if let Some(v) = map.get(&preset_raw) {
                repl.insert("dow".to_string(), v.clone());
            } else {
                return Err(BlueprintFillError::new(format!(
                    "unknown recurrence {preset_raw:?} — one of {}",
                    WEEKDAY_PRESETS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
                )));
            }
        } else if values.contains_key("day") {
            let day_raw = values
                .get("day")
                .map(|v| value_to_string(v).to_lowercase())
                .unwrap_or_default();
            let map = day_to_dow_map();
            if let Some(v) = map.get(&day_raw) {
                repl.insert("dow".to_string(), v.clone());
            } else {
                return Err(BlueprintFillError::new(format!(
                    "unknown day {day_raw:?}"
                )));
            }
        } else {
            repl.insert("dow".to_string(), "*".to_string());
        }
    }

    // interval (minutes) for */N schedules
    if sched.contains("{interval_min}") {
        let iv = values
            .get("interval_min")
            .map(|v| value_to_string(v).trim().to_string())
            .unwrap_or_default();
        let is_digits = !iv.is_empty() && iv.chars().all(|c| c.is_ascii_digit());
        let valid = is_digits && iv.parse::<i64>().map(|n| n > 0).unwrap_or(false);
        if !valid {
            return Err(BlueprintFillError::new(format!(
                "invalid interval {iv:?} — minutes as a positive integer"
            )));
        }
        repl.insert("interval_min".to_string(), iv);
    }

    // Any remaining {slot} placeholders are filled verbatim from validated
    // enum/text slot values (e.g. an hour-range window).
    let placeholders = extract_placeholders(&sched);
    for name in placeholders {
        if !repl.contains_key(&name) {
            if let Some(v) = values.get(&name) {
                repl.insert(name.clone(), value_to_string(v));
            }
        }
    }

    // Apply formatting — mirrors `sched.format(**repl)`
    let mut result = sched;
    // First check missing keys (dev error)
    for ph in extract_placeholders(&result.clone()) {
        if !repl.contains_key(&ph) {
            return Err(BlueprintFillError::new(format!(
                "schedule template missing value for '{ph}'"
            )));
        }
    }
    for (k, v) in &repl {
        let placeholder = format!("{{{k}}}");
        result = result.replace(&placeholder, v);
    }
    Ok(result)
}

/// Validate `values` and return `cron.jobs.create_job` kwargs.
///
/// Missing required (non-optional) slots raise `BlueprintFillError` naming the
/// slot, so a form can show field errors and the agent knows what to ask.
/// Unknown slot names are rejected. Enum values are checked against their
/// options. The result is passed straight to `create_job` — no second schema.
///
/// Mirrors `def fill_blueprint(blueprint, values, *, origin=None) -> Dict[str, Any]`.
pub fn fill_blueprint(
    blueprint: &AutomationBlueprint,
    values: &HashMap<String, Value>,
    origin: Option<Value>,
) -> Result<Value, BlueprintFillError> {
    let known: HashSet<String> = blueprint.slots.iter().map(|s| s.name.clone()).collect();
    let unknown: Vec<String> = {
        let mut u: Vec<String> = values
            .keys()
            .filter(|k| !known.contains(*k))
            .cloned()
            .collect();
        u.sort();
        u
    };
    if !unknown.is_empty() {
        let plural = if unknown.len() > 1 { "s" } else { "" };
        return Err(BlueprintFillError::new(format!(
            "unknown slot{plural}: {} — valid: {}",
            unknown.join(", "),
            blueprint.slots.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ")
        )));
    }

    let mut resolved: HashMap<String, Value> = HashMap::new();
    for s in &blueprint.slots {
        let raw_opt = values.get(&s.name).cloned().or_else(|| s.default.clone());
        let is_empty = match &raw_opt {
            None => true,
            Some(Value::Null) => true,
            Some(Value::String(st)) if st.is_empty() => true,
            _ => false,
        };
        if is_empty {
            if s.optional {
                continue;
            }
            return Err(BlueprintFillError::new(format!(
                "missing required value: {} ({})",
                s.name, s.label
            )));
        }
        let raw = raw_opt.unwrap();
        if s.slot_type == "enum" && s.strict && !s.options.is_empty() {
            let raw_str = value_to_string(&raw);
            let allowed: HashSet<String> = s.options.iter().cloned().collect();
            if !allowed.contains(&raw_str) {
                return Err(BlueprintFillError::new(format!(
                    "{}={raw_str:?} not allowed — one of {}",
                    s.name,
                    s.options.join(", ")
                )));
            }
        }
        resolved.insert(s.name.clone(), raw);
    }

    let schedule = resolve_schedule(blueprint, &resolved)?;

    // Render the prompt with whatever slots it references.
    let mut prompt = blueprint.prompt_template.clone();
    // Validate all placeholders in prompt exist in resolved
    for ph in extract_placeholders(&prompt.clone()) {
        if !resolved.contains_key(&ph) {
            return Err(BlueprintFillError::new(format!(
                "blueprint prompt missing value for '{ph}'"
            )));
        }
    }
    for (k, v) in &resolved {
        let placeholder = format!("{{{k}}}");
        if prompt.contains(&placeholder) {
            prompt = prompt.replace(&placeholder, &value_to_string(v));
        }
    }

    let mut spec = json!({
        "prompt": prompt,
        "schedule": schedule,
        "name": blueprint.title,
        "deliver": resolved.get("deliver").map(|v| value_to_string(v)).unwrap_or_else(|| blueprint.deliver_default.clone()),
    });

    if !blueprint.skills.is_empty() {
        spec["skills"] = json!(blueprint.skills);
    }
    if let Some(o) = origin {
        spec["origin"] = o;
    }
    Ok(spec)
}

/// Overload for `HashMap<String, String>` callers (common ergonomic path).
pub fn fill_blueprint_str_values(
    blueprint: &AutomationBlueprint,
    values: &HashMap<String, String>,
    origin: Option<Value>,
) -> Result<Value, BlueprintFillError> {
    let converted: HashMap<String, Value> = values
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    fill_blueprint(blueprint, &converted, origin)
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use serde_json::json;

    #[test]
    fn slot_type_validation() {
        assert!(BlueprintSlot::new("x", "time", "lbl", None, vec![], false, "", true).is_ok());
        assert!(BlueprintSlot::new("x", "bogus", "lbl", None, vec![], false, "", true).is_err());
    }

    #[test]
    fn catalog_has_sixteen_entries() {
        assert_eq!(catalog().len(), 16);
        assert_eq!(catalog_owned().len(), 16);
        assert_eq!(get_catalog().len(), 16);
    }

    #[test]
    fn catalog_keys_match_python() {
        let keys: Vec<&str> = catalog().iter().map(|b| b.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "morning-brief",
                "important-mail",
                "weekly-review",
                "workday-start",
                "custom-reminder",
                "evening-winddown",
                "news-digest",
                "bill-renewal-watch",
                "price-watch",
                "competitor-watch",
                "habit-checkin",
                "hydration-move",
                "meal-plan",
                "learn-daily",
                "gratitude-journal",
                "on-this-day",
            ]
        );
    }

    #[test]
    fn get_blueprint_works() {
        assert!(get_blueprint("morning-brief").is_some());
        assert_eq!(get_blueprint("morning-brief").unwrap().title, "Morning briefing");
        assert!(get_blueprint("does-not-exist").is_none());
    }

    #[test]
    fn form_schema_shape() {
        let bp = get_blueprint("morning-brief").unwrap();
        let schema = blueprint_form_schema(bp);
        assert_eq!(schema["key"], "morning-brief");
        assert_eq!(schema["fields"].as_array().unwrap().len(), 2);
        assert_eq!(schema["fields"][0]["name"], "time");
        assert_eq!(schema["fields"][0]["type"], "time");
        assert_eq!(schema["fields"][1]["name"], "deliver");
    }

    #[test]
    fn slash_command_defaults() {
        let bp = get_blueprint("morning-brief").unwrap();
        let cmd = blueprint_slash_command(bp, None);
        // Should contain /blueprint morning-brief time=... deliver=...
        assert!(cmd.starts_with("/blueprint morning-brief"));
        assert!(cmd.contains("time=08:00"));
        assert!(cmd.contains("deliver=origin"));
    }

    #[test]
    fn slash_command_quotes_text() {
        let bp = get_blueprint("custom-reminder").unwrap();
        // text slot "what" should be quoted
        let cmd = blueprint_slash_command(bp, None);
        assert!(cmd.contains("what=\"take a break and stretch\""));
    }

    #[test]
    fn slash_command_with_overrides() {
        let bp = get_blueprint("important-mail").unwrap();
        let mut vals = HashMap::new();
        vals.insert("interval_min".to_string(), json!("15"));
        vals.insert("criteria".to_string(), json!("urgent only"));
        let cmd = blueprint_slash_command(bp, Some(&vals));
        assert!(cmd.contains("interval_min=15"));
        assert!(cmd.contains("criteria=\"urgent only\""));
    }

    #[test]
    fn deeplink_shape() {
        let bp = get_blueprint("morning-brief").unwrap();
        let url = blueprint_deeplink(bp, None);
        assert!(url.starts_with("hermes://blueprint/morning-brief"));
        assert!(url.contains("time="));
        assert!(url.contains("deliver="));
    }

    #[test]
    fn humanize_schedule_variants() {
        assert_eq!(humanize_schedule(get_blueprint("morning-brief").unwrap()), "daily at 08:00");
        assert_eq!(humanize_schedule(get_blueprint("workday-start").unwrap()), "weekdays at 09:00");
        assert_eq!(humanize_schedule(get_blueprint("important-mail").unwrap()), "every 30 minutes");
        assert_eq!(humanize_schedule(get_blueprint("hydration-move").unwrap()), "weekdays, every hour");
        assert_eq!(humanize_schedule(get_blueprint("on-this-day").unwrap()), "daily at 07:30");
        // weekly-review has dow + time => "sunday at 18:00"
        assert_eq!(humanize_schedule(get_blueprint("weekly-review").unwrap()), "sunday at 18:00");
        // custom-reminder has dow => "everyday at 14:00"
        assert_eq!(humanize_schedule(get_blueprint("custom-reminder").unwrap()), "everyday at 14:00");
    }

    #[test]
    fn catalog_entry_has_required_keys() {
        let bp = get_blueprint("workday-start").unwrap();
        let entry = blueprint_catalog_entry(bp);
        assert_eq!(entry["key"], "workday-start");
        assert_eq!(entry["schedule"], "{minute} {hour} * * 1-5");
        assert!(entry.get("scheduleHuman").is_some());
        assert!(entry.get("command").is_some());
        assert!(entry.get("appUrl").is_some());
        assert!(entry.get("fields").is_some());
    }

    #[test]
    fn fill_blueprint_validates_time() {
        let bp = get_blueprint("morning-brief").unwrap();
        let mut vals = HashMap::new();
        vals.insert("time".to_string(), json!("08:00"));
        vals.insert("deliver".to_string(), json!("origin"));
        let spec = fill_blueprint(bp, &vals, None).unwrap();
        assert_eq!(spec["schedule"], "0 8 * * *");
        assert_eq!(spec["deliver"], "origin");
        assert_eq!(spec["name"], "Morning briefing");
        // skills present
        assert_eq!(spec["skills"][0], "google-workspace");
    }

    #[test]
    fn fill_blueprint_rejects_bad_time() {
        let bp = get_blueprint("morning-brief").unwrap();
        let mut vals = HashMap::new();
        vals.insert("time".to_string(), json!("25:00"));
        vals.insert("deliver".to_string(), json!("origin"));
        let err = fill_blueprint(bp, &vals, None).unwrap_err();
        assert!(err.0.contains("invalid time"), "got {err:?}");
    }

    #[test]
    fn fill_blueprint_rejects_unknown_slot() {
        let bp = get_blueprint("morning-brief").unwrap();
        let mut vals = HashMap::new();
        vals.insert("tiem".to_string(), json!("08:00"));
        let err = fill_blueprint(bp, &vals, None).unwrap_err();
        assert!(err.0.contains("unknown slot"), "got {err:?}");
    }

    #[test]
    fn fill_blueprint_rejects_bad_enum() {
        let bp = get_blueprint("important-mail").unwrap();
        let mut vals = HashMap::new();
        vals.insert("interval_min".to_string(), json!("99"));
        vals.insert("criteria".to_string(), json!("x"));
        vals.insert("deliver".to_string(), json!("origin"));
        let err = fill_blueprint(bp, &vals, None).unwrap_err();
        assert!(err.0.contains("not allowed"), "got {err:?}");
    }

    #[test]
    fn fill_blueprint_strict_false_allows_any_deliver() {
        let bp = get_blueprint("morning-brief").unwrap();
        let mut vals = HashMap::new();
        vals.insert("time".to_string(), json!("09:30"));
        vals.insert("deliver".to_string(), json!("my-custom-platform"));
        let spec = fill_blueprint(bp, &vals, None).unwrap();
        assert_eq!(spec["deliver"], "my-custom-platform");
    }

    #[test]
    fn fill_blueprint_resolves_dow_presets() {
        let bp = get_blueprint("custom-reminder").unwrap();
        let mut vals = HashMap::new();
        vals.insert("what".to_string(), json!("test"));
        vals.insert("time".to_string(), json!("14:00"));
        vals.insert("recurrence".to_string(), json!("weekends"));
        vals.insert("deliver".to_string(), json!("origin"));
        let spec = fill_blueprint(bp, &vals, None).unwrap();
        assert_eq!(spec["schedule"], "0 14 * * 0,6");
    }

    #[test]
    fn fill_blueprint_resolves_day() {
        let bp = get_blueprint("weekly-review").unwrap();
        let mut vals = HashMap::new();
        vals.insert("time".to_string(), json!("18:00"));
        vals.insert("day".to_string(), json!("friday"));
        vals.insert("deliver".to_string(), json!("origin"));
        let spec = fill_blueprint(bp, &vals, None).unwrap();
        assert_eq!(spec["schedule"], "0 18 * * 5");
    }

    #[test]
    fn fill_blueprint_interval_template() {
        let bp = get_blueprint("important-mail").unwrap();
        let mut vals = HashMap::new();
        vals.insert("interval_min".to_string(), json!("15"));
        vals.insert("criteria".to_string(), json!("urgent"));
        vals.insert("deliver".to_string(), json!("origin"));
        let spec = fill_blueprint(bp, &vals, None).unwrap();
        assert_eq!(spec["schedule"], "*/15 * * * *");
        assert!(spec["prompt"].as_str().unwrap().contains("urgent"));
    }

    #[test]
    fn fill_blueprint_with_origin() {
        let bp = get_blueprint("morning-brief").unwrap();
        let mut vals = HashMap::new();
        vals.insert("time".to_string(), json!("08:00"));
        vals.insert("deliver".to_string(), json!("origin"));
        let spec = fill_blueprint(bp, &vals, Some(json!({"platform":"cli"}))).unwrap();
        assert_eq!(spec["origin"]["platform"], "cli");
    }

    #[test]
    fn fill_blueprint_hydration_move_remaining_placeholders() {
        let bp = get_blueprint("hydration-move").unwrap();
        let mut vals = HashMap::new();
        vals.insert("interval_hours".to_string(), json!("2"));
        vals.insert("start_hour".to_string(), json!("8"));
        vals.insert("end_hour".to_string(), json!("17"));
        vals.insert("deliver".to_string(), json!("origin"));
        let spec = fill_blueprint(bp, &vals, None).unwrap();
        assert_eq!(spec["schedule"], "0 8-17/2 * * 1-5");
    }
}
