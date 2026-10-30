use super::*;

pub(super) type Field = Option<serde_json::Value>;

#[derive(serde::Deserialize)]
pub struct LiteLlmEntry {
    pub(super) max_input_tokens: Field,
    pub(super) max_tokens: Field,
    pub(super) input_cost_per_token: Field,
    pub(super) output_cost_per_token: Field,
    pub(super) cache_read_input_token_cost: Field,
    pub(super) cache_creation_input_token_cost: Field,

    pub(super) input_cost_per_token_above_200k_tokens: Field,
    pub(super) output_cost_per_token_above_200k_tokens: Field,
    pub(super) cache_read_input_token_cost_above_200k_tokens: Field,
    pub(super) cache_creation_input_token_cost_above_200k_tokens: Field,
}

pub(super) type LiteLlm = std::collections::BTreeMap<String, LiteLlmEntry>;

#[derive(serde::Deserialize)]
pub struct ModelsDevProvider {
    pub(super) models: Option<std::collections::BTreeMap<String, ModelsDevEntry>>,
}

#[derive(serde::Deserialize)]
pub struct ModelsDevEntry {
    pub(super) limit: Option<ModelsDevLimit>,
    pub(super) context_window: Field,
    pub(super) max_input_tokens: Field,
    pub(super) reasoning: Field,
    pub(super) reasoning_options: Option<Vec<ModelsDevOption>>,

    pub(super) cost: Option<ModelsDevCost>,

    pub(super) tool_call: Field,
    pub(super) modalities: Option<ModelsDevModalities>,
    pub(super) release_date: Field,
    pub(super) family: Field,
}

#[derive(serde::Deserialize)]
pub struct ModelsDevModalities {
    pub(super) output: Field,
}

#[derive(serde::Deserialize)]
pub struct ModelsDevCost {
    pub(super) input: Field,
    pub(super) output: Field,
    pub(super) cache_read: Field,
    pub(super) cache_write: Field,
    pub(super) tiers: Option<Vec<ModelsDevCostTier>>,

    pub(super) context_over_200k: Option<ModelsDevTierRates>,
    #[serde(alias = "experimentalOver200K")]
    pub(super) experimental_over_200k: Option<ModelsDevTierRates>,
}

#[derive(serde::Deserialize)]
pub struct ModelsDevCostTier {
    pub(super) input: Field,
    pub(super) output: Field,
    pub(super) cache_read: Field,
    pub(super) cache_write: Field,
    pub(super) tier: Option<ModelsDevTierBound>,
}

#[derive(serde::Deserialize)]
pub struct ModelsDevTierBound {
    #[serde(rename = "type")]
    pub(super) kind: Field,
    pub(super) size: Field,
}

#[derive(serde::Deserialize)]
pub struct ModelsDevTierRates {
    pub(super) input: Field,
    pub(super) output: Field,
    pub(super) cache_read: Field,
    pub(super) cache_write: Field,
}

#[derive(serde::Deserialize)]
pub struct ModelsDevLimit {
    pub(super) context: Field,
}

#[derive(serde::Deserialize)]
pub struct ModelsDevOption {
    #[serde(rename = "type")]
    pub(super) kind: Field,
    pub(super) values: Field,
}

pub(super) type ModelsDev = std::collections::BTreeMap<String, ModelsDevProvider>;

#[derive(serde::Deserialize)]
pub struct OpenRouter {
    pub(super) data: Vec<OpenRouterEntry>,
}

#[derive(serde::Deserialize)]
pub struct OpenRouterEntry {
    pub(super) id: Field,
    pub(super) pricing: Option<OpenRouterPricing>,
}

#[derive(serde::Deserialize)]
pub struct OpenRouterPricing {
    pub(super) prompt: Field,
    pub(super) completion: Field,
}

pub fn parse_litellm_context_json(val: &serde_json::Value) -> usize {
    serde::Deserialize::deserialize(val).map_or(0, apply_litellm)
}

pub(super) fn apply_litellm(map: LiteLlm) -> usize {
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

            if let Some((_, suffix)) = key.rsplit_once('/') {
                cache_model_context_if_absent(suffix, len);
            }
            n += 1;
        }

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

pub async fn fetch_litellm_context_windows() {
    const URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
    if load_catalog("litellm.json", URL, apply_litellm).await > 0 {
        save_models_cache_to_disk();
    }
}

pub fn parse_models_dev_json(val: &serde_json::Value) -> usize {
    serde::Deserialize::deserialize(val).map_or(0, apply_models_dev)
}

pub(super) fn apply_models_dev(providers: ModelsDev) -> usize {
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

            if let Some(r) = entry.reasoning.as_ref().and_then(|v| v.as_bool()) {
                cache_model_reasoning(key, r);
            }

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

pub async fn fetch_models_dev_context() -> usize {
    const URL: &str = "https://models.dev/api.json";
    let n = load_catalog("models.dev.json", URL, apply_models_dev).await;
    if n > 0 {
        save_models_cache_to_disk();
    }
    n
}

pub(super) fn cache_model_rate_if_absent(model_id: &str, rate: ModelRate) {
    if let Ok(g) = model_rates_cell().read()
        && g.contains_key(model_id)
    {
        return;
    }

    let mut rate = rate;
    merge_tiers(&mut rate.tiers, take_pending_tiers(model_id));
    cache_model_rate(model_id, rate);
}

pub fn parse_openrouter_models_json(val: &serde_json::Value) -> usize {
    serde::Deserialize::deserialize(val).map_or(0, apply_openrouter)
}

pub(super) fn apply_openrouter(list: OpenRouter) -> usize {
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

pub async fn fetch_openrouter_rates() -> usize {
    const URL: &str = "https://openrouter.ai/api/v1/models";
    load_catalog("openrouter.json", URL, apply_openrouter).await
}

#[derive(serde::Deserialize)]
pub(super) struct RecommendedModels {
    pub(super) providers: Option<std::collections::BTreeMap<String, Vec<String>>>,
}

pub(super) static RECOMMENDED_MODELS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::BTreeMap<String, Vec<String>>>,
> = std::sync::OnceLock::new();

pub(super) fn recommended_cell()
-> &'static std::sync::RwLock<std::collections::BTreeMap<String, Vec<String>>> {
    RECOMMENDED_MODELS.get_or_init(|| std::sync::RwLock::new(std::collections::BTreeMap::new()))
}

pub fn curated_recommended(connect_id: &str) -> Option<Vec<String>> {
    recommended_cell().read().ok()?.get(connect_id).cloned()
}

pub(super) fn apply_recommended(list: RecommendedModels) -> usize {
    let providers = list.providers.unwrap_or_default();
    let n = providers.values().map(Vec::len).sum();
    if let Ok(mut g) = recommended_cell().write() {
        *g = providers;
    }
    n
}

pub async fn fetch_recommended_models() -> usize {
    let url = std::env::var("GRAY_RECOMMENDED_URL")
        .unwrap_or_else(|_| "https://gray.alignment.id/models/recommended.json".to_string());
    load_catalog("recommended.json", &url, apply_recommended).await
}
