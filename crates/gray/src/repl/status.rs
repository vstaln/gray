use super::*;

#[derive(Debug, Default, Clone)]
pub(crate) struct SessionTotals {
    pub(crate) turns: usize,
    pub(crate) input: usize,
    pub(crate) output: usize,
    pub(crate) cost: f64,
    pub(crate) total_duration_ms: u64,
    pub(crate) timed_turns: usize,

    pub(crate) cache_read: usize,

    pub(crate) cache_write: usize,

    pub(crate) saved: f64,

    pub(crate) write_premium: f64,

    pub(crate) cache_reported: bool,

    pub(crate) misses: MissTally,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct MissTally {
    pub(crate) count: usize,
    pub(crate) tokens: usize,
    pub(crate) cost: f64,
    pub(crate) idle: usize,
    pub(crate) model_switch: usize,
    pub(crate) other: usize,
}

impl SessionTotals {
    pub(crate) fn add(
        &mut self,
        usage: &gray_core::event::Usage,
        model: &str,
        duration_ms: Option<u64>,
    ) {
        self.turns += 1;
        self.input += usage.input_tokens;
        self.output += usage.output_tokens;

        let read = usage.cache_read_input_tokens.max(usage.cached_tokens);
        let write = usage.cache_write_input_tokens;
        self.cache_read += read;
        self.cache_write += write;
        self.cache_reported |= read + write > 0;
        if let Some(c) = crate::setup::turn_cost(usage, model) {
            self.cost += c;
        }
        if let Some(rate) = crate::setup::get_model_rate(model)
            && rate.has_cache_prices
        {
            if let (Some(full), Some(actual)) = (
                crate::setup::uncached_cost(usage, model),
                crate::setup::turn_cost(usage, model),
            ) {
                self.saved += full - actual;
            }
            self.write_premium += write as f64 * (rate.cache_write - rate.input);
        }
        if let Some(ms) = duration_ms {
            self.total_duration_ms += ms;
            self.timed_turns += 1;
        }
    }

    pub(crate) fn note_miss(&mut self, m: &crate::cache::CacheMiss) {
        self.misses.count += 1;
        self.misses.tokens += m.missed_tokens;
        self.misses.cost += m.missed_cost;
        if m.model_changed {
            self.misses.model_switch += 1;
        } else if m.idle >= crate::cache::cache_ttl() {
            self.misses.idle += 1;
        } else {
            self.misses.other += 1;
        }
    }

    pub(crate) fn from_entries(
        entries: &[crate::session_store::SessionEntry],
        model: &str,
    ) -> Self {
        crate::setup::context::warm_rates_from_disk_cache();
        let mut t = SessionTotals::default();
        for e in entries.iter().filter(|e| e.usage.is_some()) {
            let u = e.usage.as_ref().expect("filtered");
            t.add(u, model, e.duration_ms);
        }
        t
    }
}

pub(crate) fn turn_tokens_per_second(output_tokens: usize, duration_ms: u64) -> Option<u64> {
    if output_tokens == 0 || duration_ms == 0 {
        return None;
    }
    Some((output_tokens as f64 * 1000.0 / duration_ms as f64).round() as u64)
}

pub(crate) fn turn_footer(
    usage: &gray_core::event::Usage,
    model: &str,
    totals: &SessionTotals,
    duration_ms: Option<u64>,
    streamed_ms: Option<u64>,
) -> String {
    let base = format!("\u{2b22} {} tokens", crate::repl::fmt_usage(usage.total()));
    let rate = streamed_ms
        .and_then(|ms| turn_tokens_per_second(usage.output_tokens, ms))
        .map(|t| format!(" · {t} tps"))
        .unwrap_or_default();
    let time = duration_ms
        .map(|ms| format!(" · {}", crate::repl::format::fmt_duration_ms(ms)))
        .unwrap_or_default();
    match crate::setup::turn_cost(usage, model) {
        Some(c) if totals.turns > 1 => format!(
            "{base}{rate}{time} · {} ({} session)",
            crate::setup::format_cost(c),
            crate::setup::format_cost(totals.cost)
        ),
        Some(c) => format!("{base}{rate}{time} · {}", crate::setup::format_cost(c)),
        None => format!("{base}{rate}{time}"),
    }
}

pub(crate) fn handle_copy(agent: &Option<Agent>, tui: Option<&crate::composer::SharedTui>) {
    let text = agent
        .as_ref()
        .and_then(|a| {
            a.messages()
                .iter()
                .rev()
                .find(|m| m.role == gray_core::Role::Assistant)
                .map(|m| m.text_content())
        })
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let Some(text) = text else {
        super::say(
            tui,
            "nothing to copy yet — no assistant response this session",
        );
        return;
    };
    if copy_to_clipboard(&text) {
        super::say(
            tui,
            &format!("copied {} chars to clipboard", text.chars().count()),
        );
    } else {
        super::say(tui, &format!("no clipboard available — response:\n{text}"));
    }
}

fn copy_to_clipboard(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    let helpers: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(target_os = "windows")]
    let helpers: &[(&str, &[&str])] = &[("clip", &[])];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let helpers: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (bin, args) in helpers {
        if std::process::Command::new(bin)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin
                    .take()
                    .map(|mut s| s.write_all(text.as_bytes()).is_ok())
                    .unwrap_or(false)
                    .then(|| c.wait())
                    .transpose()
                    .map(|st| st.is_some_and(|s| s.success()))
            })
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

pub(crate) fn handle_usage(
    totals: &SessionTotals,
    config: &Config,
    tui: Option<&crate::composer::SharedTui>,
) {
    handle_usage_inner(totals, config, tui, true)
}

pub(crate) fn handle_turn_usage(
    totals: &SessionTotals,
    config: &Config,
    tui: Option<&crate::composer::SharedTui>,
) {
    if totals.turns == 0 {
        say(tui, "usage appears after the current turn");
        return;
    }
    handle_usage_inner(totals, config, tui, false)
}

fn handle_usage_inner(
    totals: &SessionTotals,
    config: &Config,
    tui: Option<&crate::composer::SharedTui>,
    include_subscriptions: bool,
) {
    let subs = include_subscriptions
        .then(collect_subscription_usage)
        .unwrap_or_default();
    if totals.turns == 0 && subs.is_empty() {
        say(
            tui,
            "no turns yet this session — usage appears after the first turn",
        );
        return;
    }
    let model = config.model.as_deref().unwrap_or("no model");
    let rate = crate::setup::get_model_rate(config.model.as_deref().unwrap_or(""));
    use usage_panel::{PanelInput, Warmth};
    if let Some(shared) = tui {
        if include_subscriptions {
            let bg = shared.lock().expect("tui lock").snapshot();
            let mut rebuild = || {
                let warmth = {
                    let t = shared.lock().expect("tui lock");
                    match t.cache_remaining() {
                        Some(left) => Warmth::Warm(left),
                        None if t.cache_is_cold() => Warmth::Cold,
                        None => Warmth::Unknown,
                    }
                };
                let input = PanelInput {
                    totals,
                    model,
                    rate: rate.clone(),
                    warmth,
                };
                usage_panel::usage_pages(&input, &collect_subscription_usage())
            };
            let opened =
                super::with_modal_sync(tui, || usage_panel::run_usage_modal(&mut rebuild, &bg));
            if opened.is_ok() {
                return;
            }

            for p in rebuild() {
                let mut t = shared.lock().expect("tui lock");
                t.push_action(&p.title, None);
                t.push_styled_lines_with_hyperlinks(p.lines, &[], 0);
                t.ensure_gap();
            }
            return;
        }
        let mut t = shared.lock().expect("tui lock");
        if totals.turns > 0 {
            let warmth = match t.cache_remaining() {
                Some(left) => Warmth::Warm(left),
                None if t.cache_is_cold() => Warmth::Cold,
                None => Warmth::Unknown,
            };
            let input = PanelInput {
                totals,
                model,
                rate,
                warmth,
            };
            t.push_action("Session usage", Some(&usage_panel::usage_header(&input)));
            t.push_styled_lines_with_hyperlinks(usage_panel::usage_rows(&input), &[], 0);
        }
        t.ensure_gap();
    } else {
        if totals.turns > 0 {
            let input = PanelInput {
                totals,
                model,
                rate,
                warmth: Warmth::Unknown,
            };
            println!("✓ Session usage — {}", usage_panel::usage_header(&input));
            for line in usage_panel::usage_plain(&input) {
                println!("{line}");
            }
        }
        for (title, limits) in subs {
            println!(
                "✓ {}{}",
                title,
                limits
                    .plan
                    .as_deref()
                    .map(|p| format!(" — {p}"))
                    .unwrap_or_default()
            );
            for line in usage_panel::subscription_plain(&limits) {
                println!("{line}");
            }
        }
    }
}

fn collect_subscription_usage() -> Vec<(String, gray_plugin::ProviderUsageLimits)> {
    use std::collections::BTreeSet;
    let Ok(home) = crate::setup::gray_home() else {
        return Vec::new();
    };
    let connected: BTreeSet<String> = crate::setup::catalog::auth_store_path()
        .ok()
        .and_then(|p| crate::auth::CredentialStore::new(p).load().ok())
        .map(|rows| rows.into_keys().collect())
        .unwrap_or_default();
    let installed = crate::providers::ProviderRegistry::load_cached(&home)
        .installed()
        .into_iter()
        .filter(|i| connected.contains(&i.auth_ref()))
        .collect::<Vec<_>>();
    std::thread::scope(|s| {
        let handles: Vec<_> = installed
            .into_iter()
            .map(|i| {
                let i = i.clone();
                s.spawn(move || (i.clone(), plugin_usage_rpc(&i)))
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(|(i, limits)| {
                limits.map(|l| {
                    (
                        l.title.clone().unwrap_or_else(|| i.provider.name.clone()),
                        l,
                    )
                })
            })
            .collect()
    })
}

fn plugin_usage_rpc(
    installed: &crate::providers::InstalledProvider,
) -> Option<gray_plugin::ProviderUsageLimits> {
    use crate::providers::registry::ProviderRpc;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        let runtime = crate::providers::ProviderRuntime::start(installed.clone())
            .await
            .ok()?;
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
            })?;
        let limits = runtime
            .rpc()
            .usage(gray_plugin::ProviderUsageRequest {
                provider: installed.provider.id.clone(),
                auth_method: installed.auth_method.id.clone(),
                profile_binding: installed.profile_binding.clone(),
                credential,
            })
            .await
            .ok()?;
        limits.available.then_some(limits)
    })
}

pub(crate) async fn handle_context_window(
    config: &mut Config,
    cwd: &Path,
    agent: &Option<Agent>,
    direct: Option<String>,
    tui: Option<&crate::composer::SharedTui>,
) {
    fn emit(msg: String, tui: Option<&crate::composer::SharedTui>, ok: bool) {
        if let Some(shared) = tui {
            let mut t = shared.lock().expect("tui lock");
            if ok {
                t.push_action("Context updated", Some(&msg));
            } else {
                super::cron::push_notice(&mut t, "Context update failed", &msg, true);
            }
            t.ensure_gap();
            let _ = t.draw();
        } else if ok {
            println!("✓ Context updated: {msg}");
        } else {
            println!("{msg}");
        }
    }
    fn collect_parts(
        cwd: &Path,
        agent: &Option<Agent>,
        tui: Option<&crate::composer::SharedTui>,
    ) -> crate::setup::ContextParts {
        let messages: &[Message] = agent.as_ref().map(|a| a.messages()).unwrap_or(&[]);
        let latest = tui.and_then(|t| t.lock().ok().and_then(|g| g.latest_usage));
        collect_context_parts(cwd, messages, latest)
    }
    fn breakdown_text(config: &Config, parts: &crate::setup::ContextParts) -> String {
        context_breakdown_text(config, parts)
    }
    let Some(val) = direct else {
        if tui.is_some() {
            let model = config.model.clone().unwrap_or_default();
            let breakdown = collect_parts(cwd, agent, tui);
            let bg = tui.map(|s| s.lock().expect("tui lock").snapshot());
            let res = with_modal_sync(tui, || {
                crate::setup::run_context_modal(config, &breakdown, &model, bg.as_ref())
            });
            match res {
                Ok(Some(summary)) => emit(summary, tui, true),
                Ok(None) => {
                    if let Some(shared) = tui {
                        shared.lock().expect("tui lock").ensure_gap();
                    }
                }
                Err(e) => emit(format!("context error: {e}"), tui, false),
            }
        } else {
            emit(
                breakdown_text(config, &collect_parts(cwd, agent, tui)),
                tui,
                false,
            );
        }
        return;
    };
    let lower = val.trim().to_lowercase();
    if lower.is_empty() || lower == "status" || lower == "show" {
        emit(
            breakdown_text(config, &collect_parts(cwd, agent, tui)),
            tui,
            false,
        );
        return;
    }
    let (msg, ok) = apply_context_arg(config, &val);
    emit(msg, tui, ok);
}

pub(crate) fn persist_context_window(config: &Config) {
    if let Ok(path) = crate::setup::saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        let mut saved = crate::setup::load_saved_config_at(&path);
        saved.context_window = config.context_window;
        saved.context_reserve = config.context_reserve;
        saved.context_keep = config.context_keep;
        let _ = crate::setup::save_saved_config_at(&path, &saved);
    }
}

pub(crate) fn collect_context_parts(
    cwd: &Path,
    messages: &[Message],
    latest: Option<gray_core::event::Usage>,
) -> crate::setup::ContextParts {
    let sys = crate::sys_prompt_path()
        .ok()
        .and_then(|p| load_or_create_system_prompt_at(&p).ok())
        .map(|s| crate::setup::estimate_str_tokens(&crate::system_prompt::strip_comments(&s)))
        .unwrap_or(0);
    let skills = crate::setup::estimate_str_tokens(&crate::skills::format_skills_for_prompt(
        &crate::skills::discover_skills(cwd).skills,
        crate::setup::skills_auto_enabled(),
        &crate::setup::disabled_skill_names(),
    ));
    let tools_toks = serde_json::to_string(&crate::profile::builtin_registry().defs())
        .map(|s| crate::setup::estimate_str_tokens(&s))
        .unwrap_or(0);

    let history_est = messages.iter().map(crate::compact::estimate_tokens).sum();
    let messages = match latest.map(|u| u.total()).filter(|t| *t > 0) {
        Some(total) => total.saturating_sub(sys.saturating_add(tools_toks)),
        None => history_est,
    };
    let project = crate::skills_tool::project_context_block(cwd)
        .map(|b| crate::setup::estimate_str_tokens(&b))
        .unwrap_or(0);
    crate::setup::ContextParts {
        system_prompt: sys,
        project_context: project,
        tools: tools_toks,
        skills,
        messages,
    }
}

pub(crate) fn context_breakdown_text(
    config: &Config,
    parts: &crate::setup::ContextParts,
) -> String {
    let model = config.model.as_deref().unwrap_or("");
    let window = crate::setup::resolve_model_context_length(model);
    let max = crate::setup::model_max_context(model);
    let source = crate::setup::context_source(model);
    let reserve = crate::setup::user_reserve_tokens_for(window);
    let keep = crate::setup::user_keep_for(window);
    let used = parts.used();
    let free = parts.free(window, reserve);

    let compact_at = crate::compact::effective_compact_ceiling(config);
    let pct = |n: usize| {
        n.checked_mul(100)
            .and_then(|v| v.checked_div(window))
            .unwrap_or(0)
    };
    let f = crate::setup::format_context_length;
    let model_label = if model.is_empty() { "no model" } else { model };
    format!(
        "Context Usage · {model_label}\n\
         {}/{} tokens ({}%) — source: {source} (max {})\n\
         \n\
         Estimated usage by category:\n\
           · System prompt:      {} tokens ({}%)\n\
           · Project context:    {} tokens ({}%)\n\
           · System tools:       {} tokens ({}%)\n\
           · Skills:             {} tokens ({}%)\n\
           · Messages:           {} tokens ({}%)\n\
           · Free space:         {} ({}%)\n\
           · Autocompact buffer: {} tokens ({}%)\n\
           · Compacts at:         {} tokens ({}%)\n\
         \n\
         Settings: reserve: {}  keep: {}\n\
         Commands: /context <size> | /context reserve <size> | /context keep <size> | /context auto",
        f(used),
        f(window),
        pct(used),
        f(max),
        f(parts.system_prompt),
        pct(parts.system_prompt),
        f(parts.project_context),
        pct(parts.project_context),
        f(parts.tools),
        pct(parts.tools),
        f(parts.skills),
        pct(parts.skills),
        f(parts.messages),
        pct(parts.messages),
        f(free),
        pct(free),
        f(reserve),
        pct(reserve),
        f(compact_at),
        pct(compact_at),
        f(reserve),
        f(keep),
    )
}

pub(crate) fn apply_context_arg(config: &mut Config, val: &str) -> (String, bool) {
    let lower = val.trim().to_lowercase();
    if lower == "auto" || lower == "clear" || lower == "reset" || lower == "0" {
        config.context_window = None;
        crate::setup::set_user_context_window(None);
        persist_context_window(config);

        if let Some(m) = config.model.clone()
            && crate::setup::get_cached_model_context(&m).is_none()
        {
            let base = config.base_url.clone();
            let key = config.api_key.clone();
            tokio::task::spawn_blocking(move || {
                crate::setup::fetch_live_provider_models(&base, key.as_deref());
            });
        }
        let effective =
            crate::setup::resolve_model_context_length(config.model.as_deref().unwrap_or(""));
        return (
            format!(
                "window reset to auto ({} / {})",
                effective,
                crate::setup::format_context_length(effective)
            ),
            true,
        );
    }

    let mut parts = lower.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    if head == "reserve" || head == "keep" {
        let is_reserve = head == "reserve";
        if rest.is_empty() || rest == "status" || rest == "show" {
            let window =
                crate::setup::resolve_model_context_length(config.model.as_deref().unwrap_or(""));
            let cur = if is_reserve {
                crate::setup::user_reserve_tokens_for(window)
            } else {
                crate::setup::user_keep_for(window)
            };
            return (
                format!(
                    "{head}: {} ({})",
                    cur,
                    crate::setup::format_context_length(cur)
                ),
                false,
            );
        }
        if rest == "auto" || rest == "clear" || rest == "reset" || rest == "default" {
            if is_reserve {
                config.context_reserve = None;
                crate::setup::set_user_reserve_tokens(None);
            } else {
                config.context_keep = None;
                crate::setup::set_user_keep_recent_tokens(None);
            }
            persist_context_window(config);
            return (format!("{head} cleared → auto"), true);
        }
        if !is_reserve && (rest == "off" || rest == "0") {
            config.context_keep = Some(0);
            crate::setup::set_user_keep_recent_tokens(Some(0));
            persist_context_window(config);
            return ("keep set to 0 (summary only)".to_string(), true);
        }
        return match crate::setup::parse_context_window(rest) {
            Some(n) if n > 0 => {
                if is_reserve {
                    config.context_reserve = Some(n);
                    crate::setup::set_user_reserve_tokens(Some(n));
                } else {
                    config.context_keep = Some(n);
                    crate::setup::set_user_keep_recent_tokens(Some(n));
                }
                persist_context_window(config);
                (
                    format!(
                        "{head} set to {n} tokens ({})",
                        crate::setup::format_context_length(n)
                    ),
                    true,
                )
            }
            _ => (
                format!("invalid value '{rest}' — use e.g. 16k, 20000, or 'auto' to clear"),
                false,
            ),
        };
    }
    match crate::setup::parse_context_window(val) {
        Some(n) if n > 0 => {
            let model = config.model.as_deref().unwrap_or("");
            let max = crate::setup::model_max_context(model);
            let (final_n, clamped) = if n > max { (max, true) } else { (n, false) };
            config.context_window = Some(final_n);
            crate::setup::set_user_context_window(Some(final_n));
            persist_context_window(config);
            let msg = if clamped {
                format!(
                    "window clamped to model max {} ({}) — requested {}",
                    final_n,
                    crate::setup::format_context_length(final_n),
                    crate::setup::format_context_length(n)
                )
            } else {
                format!(
                    "window set to {} tokens ({})",
                    final_n,
                    crate::setup::format_context_length(final_n)
                )
            };
            (msg, true)
        }
        _ => (
            format!("invalid value '{val}' — use e.g. 128000, 128k, 1m, or 'auto' to clear"),
            false,
        ),
    }
}

pub(super) fn compaction_card(
    elapsed: &str,
    detail: &str,
) -> (
    ratatui::text::Line<'static>,
    Vec<ratatui::text::Line<'static>>,
) {
    use ratatui::style::Style;
    use ratatui::text::{Line, Span};
    let dim = Style::default().fg(crate::theme::theme().tool_dim);
    let mut spans = super::cron::card_header_spans("Context compacted", String::new(), false);
    spans.push(Span::styled(format!(" \u{b7} {elapsed}"), dim));
    let body = vec![Line::from(Span::styled(format!("  {detail}"), dim))];
    (Line::from(spans), body)
}

pub(super) fn push_compaction_card(
    tui: Option<&crate::composer::SharedTui>,
    elapsed: &str,
    detail: &str,
) {
    match tui {
        Some(shared) => {
            let (header, body) = compaction_card(elapsed, detail);
            shared.lock().expect("tui lock").push_tool_box(header, body);
        }
        None => println!("Context compacted · {elapsed} · {detail}\n"),
    }
}

pub(crate) async fn handle_compact(
    config: &Config,
    cwd: &Path,
    custom_instructions: Option<String>,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    pending_history: &mut std::sync::Arc<Vec<Message>>,
    tui: Option<&crate::composer::SharedTui>,
) {
    if agent.is_none() {
        reload_agent(
            agent,
            config,
            cwd,
            session_state.as_ref().map(|s| s.session_id.as_str()),
            tui,
        )
        .await;
    }
    let Some(ag) = agent.as_mut() else {
        return;
    };

    if ag.messages().is_empty() && !pending_history.is_empty() {
        ag.set_messages(super::take_pending_history(pending_history));
    }

    let messages = ag.messages().to_vec();
    if messages.is_empty() {
        if let Some(shared) = tui {
            super::cron::push_notice(
                &mut shared.lock().expect("tui lock"),
                "Nothing to compact",
                "conversation is empty",
                false,
            );
        } else {
            println!("nothing to compact (conversation is empty)");
        }
        return;
    }

    let msg_count = messages.len();

    let compaction_id = format!("manual-{}", uuid::Uuid::new_v4().as_simple());
    if let Some(shared) = tui {
        shared
            .lock()
            .expect("tui lock")
            .begin_compaction(compaction_id.clone());
    }

    let watch_cancel = tokio_util::sync::CancellationToken::new();
    let watch_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let key_watcher = tui.map(|shared| {
        crate::repl::key_watcher::spawn_key_watcher_with_typing(
            watch_cancel,
            watch_stop.clone(),
            Some(shared.clone()),
            cwd.to_path_buf(),
            None,
        )
    });

    let compact_res = crate::compact::compact_with_keep(
        ag,
        custom_instructions.as_deref(),
        crate::setup::user_keep_recent_tokens(),
    )
    .await;

    watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(watcher) = key_watcher {
        let _ = watcher.await;
    }

    let elapsed = tui
        .and_then(|shared| {
            shared
                .lock()
                .expect("tui lock")
                .finish_compaction(&compaction_id, None)
        })
        .unwrap_or_default();
    let elapsed_str = crate::composer::fmt_elapsed_compact(elapsed.as_secs());

    match compact_res {
        Ok(Some(summary)) => {
            if let Some(state) = session_state {
                let _ = state
                    .store
                    .append_compaction_replacement(&state.session_id, ag.messages())
                    .await;
            }

            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                let est = crate::compact::estimate_context_tokens(ag.messages(), None);
                t.seed_estimate_usage(est);

                t.reset_cache();
            }

            if let Some(shared) = tui {
                let mut tui = shared.lock().expect("tui lock");
                let (header, body) = compaction_card(
                    &elapsed_str,
                    &format!("{msg_count} messages \u{2192} summary"),
                );
                tui.push_tool_box(header, body);
                tui.push_compaction_summary(&summary);
                tui.ensure_gap();
            } else {
                println!("Context compacted · {elapsed_str} ({msg_count} messages -> summary)\n");
                println!("{summary}\n");
            }
        }
        Ok(None) => {
            if let Some(shared) = tui {
                super::cron::push_notice(
                    &mut shared.lock().expect("tui lock"),
                    "Nothing to compact",
                    "conversation already fits in context",
                    false,
                );
            } else {
                println!("nothing to compact (conversation already fits in context)");
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                super::cron::push_notice(
                    &mut shared.lock().expect("tui lock"),
                    "Compaction failed",
                    &e.to_string(),
                    true,
                );
            } else {
                println!("compaction failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod compaction_card_tests {
    use super::compaction_card;

    fn text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn compaction_card_is_a_bullet_header_with_one_detail_row() {
        let (header, body) = compaction_card("9s", "282.7k/256k tokens");
        assert_eq!(text(&header), "\u{2b22} Context compacted \u{b7} 9s");
        assert_eq!(body.len(), 1);
        assert_eq!(text(&body[0]), "  282.7k/256k tokens");
        assert!(!text(&header).contains('└'));
    }
}
