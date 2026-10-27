use super::context::ModelMeta;

const UNREADY_WORDS: &[&str] = &["preview", "experimental", "-exp", "beta", "batch"];

pub(crate) fn recommended_ids(
    models: &[(String, String)],
    curated: Option<&[String]>,
    meta: impl Fn(&str) -> Option<ModelMeta>,
    limit: usize,
) -> Vec<String> {
    if let Some(curated) = curated {
        let picked: Vec<String> = curated
            .iter()
            .filter(|id| models.iter().any(|(m, _)| m == *id))
            .take(limit)
            .cloned()
            .collect();
        if !picked.is_empty() {
            return picked;
        }
    }

    let mut winners: Vec<(String, String, String)> = Vec::new();
    for (id, name) in models {
        let lower_id = id.to_lowercase();
        let lower_name = name.to_lowercase();
        if UNREADY_WORDS
            .iter()
            .any(|w| lower_id.contains(w) || lower_name.contains(w))
        {
            continue;
        }
        let Some(m) = meta(id) else {
            continue;
        };
        if m.tool_call != Some(true) || m.text_only != Some(true) {
            continue;
        }
        let family = m.family.clone().unwrap_or_else(|| id.clone());
        let date = m.release_date.clone().unwrap_or_default();
        match winners.iter_mut().find(|(f, ..)| *f == family) {
            Some((_, cur_id, cur_date)) => {
                if date > *cur_date || (date == *cur_date && *id < *cur_id) {
                    *cur_id = id.clone();
                    *cur_date = date;
                }
            }
            None => winners.push((family, id.clone(), date)),
        }
    }
    winners.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    winners.truncate(limit);
    winners.into_iter().map(|(_, id, _)| id).collect()
}
