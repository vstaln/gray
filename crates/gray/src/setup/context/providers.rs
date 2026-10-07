//! Provider models, context caches, rates (split from `context`).

use super::*;

/// Converts a raw model ID to a friendly human-readable display name.
pub fn friendly_model_name(model_id: &str) -> String {
    if model_id.is_empty() {
        return String::new();
    }
    let name = model_id.split('/').next_back().unwrap_or(model_id);
    let words: Vec<String> = name
        .split(['-', '_', ':'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_lowercase();
            if lower == "gpt" || lower == "glm" || lower == "ai" || lower == "api" {
                w.to_uppercase()
            } else if lower.starts_with('v')
                && lower.len() > 1
                && lower[1..].chars().all(|c| c.is_ascii_digit() || c == '.')
            {
                format!("v{}", &lower[1..])
            } else {
                let mut c = w.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().chain(c).collect(),
                }
            }
        })
        .collect();
    // The split above destroys version dots: `claude-opus-5-5` was `5.5`
    // upstream, never `5 5`. Merge a lone digit onto a word ending in a
    // digit (`v3-1` -> `v3.1`, `5-5` -> `5.5`); longer tokens (`2025`,
    // `0324`, `30b`) stay separate so dates and size stamps survive.
    let mut merged: Vec<String> = Vec::with_capacity(words.len());
    for w in words {
        let lone_digit = w.len() == 1 && w.bytes().next().is_some_and(|b| b.is_ascii_digit());
        if lone_digit
            && merged
                .last()
                .is_some_and(|prev| prev.ends_with(|c: char| c.is_ascii_digit()))
        {
            merged.last_mut().unwrap().push('.');
            merged.last_mut().unwrap().push_str(&w);
        } else {
            merged.push(w);
        }
    }
    merged.join(" ")
}

/// Whether a model can reason, keyed like the context cache (exact id +
/// lowercase + `provider/` tail). Populated from models.dev's `reasoning`
/// flag and live `/models` payloads (`supported_parameters` on OpenRouter,
/// `reasoning` bool elsewhere) — the same sources opencode's
/// `capabilities.reasoning` comes from. `None` = provider never said.
static MODEL_REASONING: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, bool>>,
> = std::sync::OnceLock::new();

fn model_reasoning_cell() -> &'static std::sync::RwLock<std::collections::HashMap<String, bool>> {
    MODEL_REASONING.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub fn cache_model_reasoning(model_id: &str, reasoning: bool) {
    if let Ok(mut g) = model_reasoning_cell().write() {
        g.insert(model_id.to_string(), reasoning);
        let lower = model_id.to_lowercase();
        if lower != model_id {
            g.insert(lower, reasoning);
        }
        if let Some((_, suffix)) = model_id.rsplit_once('/') {
            g.insert(suffix.to_string(), reasoning);
            g.insert(suffix.to_lowercase(), reasoning);
        }
    }
}

/// Per-model effort values from models.dev `reasoning_options` (the automatic
/// source opencode derives variants from via `reasoningVariants`). Same keying
/// as the reasoning cache. `None` = no source has spoken for this model.
static MODEL_EFFORTS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, Vec<String>>>,
> = std::sync::OnceLock::new();

fn model_efforts_cell() -> &'static std::sync::RwLock<std::collections::HashMap<String, Vec<String>>>
{
    MODEL_EFFORTS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub(crate) fn cache_model_efforts(model_id: &str, efforts: Vec<String>) {
    if let Ok(mut g) = model_efforts_cell().write() {
        // Exact key as the provider names it: authoritative, always wins.
        g.insert(model_id.to_string(), efforts.clone());
        let lower = model_id.to_lowercase();
        if lower != model_id {
            g.insert(lower, efforts.clone());
        }
        // Suffix alias (`provider/model` -> `model`): gap-fill only. A
        // qualified entry must not clobber another provider's exact bare id:
        // kilo/openrouter list `meta/muse-spark-1.3-contributor` WITH `max`
        // while bare-id providers list the contributor id WITHOUT it (the
        // provider 400-rejects `max`) — last-writer-wins here leaked `max`
        // into the bare id's picker rows.
        if let Some((_, suffix)) = model_id.rsplit_once('/') {
            g.entry(suffix.to_string()).or_insert(efforts.clone());
            g.entry(suffix.to_lowercase()).or_insert(efforts);
        }
    }
}

fn model_efforts(model_id: &str) -> Option<Vec<String>> {
    let g = model_efforts_cell().read().ok()?;
    if let Some(v) = g.get(model_id).cloned() {
        return Some(v);
    }
    let lower = model_id.to_lowercase();
    if let Some(v) = g.get(&lower).cloned() {
        return Some(v);
    }
    if let Some((_, suffix)) = model_id.rsplit_once('/') {
        if let Some(v) = g.get(suffix).cloned() {
            return Some(v);
        }
        if let Some(v) = g.get(&suffix.to_lowercase()).cloned() {
            return Some(v);
        }
    }
    None
}

/// Capability/shape facts the recommended-model ranking needs, keyed like
/// the reasoning cache (exact id + lowercase + `provider/` tail). models.dev
/// is the authoritative source; live `/models` payloads gap-fill fields it
/// never spoke for. `None` fields = no source has spoken for that fact.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelMeta {
    pub tool_call: Option<bool>,
    /// Output is text-only (`modalities.output == ["text"]`).
    pub text_only: Option<bool>,
    /// `YYYY-MM-DD`; lexicographic order is chronological.
    pub release_date: Option<String>,
    pub family: Option<String>,
}

static MODEL_META: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, ModelMeta>>,
> = std::sync::OnceLock::new();

fn model_meta_cell() -> &'static std::sync::RwLock<std::collections::HashMap<String, ModelMeta>> {
    MODEL_META.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// models.dev write: its statement replaces whatever is cached (it is the
/// authoritative source, and it speaks for every field on every model).
pub fn cache_model_meta(model_id: &str, meta: ModelMeta) {
    if let Ok(mut g) = model_meta_cell().write() {
        g.insert(model_id.to_string(), meta);
    }
}

/// Live-payload write: fills only fields no source has spoken for —
/// models.dev values always win regardless of arrival order.
pub(crate) fn cache_model_meta_gap_fill(model_id: &str, meta: ModelMeta) {
    if let Ok(mut g) = model_meta_cell().write() {
        let entry = g.entry(model_id.to_string()).or_default();
        if entry.tool_call.is_none() {
            entry.tool_call = meta.tool_call;
        }
        if entry.text_only.is_none() {
            entry.text_only = meta.text_only;
        }
        if entry.release_date.is_none() {
            entry.release_date = meta.release_date;
        }
        if entry.family.is_none() {
            entry.family = meta.family;
        }
    }
}

/// What any source has said about `model_id`, resolved like the context
/// cache: exact, lowercase, then `provider/` tail.
pub fn model_meta(model_id: &str) -> Option<ModelMeta> {
    let g = model_meta_cell().read().ok()?;
    if let Some(m) = g.get(model_id).cloned() {
        return Some(m);
    }
    let lower = model_id.to_lowercase();
    if let Some(m) = g.get(&lower).cloned() {
        return Some(m);
    }
    if let Some((_, suffix)) = model_id.rsplit_once('/') {
        if let Some(m) = g.get(suffix).cloned() {
            return Some(m);
        }
        if let Some(m) = g.get(&suffix.to_lowercase()).cloned() {
            return Some(m);
        }
    }
    None
}

/// `Some(true/false)` when a provider source advertised reasoning support,
/// `None` when no source has spoken for this model.
pub fn model_supports_reasoning(model_id: &str) -> Option<bool> {
    let g = model_reasoning_cell().read().ok()?;
    if let Some(v) = g.get(model_id).copied() {
        return Some(v);
    }
    let lower = model_id.to_lowercase();
    if let Some(v) = g.get(&lower).copied() {
        return Some(v);
    }
    if let Some((_, suffix)) = model_id.rsplit_once('/') {
        if let Some(v) = g.get(suffix).copied() {
            return Some(v);
        }
        if let Some(v) = g.get(&suffix.to_lowercase()).copied() {
            return Some(v);
        }
    }
    None
}

/// Effort tiers a model family actually accepts. Automatic source first:
/// models.dev `reasoning_options` effort values cached by
/// `parse_models_dev_json` (same derivation as opencode's `reasoningVariants`;
/// `null` means `none`). Family tables below are the fallback for models
/// with no cached options. Each branch cites the provider's own API docs —
/// that is the source of truth; opencode is only the shape reference. Gray
/// `off` covers OpenAI `none` (omit reasoning). `None` = family unknown
/// (offer the full catalog); empty = no reasoning.
pub fn supported_efforts(model_id: &str) -> Option<Vec<&'static str>> {
    // CommandCode's Settings docs list `reasoningEffort` values as
    // low/medium/high/xhigh/max (https://api.commandcode.ai/docs/settings).
    // models.dev has no CommandCode rows, and its qualified
    // `xiaomi/mimo-v2.6-pro` rows from other gateways only say high; those
    // cached rows clamped a real xhigh request to high. The provider-specific
    // MiMo v2.6 Pro tier must win over the shared cache.
    let active = active_provider_base_url();
    if active.contains("commandcode.ai") && is_mimo_v26_pro(model_id) {
        return Some(vec!["off", "low", "medium", "high", "xhigh", "max"]);
    }
    if let Some(cached) = model_efforts(model_id) {
        let levels = super::super::THINKING_LEVELS;
        let want: Vec<&'static str> = cached
            .iter()
            .filter_map(|v| {
                let low = v.to_lowercase();
                let label = if low == "none" { "off" } else { low.as_str() };
                levels.iter().find(|(l, _)| *l == label).map(|(l, _)| *l)
            })
            .collect();
        if !want.is_empty() {
            return Some(want);
        }
    }
    let id = model_id.to_lowercase();
    // Plugin-relay providers (loopback base) may serve effort-in-id
    // catalogs: `swe-2-max`, `claude-opus-5-5-low-fast` bake the tier into
    // the name — there is no runtime knob to offer. Declared efforts won
    // above; this catches stale saved picks and hand-typed ids. Real API
    // endpoints are unaffected (the rule never runs for them).
    if is_loopback_base(&active_provider_base_url()) {
        let leaf = id.rsplit('/').next().unwrap_or(id.as_str());
        let core = leaf
            .strip_suffix("-fast")
            .or_else(|| leaf.strip_suffix("-priority"))
            .unwrap_or(leaf);
        let seg = core.rsplit('-').next().unwrap_or("");
        if seg == "none" || super::super::THINKING_LEVELS.iter().any(|(l, _)| *l == seg) {
            return Some(vec![]);
        }
    }
    // OpenAI: values are model-dependent —
    // https://developers.openai.com/api/docs/guides/reasoning
    // (`none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`).
    if id.contains("deep-research") {
        return Some(vec!["medium"]);
    }
    if id.contains("gpt")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.contains("deep-research")
    {
        // Chat-only GPT-5 variant takes a single effort.
        if id.contains("-chat") {
            return Some(vec!["medium"]);
        }
        // Unversioned gpt-5-pro takes `high` only; versioned gpt-5.x-pro
        // takes medium/high/xhigh.
        if id.contains("pro") {
            if ["5.1", "5.2", "5.3", "5.4", "5.5", "5.6"]
                .iter()
                .any(|v| id.contains(v))
            {
                return Some(vec!["medium", "high", "xhigh"]);
            }
            return Some(vec!["high"]);
        }
        // Codex variants: gpt-5.3+ takes none/low/medium/high/xhigh;
        // codex-max and gpt-5.2 codex take low/medium/high/xhigh.
        if id.contains("codex") {
            if id.contains("codex-max")
                || ["5.2", "5.3", "5.4", "5.5", "5.6"]
                    .iter()
                    .any(|v| id.contains(v))
            {
                let mut efforts = vec!["low", "medium", "high", "xhigh"];
                if !id.contains("codex-max") && !id.contains("5.2") {
                    efforts.insert(0, "off");
                }
                return Some(efforts);
            }
            return Some(vec!["low", "medium", "high"]);
        }
        // GPT-5.6 adds `max` (OpenAI GPT-5.6 model pages list it distinctly
        // from `xhigh`).
        if id.contains("5.6") {
            return Some(vec!["off", "low", "medium", "high", "xhigh", "max"]);
        }
        // GPT-5.2–5.5 add `xhigh`; GPT-5.1 replaced `minimal` with `none`.
        if ["5.2", "5.3", "5.4", "5.5"].iter().any(|v| id.contains(v)) {
            return Some(vec!["off", "low", "medium", "high", "xhigh"]);
        }
        if id.contains("5.1") {
            return Some(vec!["off", "low", "medium", "high"]);
        }
        // Unversioned gpt-5 keeps `minimal`.
        if id.contains("gpt-5") {
            return Some(vec!["off", "minimal", "low", "medium", "high"]);
        }
        // Unknown or future GPT-family models (e.g. gpt-6-astra): opencode's
        // generic path offers none/low/medium/high/xhigh to post-2025-12-04
        // models; mirror that — a model rejecting xhigh 400s and the
        // strip-and-retry path drops reasoning.
        return Some(vec!["off", "low", "medium", "high", "xhigh"]);
    }
    // Anthropic: availability is per-model —
    // https://platform.claude.com/docs/en/build-with-claude/effort
    // Modern models (Opus 4.7+, Sonnet 5, Fable, Mythos) take
    // low/medium/high/xhigh/max; the 4.6 generation and older take
    // low/medium/high/max (no xhigh).
    if id.contains("claude") || id.contains("anthropic") {
        if id.contains("4.7")
            || id.contains("4-7")
            || id.contains("4.8")
            || id.contains("4-8")
            || id.contains("sonnet-5")
            || id.contains("sonnet_5")
            || id.contains("opus-5")
            || id.contains("opus_5")
            || id.contains("fable")
            || id.contains("mythos")
        {
            return Some(vec!["low", "medium", "high", "xhigh", "max"]);
        }
        return Some(vec!["low", "medium", "high", "max"]);
    }
    // Gemini: thinking levels per model —
    // https://ai.google.dev/gemini-api/docs/thinking
    if id.contains("gemini") || id.contains("gemma") {
        // Pro image models reason at one level; flash image models take
        // minimal/high; Gemini 3 flash takes minimal/low/medium/high.
        if id.contains("pro-image") || id.contains("pro_image") {
            return Some(vec!["high"]);
        }
        if id.contains("flash-image") || id.contains("flash_image") {
            return Some(vec!["minimal", "high"]);
        }
        if id.contains("gemini-3") || id.contains("gemini_3") {
            return Some(vec!["minimal", "low", "medium", "high"]);
        }
        return Some(vec!["low", "high"]);
    }
    // xAI: documented `--effort` levels (no off/minimal/xhigh on 4.5) —
    // https://docs.x.ai/docs/guides/reasoning#control-how-hard-the-model-thinks
    if id.contains("grok") || id.contains("xai") {
        if id.contains("4.6") || id.contains("4-6") {
            return Some(vec!["low", "medium", "high", "xhigh"]);
        }
        if id.contains("mini") {
            return Some(vec!["low", "high"]);
        }
        return Some(vec!["low", "medium", "high"]);
    }
    // Kimi's Anthropic-compatible transports implement adaptive thinking
    // effort (opencode maps the family to low/medium/high/xhigh/max).
    if id.contains("kimi") || id.contains("k2p") || id.contains("moonshot") {
        return Some(vec!["low", "medium", "high", "xhigh", "max"]);
    }
    if id.contains("deepseek") && id.contains("reasoner") {
        return Some(vec!["low", "medium", "high"]);
    }
    // DeepSeek v4 on OpenAI-compatible transports additionally accepts max.
    if id.contains("deepseek-v4") || id.contains("deepseek_v4") {
        return Some(vec!["low", "medium", "high", "max"]);
    }
    // Muse Spark / Glimmer: effort values per models.dev reasoning_options —
    // [minimal, low, medium, high, xhigh] (no `max`; the provider 400-rejects it).
    if id.contains("muse") || id.contains("spark") || id.contains("glimmer") {
        return Some(vec!["minimal", "low", "medium", "high", "xhigh"]);
    }
    None
}

/// CommandCode's MiMo v2.6 Pro family (`-pro` and its UltraSpeed tier).
fn is_mimo_v26_pro(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    id.contains("mimo") && (id.contains("v2.6-pro") || id.contains("v2.6_pro"))
}

/// The step family: StepFun's API always reasons. `thinking: {"type":
/// "disabled"}` is ignored — measured against api.stepfun.ai, 1088
/// reasoning deltas still streamed at effort=off — and its `/models`
/// advertises `reasoning: false`, so both the disable flag and the
/// advertised metadata are lies this family has to override.
pub fn step_family(model_id: &str) -> bool {
    let id = model_id.rsplit('/').next().unwrap_or(model_id);
    id.starts_with("step-") || id.starts_with("step_")
}

/// Whether `off` really stops reasoning on this model. A family that
/// ignores the disable flag (the step family, measured) must not be
/// offered `off — No reasoning`: the row would be a lie, and selecting it
/// hides text the provider keeps streaming and keeps billing.
pub fn reasoning_off_is_honest(model_id: &str) -> bool {
    !step_family(model_id)
}

/// The levels the step family accepts — there is no `off` to offer.
pub const STEP_LEVELS: &[(&str, &str)] = &[
    ("low", "Light reasoning"),
    ("medium", "Moderate reasoning"),
    ("high", "Deep reasoning"),
    ("max", "Maximum reasoning"),
];

/// Levels from `THINKING_LEVELS` the model actually accepts. `off` is
/// offered when the model honors it (see [`reasoning_off_is_honest`]).
/// Unknown family → full catalog; known non-reasoning → just `off`.
pub fn supported_thinking_levels(model_id: &str) -> Vec<(&'static str, &'static str)> {
    // Family truth outranks advertised metadata: the step family always
    // reasons, whatever its /models endpoint or the models.dev cache says.
    if step_family(model_id) {
        return STEP_LEVELS.to_vec();
    }
    if model_supports_reasoning(model_id) == Some(false) {
        return vec![("off", "No reasoning")];
    }
    let Some(want) = supported_efforts(model_id) else {
        return super::super::THINKING_LEVELS.to_vec();
    };
    let off_ok = reasoning_off_is_honest(model_id);
    super::super::THINKING_LEVELS
        .iter()
        .filter(|(l, _)| (*l == "off" && off_ok) || want.contains(l))
        .copied()
        .collect()
}

/// Clamps a thinking level to what `model_id` actually accepts (Prime-Agent
/// `clampThinkingLevel` parity). Keeps `level` when supported, else the
/// nearest level in `THINKING_LEVELS` order — upward first, then downward
/// (so `max`→`xhigh` on Spark). `off` is always valid; unknown family
/// → full catalog (kept); `model_supports_reasoning == Some(false)` →
/// `off` only. Unknown/empty `level` falls back to the first supported level
/// (`off`).
pub fn clamp_thinking_level(model_id: &str, level: &str) -> &'static str {
    let supported = supported_thinking_levels(model_id);
    if let Some((l, _)) = supported.iter().find(|(l, _)| *l == level) {
        return l;
    }
    let order: Vec<&str> = super::super::THINKING_LEVELS
        .iter()
        .map(|(l, _)| *l)
        .collect();
    let Some(req_idx) = order.iter().position(|l| *l == level) else {
        return supported.first().map(|(l, _)| *l).unwrap_or("off");
    };
    for cand in order.iter().skip(req_idx) {
        if let Some((l, _)) = supported.iter().find(|(l, _)| l == cand) {
            return l;
        }
    }
    for cand in order[..req_idx].iter().rev() {
        if let Some((l, _)) = supported.iter().find(|(l, _)| l == cand) {
            return l;
        }
    }
    supported.first().map(|(l, _)| *l).unwrap_or("off")
}

/// Dynamically queries the provider's live /models endpoint (e.g. OpenAI, OpenRouter, Ollama, vLLM, LMStudio, etc.).
/// The live `/models` (or `/tags`) probe. Split out so the blocking entry
/// point and a background refresher thread can share one implementation.
async fn fetch_models_async(base: String, key: Option<String>) -> Vec<(String, String)> {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(3000))
        .user_agent(concat!("gray/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let trimmed_base = base.trim_end_matches('/');
    let endpoints = if trimmed_base.contains("openrouter.ai") {
        vec!["https://openrouter.ai/api/v1/models".to_string()]
    } else if trimmed_base.ends_with("/v1") {
        vec![
            format!("{trimmed_base}/models"),
            format!("{trimmed_base}/tags"),
        ]
    } else {
        vec![
            format!("{trimmed_base}/models"),
            format!("{trimmed_base}/v1/models"),
            format!("{trimmed_base}/api/tags"),
            format!("{trimmed_base}/api/v1/models"),
        ]
    };

    for url in endpoints {
        let mut req = client.get(&url);
        if let Some(k) = &key
            && !k.is_empty()
        {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        if url.contains("openrouter") {
            req = req.header("HTTP-Referer", "https://github.com/vstaln/gray");
            req = req.header("X-Title", "Gray");
        }

        if let Ok(resp) = req.send().await
            && resp.status().is_success()
            && let Ok(json) = resp.json::<serde_json::Value>().await
        {
            let mut models = Vec::new();
            let items_opt = if let Some(arr) = json.as_array() {
                Some(arr)
            } else if let Some(arr) = json.get("data").and_then(|d| d.as_array()) {
                Some(arr)
            } else {
                json.get("models").and_then(|m| m.as_array())
            };

            if let Some(items) = items_opt {
                for item in items {
                    let id = item
                        .get("id")
                        .or_else(|| item.get("name"))
                        .or_else(|| item.get("model"))
                        .and_then(|v| v.as_str());
                    if let Some(id_str) = id {
                        let name = item
                            .get("name")
                            .or_else(|| item.get("display_name"))
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| friendly_model_name(id_str));
                        if let Some(len) = extract_context_length_from_json(item) {
                            cache_model_context(id_str, len);
                        }
                        // OpenRouter advertises `supported_parameters:
                        // [..., "reasoning", ...]`; other OpenAI-style
                        // endpoints may carry a `reasoning` bool.
                        if let Some(r) = item
                            .get("supported_parameters")
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().any(|p| p.as_str() == Some("reasoning")))
                            .or_else(|| item.get("reasoning").and_then(|v| v.as_bool()))
                        {
                            cache_model_reasoning(id_str, r);
                        }
                        // OpenRouter-shaped items carry the facts the
                        // recommended-model ranking needs; gap-fill only,
                        // models.dev stays authoritative.
                        let tool_call = item
                            .get("supported_parameters")
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().any(|p| p.as_str() == Some("tools")));
                        let text_only = item
                            .get("architecture")
                            .and_then(|a| a.get("output_modalities"))
                            .and_then(|v| v.as_array())
                            .map(|o| o.len() == 1 && o[0].as_str() == Some("text"));
                        if tool_call.is_some() || text_only.is_some() {
                            cache_model_meta_gap_fill(
                                id_str,
                                ModelMeta {
                                    tool_call,
                                    text_only,
                                    release_date: None,
                                    family: None,
                                },
                            );
                        }
                        models.push((id_str.to_string(), name));
                    }
                }
            }
            if !models.is_empty() {
                cache_provider_model_ids(&base, &models);
                save_provider_model_list(&base, &models);
                save_models_cache_to_disk();
                return models;
            }
        }
    }

    Vec::new()
}

/// Live provider model list. Blocking by design: every caller wants an
/// answer before it draws. With no ambient runtime (a background thread)
/// run a short-lived one rather than quietly returning an empty list.
pub fn fetch_live_provider_models(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let base = base_url.to_string();
    let key = api_key.map(|k| k.to_string());
    // ponytail: isolated short-lived runtime, never the ambient Handle — block_on
    // on a borrowed handle panics with "Tokio 1.x context ... being shutdown"
    // when a background fetch outlives REPL shutdown. Own runtime = no coupling.
    std::thread::scope(|s| {
        s.spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(fetch_models_async(base, key)),
                Err(_) => Vec::new(),
            }
        })
        .join()
        .unwrap_or_default()
    })
}

/// Live model list for a plugin-backed provider: the sidecar's
/// `provider/models` RPC is the only real source — the declared `base_url`
/// is a relay placeholder an HTTP fetch can never satisfy. A non-empty
/// result persists under [`crate::setup::catalog::plugin_models_key`], so
/// the `/model` picker paints instantly on the next open (loopback bases
/// themselves get no disk entry — see `save_provider_model_list_at`).
/// Same own-runtime discipline as [`fetch_live_provider_models`].
pub(crate) fn fetch_plugin_provider_models(
    installed: crate::providers::InstalledProvider,
) -> Vec<(String, String)> {
    std::thread::scope(|s| {
        s.spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(plugin_models_rpc(&installed)),
                Err(_) => Vec::new(),
            }
        })
        .join()
        .unwrap_or_default()
    })
}

/// The async half of [`fetch_plugin_provider_models`]: spawn the sidecar,
/// ask for its catalog, persist a usable list. The stored credential goes
/// in when one exists; subscription plugins that keep their own login state
/// answer to an empty envelope (the connect-login path uses exactly that).
async fn plugin_models_rpc(
    installed: &crate::providers::InstalledProvider,
) -> Vec<(String, String)> {
    use crate::providers::registry::ProviderRpc;
    let Ok(runtime) = crate::providers::ProviderRuntime::start(installed.clone()).await else {
        return Vec::new();
    };
    let credential = crate::setup::catalog::auth_store_path()
        .ok()
        .and_then(|path| {
            crate::auth::CredentialStore::new(path)
                .read_plugin(&installed.auth_ref())
                .ok()
                .flatten()
        })
        .or_else(|| {
            gray_core::credential::CredentialEnvelope::new(
                installed.plugin.clone(),
                installed.provider.id.clone(),
                installed.auth_method.id.clone(),
                installed.profile_binding.clone(),
                gray_core::credential::CredentialMaterial::empty(),
            )
            .ok()
        });
    let Some(credential) = credential else {
        return Vec::new();
    };
    let request = gray_plugin::ProviderModelsRequest {
        provider: installed.provider.id.clone(),
        auth_method: installed.auth_method.id.clone(),
        profile_binding: installed.profile_binding.clone(),
        credential,
    };
    let Ok(catalog) = runtime.rpc().models(request).await else {
        return Vec::new();
    };
    // Provider-declared metadata feeds the same caches a live /models
    // payload does — effort levels the plugin advertises become
    // authoritative for the clamp and picker rows. A plugin catalog IS
    // the declaration: an empty efforts list means "no effort knob" (the
    // tier is baked into the model id, e.g. Devin's `swe-2-max`), not
    // "didn't say" — mark it so the picker reports no levels, the footer
    // hides the effort badge, and no default effort is sent.
    for m in &catalog.models {
        if let Some(w) = m.context_window {
            cache_model_context_if_absent(&m.id, w as usize);
        }
    }
    // Rows go to disk as objects (efforts, variants, slots ride along) so
    // the next open — and the request builder resolving a variant — knows
    // them before any RPC; registering feeds the in-memory caches now.
    let rows: Vec<crate::setup::variants::CachedRow> = catalog
        .models
        .iter()
        .map(|m| {
            let name = m.name.trim();
            crate::setup::variants::CachedRow::Full(crate::setup::variants::CachedModel {
                id: m.id.clone(),
                name: if name.is_empty() {
                    m.id.clone()
                } else {
                    name.to_string()
                },
                reasoning_efforts: m.reasoning_efforts.clone(),
                variants: m.variants.clone(),
                slots: m.slots.clone(),
                declared: true,
            })
        })
        .collect();
    for row in &rows {
        row.register();
    }
    let models: Vec<(String, String)> = rows.iter().map(|r| r.pair()).collect();
    if !rows.is_empty() {
        save_provider_model_rows(
            &crate::setup::catalog::plugin_models_key(&installed.provider_id()),
            rows,
        );
    }
    models
}

/// Verdict of the connect-modal key check. `Rejected` is the provider's own
/// answer (an auth refusal); `Unknown` is every inconclusive probe — offline,
/// timeout, odd provider — and must never block connecting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    Valid,
    Rejected(u16),
    Unknown,
}

/// How the check authenticates: a Bearer header everywhere except Anthropic,
/// whose API wants `x-api-key` + `anthropic-version` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthStyle {
    Bearer,
    Anthropic,
}

/// Hosts that answer a bad key with 400 instead of 401 — measured: Google
/// (`generativelanguage.googleapis.com`) and xAI (`api.x.ai`) both reject a
/// fake Bearer on `/models` with 400.
const BAD_KEY_IS_400: &[&str] = &["generativelanguage.googleapis.com", "api.x.ai"];

/// The (url, auth, reject-400) probe for `base_url`: OpenRouter's `/key`
/// (its `/models` is public and validates nothing), Anthropic's `/models`
/// under its native headers, everything else's `/models` under Bearer. The
/// flag marks hosts whose 400 means "bad key" rather than "bad request".
pub(crate) fn key_check_request(base_url: &str) -> (String, AuthStyle, bool) {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.contains("openrouter.ai") {
        return (
            "https://openrouter.ai/api/v1/key".to_string(),
            AuthStyle::Bearer,
            false,
        );
    }
    let host = reqwest::Url::parse(trimmed)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let style = if host == "api.anthropic.com" {
        AuthStyle::Anthropic
    } else {
        AuthStyle::Bearer
    };
    (
        format!("{trimmed}/models"),
        style,
        BAD_KEY_IS_400.contains(&host.as_str()),
    )
}

/// HTTP status → verdict: only an explicit auth refusal is a rejection —
/// 401/403 anywhere, 400 on the hosts that use it for auth — anything else
/// (5xx, redirects, odd 2xx-adjacent) stays inconclusive.
pub(crate) fn classify_key_status(status: u16, reject_400: bool) -> KeyCheck {
    match status {
        200..=299 => KeyCheck::Valid,
        401 | 403 => KeyCheck::Rejected(status),
        400 if reject_400 => KeyCheck::Rejected(status),
        _ => KeyCheck::Unknown,
    }
}

async fn key_check_async(url: String, style: AuthStyle, reject_400: bool, key: String) -> KeyCheck {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(concat!("gray/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return KeyCheck::Unknown;
    };
    let req = match style {
        AuthStyle::Bearer => client.get(&url).bearer_auth(key),
        AuthStyle::Anthropic => client
            .get(&url)
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
    };
    // The body is never read: providers may echo key fragments back.
    match req.send().await {
        Ok(resp) => classify_key_status(resp.status().as_u16(), reject_400),
        Err(_) => KeyCheck::Unknown,
    }
}

/// Ask the provider whether `key` works — the connect modal's pre-save check.
/// Blocking with a hard 5s cap on its own short-lived runtime, the same
/// discipline as [`fetch_live_provider_models`].
pub fn verify_api_key(base_url: &str, key: &str) -> KeyCheck {
    let (url, style, reject_400) = key_check_request(base_url);
    let key = key.to_string();
    std::thread::scope(|s| {
        s.spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(key_check_async(url, style, reject_400, key)),
                Err(_) => KeyCheck::Unknown,
            }
        })
        .join()
        .unwrap_or(KeyCheck::Unknown)
    })
}

/// Local Ollama probe for the connect list's `(running locally)` badge —
/// one ~300ms ask of `/api/tags`, taken once when the modal opens (the item
/// list rebuilds every loop pass, far too often for a probe).
pub(crate) fn ollama_running() -> bool {
    std::thread::scope(|s| {
        s.spawn(|| {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(async {
                    let Ok(client) = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_millis(300))
                        .build()
                    else {
                        return false;
                    };
                    client
                        .get("http://localhost:11434/api/tags")
                        .send()
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false)
                }),
                Err(_) => false,
            }
        })
        .join()
        .unwrap_or(false)
    })
}

static MODEL_CONTEXT_CACHE: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, usize>>,
> = std::sync::OnceLock::new();

fn model_context_cache() -> &'static std::sync::RwLock<std::collections::HashMap<String, usize>> {
    MODEL_CONTEXT_CACHE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

pub fn cache_model_context(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "live", true);
}

pub fn get_cached_model_context(model_id: &str) -> Option<usize> {
    if let Ok(g) = model_context_cache().read() {
        if let Some(v) = g.get(model_id).copied() {
            return Some(v);
        }
        let lower = model_id.to_lowercase();
        if let Some(v) = g.get(&lower).copied() {
            return Some(v);
        }
    }
    None
}

/// Model discovery is connection-scoped, unlike the global context/pricing cache.
#[derive(Default)]
struct ProviderModels {
    active: String,
    ids: std::collections::HashMap<String, Vec<String>>,
}

static PROVIDER_MODELS: std::sync::OnceLock<std::sync::RwLock<ProviderModels>> =
    std::sync::OnceLock::new();

fn provider_models_cell() -> &'static std::sync::RwLock<ProviderModels> {
    PROVIDER_MODELS.get_or_init(|| std::sync::RwLock::new(ProviderModels::default()))
}

/// Called at startup and after connecting, never by background fetches.
/// A late response from the previous connection must not change completion scope.
pub fn set_active_model_provider(base_url: &str) {
    if let Ok(mut cache) = provider_models_cell().write() {
        cache.active = base_url.trim_end_matches('/').to_string();
    }
}

/// The provider endpoint currently selected for model discovery.
fn active_provider_base_url() -> String {
    provider_models_cell()
        .read()
        .ok()
        .map(|cache| cache.active.clone())
        .unwrap_or_default()
}

pub(crate) fn cache_provider_model_ids(base_url: &str, models: &[(String, String)]) {
    if let Ok(mut cache) = provider_models_cell().write() {
        let mut ids: Vec<String> = models.iter().map(|(id, _)| id.clone()).collect();
        ids.sort();
        ids.dedup();
        cache
            .ids
            .insert(base_url.trim_end_matches('/').to_string(), ids);
    }
}

/// Only ids advertised by the active endpoint; no I/O per keystroke.
pub fn cached_model_ids() -> Vec<String> {
    provider_models_cell()
        .read()
        .ok()
        .and_then(|cache| cache.ids.get(&cache.active).cloned())
        .unwrap_or_default()
}

/// Loopback endpoint (`localhost`, `127.0.0.0/8`, `::1`) by host, unparseable
/// input treated as not loopback (the caller is a normalized base URL).
fn is_loopback_base(base: &str) -> bool {
    reqwest::Url::parse(base)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .is_some_and(|host| is_loopback_host(&host))
}

/// Loopback by name or by address, IPv6 brackets included.
fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A previous session's provider model list, so the picker paints instantly
/// and only the background refresh touches the network. On-disk list cache
/// (`~/.gray/provider_models.json`, `{ "<base>": [["id", "name"], ...] }`),
/// keyed like `recent_models` by normalized base URL. Rows with catalog
/// metadata (plugin efforts / variants / slots) are stored as objects
/// instead of pairs; both forms load ([`crate::setup::variants::CachedRow`]).
pub(crate) fn save_provider_model_list_at(
    home: &std::path::Path,
    base_url: &str,
    models: &[(String, String)],
) {
    let rows = models
        .iter()
        .map(|(id, name)| crate::setup::variants::CachedRow::Pair(id.clone(), name.clone()))
        .collect();
    save_provider_model_rows_at(home, base_url, rows);
}

/// [`save_provider_model_list_at`] for rows that may carry metadata. A bare
/// pair re-saved over an object (a generic refresh of the same id) keeps the
/// object's metadata and takes the new name.
pub(crate) fn save_provider_model_rows_at(
    home: &std::path::Path,
    base_url: &str,
    rows: Vec<crate::setup::variants::CachedRow>,
) {
    use crate::setup::variants::CachedRow;
    if rows.is_empty() {
        // A failed fetch must never wipe the last good list.
        return;
    }
    let base = crate::setup::catalog::normalize_custom_base_url(base_url);
    if base.is_empty() || is_loopback_base(&base) {
        // Loopback gets no disk entry: the fetch is already sub-millisecond
        // local (nothing to paint ahead of), and the port changes on every
        // restart — each one a dead key this cache would keep forever (a unit
        // test's random port added one per `cargo test`). There is no
        // eviction, so refusing the write is the only bound that holds.
        return;
    }
    let path = home.join("provider_models.json");
    let mut map = read_model_list_map(&path);
    let old = map.remove(&base).unwrap_or_default();
    let rows = rows
        .into_iter()
        .map(|row| match row {
            CachedRow::Pair(id, name) => match old.iter().find(|o| o.id() == id) {
                Some(CachedRow::Full(m)) => {
                    CachedRow::Full(crate::setup::variants::CachedModel { name, ..m.clone() })
                }
                _ => CachedRow::Pair(id, name),
            },
            full => full,
        })
        .collect();
    map.insert(base, rows);
    let Ok(s) = serde_json::to_string(&map) else {
        return;
    };
    // Atomic tmp-file + rename (models.json precedent).
    let tmp = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    if std::fs::write(&tmp, s).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, path);
}

/// The whole cache file; missing or corrupt reads as empty.
fn read_model_list_map(
    path: &std::path::Path,
) -> std::collections::BTreeMap<String, Vec<crate::setup::variants::CachedRow>> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Best-effort load of one provider's cached rows (pairs and objects).
pub(crate) fn load_provider_model_rows_at(
    home: &std::path::Path,
    base_url: &str,
) -> Vec<crate::setup::variants::CachedRow> {
    let base = crate::setup::catalog::normalize_custom_base_url(base_url);
    read_model_list_map(&home.join("provider_models.json"))
        .remove(&base)
        .unwrap_or_default()
}

/// Best-effort load: missing file, unknown base, or corrupt JSON reads as
/// an empty list and the caller falls back to the live fetch.
#[cfg(test)]
pub(crate) fn load_provider_model_list_at(
    home: &std::path::Path,
    base_url: &str,
) -> Vec<(String, String)> {
    load_provider_model_rows_at(home, base_url)
        .iter()
        .map(|r| r.pair())
        .collect()
}

/// Thin `gray_home` wrappers; the `_at` forms above stay testable without
/// touching process env.
pub(crate) fn save_provider_model_list(base_url: &str, models: &[(String, String)]) {
    if let Ok(home) = crate::setup::catalog::gray_home() {
        save_provider_model_list_at(&home, base_url, models);
    }
}

pub(crate) fn save_provider_model_rows(
    base_url: &str,
    rows: Vec<crate::setup::variants::CachedRow>,
) {
    if let Ok(home) = crate::setup::catalog::gray_home() {
        save_provider_model_rows_at(&home, base_url, rows);
    }
}

/// The cached list for `base_url`; object rows register their metadata
/// (efforts, variants, slots) on the way out, so whoever reads the list
/// next — picker or request builder — sees the declared shapes.
pub(crate) fn load_provider_model_list(base_url: &str) -> Vec<(String, String)> {
    let Ok(home) = crate::setup::catalog::gray_home() else {
        return Vec::new();
    };
    let rows = load_provider_model_rows_at(&home, base_url);
    for row in &rows {
        row.register();
    }
    rows.iter().map(|r| r.pair()).collect()
}

/// Gap-fill insert: leaves an existing entry (e.g. provider-fetched) alone.
/// Provider values always win over the LiteLLM table regardless of arrival order.
pub fn cache_model_context_if_absent(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "litellm", false);
}

/// Gap-fill insert for models.dev values (same provider-wins semantics).
pub fn cache_models_dev_if_absent(model_id: &str, length: usize) {
    cache_model_context_with_source(model_id, length, "models.dev", false);
}

/// Shared insert behind the cache fns above; also fans out the source tag.
fn cache_model_context_with_source(
    model_id: &str,
    length: usize,
    src: &'static str,
    overwrite: bool,
) {
    if length == 0 {
        return;
    }
    if let Ok(mut g) = model_context_cache().write()
        && (overwrite || !g.contains_key(model_id))
    {
        g.insert(model_id.to_string(), length);
        let lower = model_id.to_lowercase();
        if lower != model_id {
            if overwrite {
                g.insert(lower, length);
            } else {
                g.entry(lower).or_insert(length);
            }
        }
    }
    record_context_source(model_id, src, overwrite);
}

static MODEL_CONTEXT_SOURCE: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, &'static str>>,
> = std::sync::OnceLock::new();

fn model_context_source_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, &'static str>> {
    MODEL_CONTEXT_SOURCE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Tags a cached window with its origin. Gap-fill callers pass
/// `overwrite: false` so a live value keeps its "live" tag.
fn record_context_source(model_id: &str, src: &'static str, overwrite: bool) {
    if let Ok(mut g) = model_context_source_cell().write() {
        if overwrite || !g.contains_key(model_id) {
            g.insert(model_id.to_string(), src);
        }
        let lower = model_id.to_lowercase();
        if lower != model_id && (overwrite || !g.contains_key(&lower)) {
            g.insert(lower, src);
        }
    }
}

fn get_cached_source(model_id: &str) -> Option<&'static str> {
    if let Ok(g) = model_context_source_cell().read() {
        if let Some(s) = g.get(model_id).copied() {
            return Some(s);
        }
        let lower = model_id.to_lowercase();
        if let Some(s) = g.get(&lower).copied() {
            return Some(s);
        }
        if let Some((_, suffix)) = model_id.rsplit_once('/') {
            if let Some(s) = g.get(suffix).copied() {
                return Some(s);
            }
            if let Some(s) = g.get(&suffix.to_lowercase()).copied() {
                return Some(s);
            }
        }
    }
    None
}

/// Where the effective window for `model` came from.
pub fn context_source(model: &str) -> &'static str {
    if get_user_context_window().is_some() {
        return "override";
    }
    ensure_disk_loaded();
    if let Some(s) = get_cached_source(model) {
        return s;
    }
    "guess"
}

fn json_usize(v: &serde_json::Value) -> Option<usize> {
    v.as_u64()
        .map(|n| n as usize)
        .or_else(|| v.as_f64().map(|f| f as usize))
}

/// USD-per-token rates from LiteLLM's table (same source as the context
/// windows — and the one T3 Code prices against). Base tier, like T3.
/// A priced context tier (models.dev `cost.tiers`, LiteLLM
/// `*_above_200k_tokens`): when a turn's inclusive input exceeds `size`,
/// these rates replace the base ones — opencode `getUsage` semantics
/// (`context > tier.size`, largest matching tier wins). Fields left `None`
/// inherit the base rate: models.dev `context_over_200k` often omits
/// `cache_write`, and zero-filling it (opencode's `?? 0`) undercharges.
#[derive(Debug, Clone, Copy)]
pub struct RateTier {
    pub size: usize,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelRate {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    /// False when the entry had no cache prices — price all input at `input`.
    pub has_cache_prices: bool,
    /// Context-priced tiers, sorted descending by `size`. Empty = flat rate.
    pub tiers: Vec<RateTier>,
}

static MODEL_RATES: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, ModelRate>>,
> = std::sync::OnceLock::new();

fn model_rates_cell() -> &'static std::sync::RwLock<std::collections::HashMap<String, ModelRate>> {
    MODEL_RATES.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Tiers seen before their base rate exists (a models.dev entry that ships
/// `cost.tiers`/`context_over_200k` but no base `input`/`output`). Drained by
/// the next rate write for that id — whichever source prices it first picks
/// the tiers up, so apply order between the catalog fetches never loses one.
static PENDING_TIERS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<String, Vec<RateTier>>>,
> = std::sync::OnceLock::new();

fn pending_tiers_cell()
-> &'static std::sync::RwLock<std::collections::HashMap<String, Vec<RateTier>>> {
    PENDING_TIERS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

fn stash_pending_tiers(model_id: &str, tiers: Vec<RateTier>) {
    if tiers.is_empty() {
        return;
    }
    if let Ok(mut g) = pending_tiers_cell().write() {
        merge_tiers(g.entry(model_id.to_string()).or_default(), tiers.clone());
        let lower = model_id.to_lowercase();
        if lower != model_id {
            merge_tiers(g.entry(lower).or_default(), tiers);
        }
    }
}

fn take_pending_tiers(model_id: &str) -> Vec<RateTier> {
    pending_tiers_cell()
        .write()
        .ok()
        .and_then(|mut g| g.remove(model_id))
        .unwrap_or_default()
}

/// Union tier lists by `size`; an incoming tier replaces the stored one at
/// the same size (the caller's source is authoritative for the tiers it
/// ships). Result stays sorted descending.
fn merge_tiers(existing: &mut Vec<RateTier>, incoming: Vec<RateTier>) {
    for t in incoming {
        match existing.iter_mut().find(|e| e.size == t.size) {
            Some(e) => *e = t,
            None => existing.push(t),
        }
    }
    existing.sort_by_key(|tier| std::cmp::Reverse(tier.size));
}

/// LiteLLM path: the base rates replace (authoritative); tiers union into
/// whatever arrived earlier so a models.dev apply in either boot order
/// yields the same merged rate.
fn cache_model_rate(model_id: &str, rate: ModelRate) {
    fn merge_into(
        map: &mut std::collections::HashMap<String, ModelRate>,
        id: String,
        rate: ModelRate,
    ) {
        match map.get_mut(&id) {
            Some(cur) => {
                merge_tiers(&mut cur.tiers, rate.tiers);
                let tiers = std::mem::take(&mut cur.tiers);
                *cur = ModelRate { tiers, ..rate };
            }
            None => {
                let mut rate = rate;
                merge_tiers(&mut rate.tiers, take_pending_tiers(&id));
                map.insert(id, rate);
            }
        }
    }
    if let Ok(mut g) = model_rates_cell().write() {
        merge_into(&mut g, model_id.to_string(), rate.clone());
        let lower = model_id.to_lowercase();
        if lower != model_id {
            merge_into(&mut g, lower, rate);
        }
    }
}

/// models.dev path: gap-fill — base rates are added only when no source has
/// priced the model (LiteLLM/OpenRouter stay authoritative); tiers always
/// merge, but an existing tier at the same size keeps its rates.
fn cache_models_dev_rate(model_id: &str, base: Option<ModelRate>, tiers: Vec<RateTier>) {
    if base.is_none() && tiers.is_empty() {
        return;
    }
    let mut ids = vec![model_id.to_string()];
    let lower = model_id.to_lowercase();
    if lower != model_id {
        ids.push(lower);
    }
    let mut stashed: Vec<String> = Vec::new();
    if let Ok(mut g) = model_rates_cell().write() {
        for id in &ids {
            match g.get_mut(id) {
                Some(cur) => {
                    let missing: Vec<RateTier> = tiers
                        .iter()
                        .copied()
                        .filter(|t| !cur.tiers.iter().any(|e| e.size == t.size))
                        .collect();
                    cur.tiers.extend(missing);
                    cur.tiers.sort_by_key(|tier| std::cmp::Reverse(tier.size));
                }
                None => {
                    if let Some(mut r) = base.clone() {
                        merge_tiers(&mut r.tiers, tiers.clone());
                        g.insert(id.clone(), r);
                    } else {
                        stashed.push(id.clone());
                    }
                }
            }
        }
    }
    for id in stashed {
        stash_pending_tiers(&id, tiers.clone());
    }
}

/// Rate for a model id, with the same `provider/model` tail fallback as the
/// context cache. None = unpriced (LiteLLM has no rate for it).
pub fn get_model_rate(model_id: &str) -> Option<ModelRate> {
    if let Ok(g) = model_rates_cell().read() {
        if let Some(r) = g.get(model_id).cloned() {
            return Some(r);
        }
        let lower = model_id.to_lowercase();
        if let Some(r) = g.get(&lower).cloned() {
            return Some(r);
        }
        if let Some((_, suffix)) = model_id.rsplit_once('/')
            && let Some(r) = g.get(suffix).cloned()
        {
            return Some(r);
        }
    }
    None
}

fn json_rate(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .or_else(|| v.as_u64().map(|n| n as f64))
}

/// Turn cost in USD, or None when the model is unpriced. Cache-aware: fresh
/// input at `input`, cached reads/writes at their prices; providers that only
/// fill inclusive `input_tokens` get it all priced fresh.
pub fn turn_cost(usage: &gray_core::event::Usage, model: &str) -> Option<f64> {
    let r = get_model_rate(model)?;
    let (input, output, cache_read, cache_write, has_cache) =
        rate_at_context(&r, usage.input_tokens);
    let read = usage.cache_read_input_tokens as f64;
    let write = usage.cache_write_input_tokens as f64;
    let mut fresh = usage.non_cached_input_tokens as f64;
    if fresh == 0.0 {
        fresh = (usage.input_tokens as f64 - read - write).max(0.0);
    }
    let input_cost = if has_cache {
        fresh * input + read * cache_read + write * cache_write
    } else {
        (fresh + read + write) * input
    };
    Some(input_cost + usage.output_tokens as f64 * output)
}

/// The same turn with caching off: every input token at the plain input
/// rate (same [`rate_at_context`] tier selection as [`turn_cost`]) plus
/// output. The `Saved` row's counterfactual — `turn_cost` subtracted from
/// this is what prompt caching kept off the bill. `None` when unpriced.
pub fn uncached_cost(usage: &gray_core::event::Usage, model: &str) -> Option<f64> {
    let r = get_model_rate(model)?;
    let (input, output, ..) = rate_at_context(&r, usage.input_tokens);
    Some(usage.input_tokens as f64 * input + usage.output_tokens as f64 * output)
}

/// The rate in force for a prompt of `context` tokens — the base rate, or
/// the largest tier strictly below it (opencode `getUsage`: tiers apply on
/// `context > tier.size`; models.dev only ships `type: "context"` tiers).
fn rate_at_context(r: &ModelRate, context: usize) -> (f64, f64, f64, f64, bool) {
    let Some(t) = r
        .tiers
        .iter()
        .filter(|t| context > t.size)
        .max_by_key(|t| t.size)
    else {
        return (
            r.input,
            r.output,
            r.cache_read,
            r.cache_write,
            r.has_cache_prices,
        );
    };
    (
        t.input.unwrap_or(r.input),
        t.output.unwrap_or(r.output),
        t.cache_read.unwrap_or(r.cache_read),
        t.cache_write.unwrap_or(r.cache_write),
        r.has_cache_prices || t.cache_read.is_some() || t.cache_write.is_some(),
    )
}

/// `$0.004`, `$0.41`, `$1.50` — 4 decimals trimmed, 2 minimum past a dollar.
pub fn format_cost(usd: f64) -> String {
    if usd >= 1.0 {
        return format!("${:.2}", usd);
    }
    let trimmed = format!("{:.4}", usd)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string();
    if trimmed == "0" {
        return if usd > 0.0 {
            "<$0.0001".to_string()
        } else {
            "$0".to_string()
        };
    }
    format!("${trimmed}")
}

// Field subsets of the public catalogs. Deserializing straight into these,
// never into a full `Value` tree (5+ MB of tiny nodes), keeps startup
// memory flat: a dropped tree leaves its pages pinned by cache entries
// allocated in between. Leaf fields stay `Value` so a type change in one
// field reads as absent; `BTreeMap` keeps the sorted order `Value` maps had.
type Field = Option<serde_json::Value>;

#[derive(serde::Deserialize)]
pub struct LiteLlmEntry {
    max_input_tokens: Field,
    max_tokens: Field,
    input_cost_per_token: Field,
    output_cost_per_token: Field,
    cache_read_input_token_cost: Field,
    cache_creation_input_token_cost: Field,
    /// Anthropic-style long-context surcharge: present => tier at 200k.
    input_cost_per_token_above_200k_tokens: Field,
    output_cost_per_token_above_200k_tokens: Field,
    cache_read_input_token_cost_above_200k_tokens: Field,
    cache_creation_input_token_cost_above_200k_tokens: Field,
}
type LiteLlm = std::collections::BTreeMap<String, LiteLlmEntry>;

#[derive(serde::Deserialize)]
pub struct ModelsDevProvider {
    models: Option<std::collections::BTreeMap<String, ModelsDevEntry>>,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevEntry {
    limit: Option<ModelsDevLimit>,
    context_window: Field,
    max_input_tokens: Field,
    reasoning: Field,
    reasoning_options: Option<Vec<ModelsDevOption>>,
    /// USD per 1M tokens (opencode `model.cost`), with optional context tiers.
    cost: Option<ModelsDevCost>,
    /// Recommended-ranking facts: tool support, output shape, freshness.
    tool_call: Field,
    modalities: Option<ModelsDevModalities>,
    release_date: Field,
    family: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevModalities {
    output: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevCost {
    input: Field,
    output: Field,
    cache_read: Field,
    cache_write: Field,
    tiers: Option<Vec<ModelsDevCostTier>>,
    /// Flattened 200k tier (newer key for opencode's `experimentalOver200K`).
    context_over_200k: Option<ModelsDevTierRates>,
    #[serde(alias = "experimentalOver200K")]
    experimental_over_200k: Option<ModelsDevTierRates>,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevCostTier {
    input: Field,
    output: Field,
    cache_read: Field,
    cache_write: Field,
    tier: Option<ModelsDevTierBound>,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevTierBound {
    #[serde(rename = "type")]
    kind: Field,
    size: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevTierRates {
    input: Field,
    output: Field,
    cache_read: Field,
    cache_write: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevLimit {
    context: Field,
}
#[derive(serde::Deserialize)]
pub struct ModelsDevOption {
    #[serde(rename = "type")]
    kind: Field,
    values: Field,
}
type ModelsDev = std::collections::BTreeMap<String, ModelsDevProvider>;

#[derive(serde::Deserialize)]
pub struct OpenRouter {
    data: Vec<OpenRouterEntry>,
}
#[derive(serde::Deserialize)]
pub struct OpenRouterEntry {
    id: Field,
    pricing: Option<OpenRouterPricing>,
}
#[derive(serde::Deserialize)]
pub struct OpenRouterPricing {
    prompt: Field,
    completion: Field,
}

/// Projects LiteLLM's public `model_prices_and_context_window.json` (the same
/// table T3 Code / ccusage price against) into the context cache.
/// `max_input_tokens` is the window; legacy `max_tokens` is the fallback
/// (on new entries it can mean output size, so it loses). Gap-fill only.
/// Returns the number of models cached.
pub fn parse_litellm_context_json(val: &serde_json::Value) -> usize {
    serde::Deserialize::deserialize(val).map_or(0, apply_litellm)
}

fn apply_litellm(map: LiteLlm) -> usize {
    let mut n = 0;
    for (key, entry) in &map {
        if key == "sample_spec" {
            continue;
        }
        let len = entry
            .max_input_tokens
            .as_ref()
            .and_then(json_usize)
            .or_else(|| entry.max_tokens.as_ref().and_then(json_usize));
        if let Some(len) = len.filter(|&v| v >= 1024) {
            cache_model_context_if_absent(key, len);
            // Keys are usually bare (`gpt-4o`); index the tail too so
            // `provider/model` lookups hit without knowing every prefix.
            if let Some((_, suffix)) = key.rsplit_once('/') {
                cache_model_context_if_absent(suffix, len);
            }
            n += 1;
        }
        // Rates ride the same loop. Both base rates required — a half-priced
        // model silently under-reports, which is worse than unpriced.
        let rate = |f: &Field| f.as_ref().and_then(json_rate);
        if let (Some(input), Some(output)) = (
            rate(&entry.input_cost_per_token),
            rate(&entry.output_cost_per_token),
        ) {
            let (cache_read, cache_write, has_cache) = match (
                rate(&entry.cache_read_input_token_cost),
                rate(&entry.cache_creation_input_token_cost),
            ) {
                (Some(r), Some(w)) => (r, w, true),
                _ => (0.0, 0.0, false),
            };
            let tiers = {
                let t_in = rate(&entry.input_cost_per_token_above_200k_tokens);
                let t_out = rate(&entry.output_cost_per_token_above_200k_tokens);
                if t_in.is_some() || t_out.is_some() {
                    vec![RateTier {
                        size: 200_000,
                        input: t_in,
                        output: t_out,
                        cache_read: rate(&entry.cache_read_input_token_cost_above_200k_tokens),
                        cache_write: rate(&entry.cache_creation_input_token_cost_above_200k_tokens),
                    }]
                } else {
                    Vec::new()
                }
            };
            let rate = ModelRate {
                input,
                output,
                cache_read,
                cache_write,
                has_cache_prices: has_cache,
                tiers,
            };
            cache_model_rate(key, rate.clone());
            if let Some((_, suffix)) = key.rsplit_once('/') {
                cache_model_rate(suffix, rate);
            }
        }
    }
    n
}

/// Fetches LiteLLM's model table in the background and caches context windows.
/// Fire-and-forget: callers `tokio::spawn` this at boot next to the provider
/// `/models` fetch. Failures are silent — the hardcoded fallback covers offline.
pub async fn fetch_litellm_context_windows() {
    const URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
    if load_catalog("litellm.json", URL, apply_litellm).await > 0 {
        save_models_cache_to_disk();
    }
}

/// Projects models.dev's public `api.json` (`providers -> models ->
/// `limit.context`, the same shape opencode's provider.ts consumes) into the
/// context cache. Also accepts `context_window` / `max_input_tokens` keys.
/// Gap-fill only. Returns the number of models cached.
pub fn parse_models_dev_json(val: &serde_json::Value) -> usize {
    serde::Deserialize::deserialize(val).map_or(0, apply_models_dev)
}

fn apply_models_dev(providers: ModelsDev) -> usize {
    let mut n = 0;
    for models in providers.values().filter_map(|p| p.models.as_ref()) {
        for (key, entry) in models {
            let len = entry
                .limit
                .as_ref()
                .and_then(|l| l.context.as_ref())
                .and_then(json_usize)
                .or_else(|| entry.context_window.as_ref().and_then(json_usize))
                .or_else(|| entry.max_input_tokens.as_ref().and_then(json_usize));
            if let Some(len) = len.filter(|&v| v >= 1024) {
                cache_models_dev_if_absent(key, len);
                if let Some((_, suffix)) = key.rsplit_once('/') {
                    cache_models_dev_if_absent(suffix, len);
                }
                n += 1;
            }
            // models.dev `reasoning` bool — same flag opencode maps to
            // `capabilities.reasoning`.
            if let Some(r) = entry.reasoning.as_ref().and_then(|v| v.as_bool()) {
                cache_model_reasoning(key, r);
            }
            // Capability/shape facts for the recommended-model ranking;
            // text-only means output is exactly `["text"]` (an image-output
            // model like nano-banana must not be recommended).
            let meta = ModelMeta {
                tool_call: entry.tool_call.as_ref().and_then(|v| v.as_bool()),
                text_only: entry
                    .modalities
                    .as_ref()
                    .and_then(|m| m.output.as_ref())
                    .and_then(|v| v.as_array())
                    .map(|o| o.len() == 1 && o[0].as_str() == Some("text")),
                release_date: entry
                    .release_date
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                family: entry
                    .family
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            };
            if meta != ModelMeta::default() {
                cache_model_meta(key, meta.clone());
                if let Some((_, suffix)) = key.rsplit_once('/') {
                    cache_model_meta(suffix, meta);
                }
            }
            // models.dev `reasoning_options` effort values — the automatic
            // per-model source opencode derives variants from
            // (`reasoningVariants`, transform.ts). `null` means `none`.
            // Toggle/budget-only entries stay on family tables.
            if let Some(values) = entry
                .reasoning_options
                .iter()
                .flatten()
                .find(|o| o.kind.as_ref().and_then(|t| t.as_str()) == Some("effort"))
                .and_then(|o| o.values.as_ref())
                .and_then(|v| v.as_array())
            {
                let efforts: Vec<String> = values
                    .iter()
                    .filter_map(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .or_else(|| v.is_null().then(|| "none".to_string()))
                    })
                    .collect();
                if !efforts.is_empty() {
                    cache_model_efforts(key, efforts);
                }
            }
            // models.dev `cost` (per-1M USD, divided here to match the
            // per-token LiteLLM rates). Gap-fill: LiteLLM stays authoritative
            // for base rates; tiers union (weaker `context_over_200k` first,
            // explicit `tiers` last so they win a same-size slot).
            if let Some(cost) = &entry.cost {
                let per_m = |f: &Field| f.as_ref().and_then(json_rate).map(|v| v / 1_000_000.0);
                let mut tiers: Vec<RateTier> = Vec::new();
                for flat in [&cost.context_over_200k, &cost.experimental_over_200k]
                    .into_iter()
                    .flatten()
                {
                    tiers.push(RateTier {
                        size: 200_000,
                        input: per_m(&flat.input),
                        output: per_m(&flat.output),
                        cache_read: per_m(&flat.cache_read),
                        cache_write: per_m(&flat.cache_write),
                    });
                }
                for t in cost.tiers.iter().flatten() {
                    let Some(bound) = t.tier.as_ref() else {
                        continue;
                    };
                    if bound.kind.as_ref().and_then(|v| v.as_str()) != Some("context") {
                        continue;
                    }
                    let Some(size) = bound.size.as_ref().and_then(json_usize) else {
                        continue;
                    };
                    // Same-size replaces (explicit tier wins over the flat
                    // 200k shorthand) — one `size` maps one rate.
                    tiers.retain(|e| e.size != size);
                    tiers.push(RateTier {
                        size,
                        input: per_m(&t.input),
                        output: per_m(&t.output),
                        cache_read: per_m(&t.cache_read),
                        cache_write: per_m(&t.cache_write),
                    });
                }
                let base = match (per_m(&cost.input), per_m(&cost.output)) {
                    (Some(input), Some(output)) => Some(ModelRate {
                        input,
                        output,
                        cache_read: per_m(&cost.cache_read).unwrap_or(0.0),
                        cache_write: per_m(&cost.cache_write).unwrap_or(0.0),
                        // Cache-aware only when both sides are priced —
                        // a read-only price would bill writes at input rate.
                        has_cache_prices: per_m(&cost.cache_read).is_some()
                            && per_m(&cost.cache_write).is_some(),
                        tiers: Vec::new(),
                    }),
                    _ => None,
                };
                cache_models_dev_rate(key, base.clone(), tiers.clone());
                if let Some((_, suffix)) = key.rsplit_once('/') {
                    cache_models_dev_rate(suffix, base, tiers);
                }
            }
        }
    }
    n
}

/// Fetches models.dev's table in the background and caches context windows.
/// Same fire-and-forget contract as `fetch_litellm_context_windows`.
/// Returns the number of models cached (0 on any failure).
pub async fn fetch_models_dev_context() -> usize {
    const URL: &str = "https://models.dev/api.json";
    let n = load_catalog("models.dev.json", URL, apply_models_dev).await;
    if n > 0 {
        save_models_cache_to_disk();
    }
    n
}

fn cache_model_rate_if_absent(model_id: &str, rate: ModelRate) {
    if let Ok(g) = model_rates_cell().read()
        && g.contains_key(model_id)
    {
        return;
    }
    // New rate: merge may stash nothing when tiers predate the base (the
    // models.dev-only-tier case) — fold the pending list in here instead.
    let mut rate = rate;
    merge_tiers(&mut rate.tiers, take_pending_tiers(model_id));
    cache_model_rate(model_id, rate);
}

/// OpenRouter's `/models` carries per-model `pricing` (USD/token as strings).
/// Gap-fill only — LiteLLM stays authoritative; this just covers models too
/// new for LiteLLM's table (e.g. Muse Spark at launch).
pub fn parse_openrouter_models_json(val: &serde_json::Value) -> usize {
    serde::Deserialize::deserialize(val).map_or(0, apply_openrouter)
}

fn apply_openrouter(list: OpenRouter) -> usize {
    fn num(v: &serde_json::Value) -> Option<f64> {
        v.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| v.as_f64())
            .filter(|f| f.is_finite() && *f >= 0.0)
    }
    let mut n = 0;
    for entry in &list.data {
        let (Some(id), Some(pricing)) =
            (entry.id.as_ref().and_then(|v| v.as_str()), &entry.pricing)
        else {
            continue;
        };
        let (Some(input), Some(output)) = (
            pricing.prompt.as_ref().and_then(num),
            pricing.completion.as_ref().and_then(num),
        ) else {
            continue;
        };
        if input == 0.0 && output == 0.0 {
            continue;
        }
        let rate = ModelRate {
            input,
            output,
            cache_read: 0.0,
            cache_write: 0.0,
            has_cache_prices: false,
            tiers: Vec::new(),
        };
        cache_model_rate_if_absent(id, rate.clone());
        let lower = id.to_lowercase();
        cache_model_rate_if_absent(&lower, rate.clone());
        if let Some((_, suffix)) = id.rsplit_once('/') {
            cache_model_rate_if_absent(suffix, rate);
        }
        n += 1;
    }
    n
}

/// Same fire-and-forget contract as `fetch_litellm_context_windows`.
/// No auth needed — OpenRouter's model list is public.
pub async fn fetch_openrouter_rates() -> usize {
    const URL: &str = "https://openrouter.ai/api/v1/models";
    load_catalog("openrouter.json", URL, apply_openrouter).await
}

/// The website's curated connect-id → model-id ranking
/// (`{"version":1,"providers":{"openrouter":[...]}}`; unknown fields
/// ignored). Keys are connect ids, the same ids the picker looks up.
#[derive(serde::Deserialize)]
struct RecommendedModels {
    providers: Option<std::collections::BTreeMap<String, Vec<String>>>,
}

static RECOMMENDED_MODELS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::BTreeMap<String, Vec<String>>>,
> = std::sync::OnceLock::new();

fn recommended_cell() -> &'static std::sync::RwLock<std::collections::BTreeMap<String, Vec<String>>>
{
    RECOMMENDED_MODELS.get_or_init(|| std::sync::RwLock::new(std::collections::BTreeMap::new()))
}

/// Curated model ids for a connect id (e.g. `openrouter`), best first.
/// `None` when the website never published a list — the picker's automatic
/// ranking takes over.
pub fn curated_recommended(connect_id: &str) -> Option<Vec<String>> {
    recommended_cell().read().ok()?.get(connect_id).cloned()
}

fn apply_recommended(list: RecommendedModels) -> usize {
    let providers = list.providers.unwrap_or_default();
    let n = providers.values().map(Vec::len).sum();
    if let Ok(mut g) = recommended_cell().write() {
        *g = providers;
    }
    n
}

/// Fetches the curated recommended-model table in the background. Same
/// fire-and-forget contract as [`fetch_models_dev_context`] (24h disk TTL,
/// stale fallback). A 404 or offline day reads as no curation — returns 0
/// and the automatic ranking covers it.
pub async fn fetch_recommended_models() -> usize {
    let url = std::env::var("GRAY_RECOMMENDED_URL")
        .unwrap_or_else(|_| "https://gray.alignment.id/models/recommended.json".to_string());
    load_catalog("recommended.json", &url, apply_recommended).await
}

/// Catalogs change a few times a day at most; a day-old copy is plenty.
const CATALOG_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Applies a public catalog: the on-disk copy (`~/.gray/cache/<name>`) while
/// it is under a day old, else a fresh download (saved for next launch),
/// else the stale copy so offline still has data.
async fn load_catalog<T: serde::de::DeserializeOwned>(
    name: &str,
    url: &str,
    apply: fn(T) -> usize,
) -> usize {
    let path = crate::setup::catalog::gray_home()
        .ok()
        .map(|h| h.join("cache").join(name));
    let from_disk = |max_age: std::time::Duration| -> Option<T> {
        let p = path.as_ref()?;
        let age = std::fs::metadata(p)
            .ok()?
            .modified()
            .ok()?
            .elapsed()
            .unwrap_or_default();
        (age <= max_age).then_some(())?;
        serde_json::from_slice(&std::fs::read(p).ok()?).ok()
    };
    let catalog = match from_disk(CATALOG_TTL) {
        Some(t) => Some(t),
        None => match fetch_body(url)
            .await
            .and_then(|body| Some((serde_json::from_slice::<T>(body.as_ref()).ok()?, body)))
        {
            Some((t, body)) => {
                if let Some(p) = &path
                    && let Some(dir) = p.parent()
                    && std::fs::create_dir_all(dir).is_ok()
                {
                    let tmp = p.with_extension("json.tmp");
                    if std::fs::write(&tmp, body.as_ref()).is_ok() {
                        let _ = std::fs::rename(&tmp, p);
                    }
                }
                Some(t)
            }
            None => from_disk(std::time::Duration::MAX),
        },
    };
    catalog.map_or(0, apply)
}

async fn fetch_body(url: &str) -> Option<impl AsRef<[u8]>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent(concat!("gray/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.bytes().await.ok()
}

/// A previous session's live/litellm/models.dev values beat the hardcoded
/// guess on cold boot, before any fetch completes.
/// On-disk context cache (`~/.gray/models.json`, `{ "model-id": tokens }`).
fn models_cache_path() -> Option<std::path::PathBuf> {
    crate::setup::catalog::gray_home()
        .ok()
        .map(|h| h.join("models.json"))
}

/// Loads the disk cache into memory (gap-fill, source "disk"). Best-effort.
pub fn load_models_cache_to_memory() -> usize {
    let Some(path) = models_cache_path() else {
        return 0;
    };
    let s = std::fs::read_to_string(path).ok().unwrap_or_default();
    if s.is_empty() {
        return 0;
    }
    let map: std::collections::HashMap<String, usize> =
        serde_json::from_str(&s).unwrap_or_default();
    let mut n = 0;
    for (k, v) in map {
        if v == 0 || get_cached_model_context(&k).is_some() {
            continue;
        }
        cache_model_context_with_source(&k, v, "disk", false);
        n += 1;
    }
    n
}

/// Merges in-memory entries over the disk map. `None` = nothing new (caller
/// must skip the write so mtime stays stable and concurrent boots don't
/// rewrite/race every fetch).
fn merged_models_cache(
    mut disk: std::collections::HashMap<String, usize>,
    mem: impl IntoIterator<Item = (String, usize)>,
) -> Option<std::collections::HashMap<String, usize>> {
    let mut dirty = false;
    for (k, v) in mem {
        if disk.get(&k) != Some(&v) {
            disk.insert(k, v);
            dirty = true;
        }
    }
    if dirty { Some(disk) } else { None }
}

/// Persists the in-memory cache to disk (read-modify-write, best-effort).
/// Called after any successful fetch so cold boot beats the guess.
/// Skips the write when memory adds nothing new; the write itself is atomic
/// tmp-file + rename (delivery.rs precedent).
pub fn save_models_cache_to_disk() {
    let Some(path) = models_cache_path() else {
        return;
    };
    let mem: Vec<(String, usize)> = model_context_cache()
        .read()
        .map(|g| g.iter().map(|(k, v)| (k.clone(), *v)).collect())
        .unwrap_or_default();
    if mem.is_empty() {
        return;
    }
    let disk: std::collections::HashMap<String, usize> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let Some(map) = merged_models_cache(disk, mem) else {
        return;
    };
    let Ok(s) = serde_json::to_string(&map) else {
        return;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    // Audit #18: a shared models.json.tmp lets two concurrent refreshes
    // clobber or rename each other's temporary write; the pid+random
    // suffix makes each rename target distinct.
    let tmp = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    if std::fs::write(&tmp, s).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, path);
}

/// One-shot cold-boot load so disk values are present before first resolve.
/// (The only startup hook reachable without touching other modules.)
pub(crate) fn ensure_disk_loaded() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = load_models_cache_to_memory();
    });
}

/// Disk-only rate priming for callers that price before the spawned catalog
/// fetches land — a startup resume rebuilds session totals ahead of them and
/// would otherwise price every turn $0. Applies `~/.gray/cache/` bodies
/// through the same parsers the fetch tasks run (merge semantics keep
/// re-application idempotent); never touches the network.
pub(crate) fn warm_rates_from_disk_cache() {
    let Some(dir) = crate::setup::catalog::gray_home()
        .ok()
        .map(|h| h.join("cache"))
    else {
        return;
    };
    for (name, apply) in [
        (
            "models.dev.json",
            parse_models_dev_json as fn(&serde_json::Value) -> usize,
        ),
        ("litellm.json", parse_litellm_context_json as _),
        ("openrouter.json", parse_openrouter_models_json as _),
    ] {
        if let Ok(s) = std::fs::read(dir.join(name))
            && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&s)
        {
            apply(&v);
        }
    }
}

#[path = "providers_tests.rs"]
#[cfg(test)]
mod tests;
