//! Session status: totals, usage, context window, compact (split from `repl`).

use super::*;

/// Running token + cost totals for the current process session (reset on `/new`).
#[derive(Debug, Default)]
pub(crate) struct SessionTotals {
    pub(crate) turns: usize,
    pub(crate) input: usize,
    pub(crate) output: usize,
    pub(crate) cost: f64,
    pub(crate) total_duration_ms: u64,
    pub(crate) timed_turns: usize,
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
        if let Some(c) = crate::setup::turn_cost(usage, model) {
            self.cost += c;
        }
        if let Some(ms) = duration_ms {
            self.total_duration_ms += ms;
            self.timed_turns += 1;
        }
    }

    /// Rebuilds totals from stored session entries. Usage is recorded on the
    /// last message of each turn, so entries carrying usage map 1:1 to turns.
    pub(crate) fn from_entries(entries: &[gray_session::SessionEntry], model: &str) -> Self {
        let mut t = SessionTotals::default();
        for e in entries.iter().filter(|e| e.usage.is_some()) {
            let u = e.usage.as_ref().expect("filtered");
            t.add(u, model, e.duration_ms);
        }
        t
    }
}

/// `⬡ 12,400 tok · 6s · $0.004 ($0.41 session)` — cost/time parts appear only
/// when known; otherwise the footer stays tokens-only as before.
pub(crate) fn turn_footer(
    usage: &gray_core::event::Usage,
    model: &str,
    totals: &SessionTotals,
    duration_ms: Option<u64>,
) -> String {
    let base = format!("\u{2b22} {} tok", crate::repl::fmt_usage(usage.total()));
    let time = duration_ms
        .map(|ms| format!(" · {}", crate::repl::format::fmt_duration_ms(ms)))
        .unwrap_or_default();
    match crate::setup::turn_cost(usage, model) {
        Some(c) if totals.turns > 1 => format!(
            "{base}{time} · {} ({} session)",
            crate::setup::format_cost(c),
            crate::setup::format_cost(totals.cost)
        ),
        Some(c) => format!("{base}{time} · {}", crate::setup::format_cost(c)),
        None => format!("{base}{time}"),
    }
}

/// Handles `/usage` / `/cost`: session totals plus the active model's rate.
/// TUI renders like `/model` — `✓` action header + dim detail lines.
pub(crate) fn handle_usage(
    totals: &SessionTotals,
    config: &Config,
    tui: Option<&crate::composer::SharedTui>,
) {
    if totals.turns == 0 {
        say(
            tui,
            "no turns yet this session — usage appears after the first turn",
        );
        return;
    }
    let model = config.model.as_deref().unwrap_or("no model");
    let header = format!(
        "{model} · {} turn{}",
        totals.turns,
        if totals.turns == 1 { "" } else { "s" }
    );
    let body = format!(
        "{} in · {} out · {} total",
        crate::repl::fmt_usage(totals.input),
        crate::repl::fmt_usage(totals.output),
        crate::repl::fmt_usage(totals.input + totals.output),
    );
    let time_line = if totals.total_duration_ms > 0 && totals.timed_turns > 0 {
        let avg = totals.total_duration_ms / totals.timed_turns as u64;
        Some(format!(
            "{} total · {} avg",
            crate::repl::format::fmt_duration_ms(totals.total_duration_ms),
            crate::repl::format::fmt_duration_ms(avg),
        ))
    } else {
        None
    };
    let cost_line = match crate::setup::get_model_rate(config.model.as_deref().unwrap_or("")) {
        Some(r) => format!(
            "{} @ ${:.2}/${:.2} per 1M in/out",
            crate::setup::format_cost(totals.cost),
            r.input * 1_000_000.0,
            r.output * 1_000_000.0
        ),
        None => "unpriced (no rate yet — pricing tables lag new models)".to_string(),
    };
    if let Some(shared) = tui {
        let mut t = shared.lock().expect("tui lock");
        t.push_action("Session usage", Some(&header));
        t.push_dim(body);
        if let Some(time) = &time_line {
            t.push_dim(time.clone());
        }
        t.push_dim(cost_line);
    } else {
        println!("✓ Session usage — {header}\n  {body}");
        if let Some(time) = &time_line {
            println!("  {time}");
        }
        println!("  {cost_line}");
    }
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
            if ok {
                let mut t = shared.lock().expect("tui lock");
                t.push_dim(format!("└ {msg}"));
                let _ = t.draw();
            } else {
                shared
                    .lock()
                    .expect("tui lock")
                    .push_dim(format!("└ {msg}"));
            }
        } else if ok {
            println!("✓ {msg}");
        } else {
            println!("{msg}");
        }
    }
    fn persist_window(config: &Config) {
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            saved.context_window = config.context_window;
            saved.context_reserve = config.context_reserve;
            saved.context_keep = config.context_keep;
            let _ = crate::setup::save_saved_config_at(&path, &saved);
        }
    }
    fn collect_parts(
        cwd: &Path,
        agent: &Option<Agent>,
        tui: Option<&crate::composer::SharedTui>,
    ) -> crate::setup::ContextParts {
        let sys = crate::sys_prompt_path()
            .ok()
            .and_then(|p| load_or_create_system_prompt_at(&p).ok())
            .map(|s| crate::setup::estimate_str_tokens(&s))
            .unwrap_or(0);
        let ctx_bytes: usize = crate::system_prompt::discover_context_files(cwd)
            .iter()
            .map(|f| f.content.len())
            .sum::<usize>();
        let skills = crate::skills::discover_skills(cwd).skills;
        let skills_toks =
            crate::setup::estimate_str_tokens(&crate::skills::format_skills_for_prompt(&skills));
        let tools_toks = serde_json::to_string(&crate::profile::builtin_registry().defs())
            .map(|s| crate::setup::estimate_str_tokens(&s))
            .unwrap_or(0);
        let latest = tui.and_then(|t| t.lock().ok().and_then(|g| g.latest_usage));
        let messages = agent
            .as_ref()
            .map(|a| crate::compact::estimate_context_tokens(a.messages(), latest))
            .unwrap_or(0);
        crate::setup::ContextParts {
            system_prompt: sys,
            project_context: (ctx_bytes as f64 / 4.0).ceil() as usize,
            tools: tools_toks,
            skills: skills_toks,
            messages,
        }
    }
    fn breakdown_text(config: &Config, parts: &crate::setup::ContextParts) -> String {
        let model = config.model.as_deref().unwrap_or("");
        let window = crate::setup::resolve_model_context_length(model);
        let max = crate::setup::model_max_context(model);
        let source = crate::setup::context_source(model);
        let reserve = crate::setup::user_reserve_tokens_for(window);
        let keep = crate::setup::user_keep_for(window);
        let used = parts.used();
        let free = parts.free(window, reserve);
        let pct = |n: usize| {
            n.checked_mul(100)
                .and_then(|v| v.checked_div(window))
                .unwrap_or(0)
        };
        let f = crate::setup::format_context_length;
        let ic = crate::setup::icon;
        // 10x10 hexagon grid, kinds: 0-4 categories, 5 free, 6 buffer.
        let grid_cells = parts.grid_cells(window, reserve);
        let mut flat: Vec<usize> = Vec::with_capacity(100);
        for (kind, n) in grid_cells.iter().enumerate() {
            flat.extend(std::iter::repeat_n(kind, *n));
        }
        while flat.len() < 100 {
            flat.push(5);
        }
        let cell = |kind: usize| match kind {
            0..=4 => ic("cell"),
            5 => ic("cell_free"),
            _ => ic("cell_buffer"),
        };
        let mut grid = String::new();
        for r in 0..10 {
            for c in 0..10 {
                grid.push_str(cell(flat[r * 10 + c]));
                grid.push(' ');
            }
            grid.push('\n');
        }
        format!(
            "Context Usage\n{} · {}/{} tokens ({}%) — source: {source} / max {} ({})\n{grid}\nEstimated usage by category\n{} System prompt: {} tokens ({}%)\n{} Project context: {} tokens ({}%)\n{} System tools: {} tokens ({}%)\n{} Skills: {} tokens ({}%)\n{} Messages: {} tokens ({}%)\n{} Free space: {} ({}%)\n{} Autocompact buffer: {} tokens ({}%)\n  reserve: {}  keep: {}\n  set: /context 128k | /context reserve 16k | /context keep 20k  |  clear: /context auto",
            if model.is_empty() { "no model" } else { model },
            f(used),
            f(window),
            pct(used),
            max,
            f(max),
            ic("cell"),
            f(parts.system_prompt),
            pct(parts.system_prompt),
            ic("cell"),
            f(parts.project_context),
            pct(parts.project_context),
            ic("cell"),
            f(parts.tools),
            pct(parts.tools),
            ic("cell"),
            f(parts.skills),
            pct(parts.skills),
            ic("cell"),
            f(parts.messages),
            pct(parts.messages),
            ic("cell_free"),
            f(free),
            pct(free),
            ic("cell_buffer"),
            f(reserve),
            pct(reserve),
            f(reserve),
            f(keep),
        )
    }
    let Some(val) = direct else {
        // Bare `/context`: modal in the TUI, static breakdown on pipes.
        if tui.is_some() {
            let model = config.model.clone().unwrap_or_default();
            let breakdown = collect_parts(cwd, agent, tui);
            let bg = tui.map(|s| s.lock().expect("tui lock").snapshot());
            let res = with_modal_sync(tui, || {
                crate::setup::run_context_modal(config, &breakdown, &model, bg.as_ref())
            });
            match res {
                Ok(true) => emit(
                    breakdown_text(config, &collect_parts(cwd, agent, tui)),
                    tui,
                    false,
                ),
                Ok(false) => {}
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
    if lower == "auto" || lower == "clear" || lower == "reset" || lower == "0" {
        config.context_window = None;
        crate::setup::set_user_context_window(None);
        persist_window(config);
        // re-prime cache if model present
        if let Some(m) = config.model.clone()
            && crate::setup::get_cached_model_context(&m).is_none()
        {
            let base = config.base_url.clone();
            let key = config.api_key.clone();
            tokio::spawn(async move {
                crate::setup::fetch_live_provider_models(&base, key.as_deref());
            });
        }
        let effective =
            crate::setup::resolve_model_context_length(config.model.as_deref().unwrap_or(""));
        emit(
            format!(
                "context window cleared → auto ({} / {})",
                effective,
                crate::setup::format_context_length(effective)
            ),
            tui,
            true,
        );
        return;
    }
    // Subcommands: `reserve <n|auto|default>` and `keep <n|auto|default|off`
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
            emit(
                format!(
                    "{head}: {} ({})",
                    cur,
                    crate::setup::format_context_length(cur)
                ),
                tui,
                false,
            );
            return;
        }
        if rest == "auto" || rest == "clear" || rest == "reset" || rest == "default" {
            if is_reserve {
                config.context_reserve = None;
                crate::setup::set_user_reserve_tokens(None);
            } else {
                config.context_keep = None;
                crate::setup::set_user_keep_recent_tokens(None);
            }
            persist_window(config);
            emit(format!("{head} cleared → auto"), tui, true);
            return;
        }
        if !is_reserve && (rest == "off" || rest == "0") {
            config.context_keep = Some(0);
            crate::setup::set_user_keep_recent_tokens(Some(0));
            persist_window(config);
            emit("keep set to 0 (summary only)".to_string(), tui, true);
            return;
        }
        match crate::setup::parse_context_window(rest) {
            Some(n) if n > 0 => {
                if is_reserve {
                    config.context_reserve = Some(n);
                    crate::setup::set_user_reserve_tokens(Some(n));
                } else {
                    config.context_keep = Some(n);
                    crate::setup::set_user_keep_recent_tokens(Some(n));
                }
                persist_window(config);
                emit(
                    format!(
                        "{head} set to {n} tokens ({})",
                        crate::setup::format_context_length(n)
                    ),
                    tui,
                    true,
                );
            }
            _ => emit(
                format!("invalid value '{rest}' — use e.g. 16k, 20000, or 'auto' to clear"),
                tui,
                false,
            ),
        }
        return;
    }
    match crate::setup::parse_context_window(&val) {
        Some(n) if n > 0 => {
            let model = config.model.as_deref().unwrap_or("");
            let max = crate::setup::model_max_context(model);
            let (final_n, clamped) = if n > max { (max, true) } else { (n, false) };
            config.context_window = Some(final_n);
            crate::setup::set_user_context_window(Some(final_n));
            persist_window(config);
            let msg = if clamped {
                format!(
                    "context window clamped to model max {} ({}) — requested {}",
                    final_n,
                    crate::setup::format_context_length(final_n),
                    crate::setup::format_context_length(n)
                )
            } else {
                format!(
                    "context window set to {} tokens ({}) — overrides auto-fetched value",
                    final_n,
                    crate::setup::format_context_length(final_n)
                )
            };
            emit(msg, tui, true);
        }
        _ => {
            emit(
                format!("invalid value '{val}' — use e.g. 128000, 128k, 1m, or 'auto' to clear"),
                tui,
                false,
            );
        }
    }
}

/// Handles the `/compact` / `/compress` command family.
pub(crate) async fn handle_compact(
    config: &Config,
    cwd: &Path,
    custom_instructions: Option<String>,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    tui: Option<&crate::composer::SharedTui>,
) {
    if agent.is_none() {
        reload_agent(agent, config, cwd).await;
    }
    let Some(ag) = agent.as_mut() else {
        if let Some(shared) = tui {
            shared
                .lock()
                .expect("tui lock")
                .push_dim("└ error: agent could not be initialized".to_string());
        } else {
            println!("error: agent could not be initialized");
        }
        return;
    };

    let messages = ag.messages().to_vec();
    if messages.is_empty() {
        if let Some(shared) = tui {
            shared
                .lock()
                .expect("tui lock")
                .push_dim("└ nothing to compact (conversation is empty)".to_string());
        } else {
            println!("nothing to compact (conversation is empty)");
        }
        return;
    }

    let msg_count = messages.len();

    if let Some(shared) = tui {
        shared
            .lock()
            .expect("tui lock")
            .set_status(Some("Compacting conversation context"));
    }

    let compact_res = crate::compact::compact_with_keep(
        ag,
        custom_instructions.as_deref(),
        crate::setup::user_keep_recent_tokens(),
    )
    .await;

    if let Some(shared) = tui {
        shared.lock().expect("tui lock").set_status(None);
    }

    match compact_res {
        Ok(true) => {
            // Record to session storage if active (helper already set_messages)
            if let Some(state) = session_state {
                for msg in ag.messages().to_vec() {
                    let _ = state.store.append(&state.session_id, &msg).await;
                }
            }

            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim(format!(
                    "└ compressed context ({} turns -> structured summary)",
                    msg_count
                ));
            } else {
                println!(
                    "compressed context ({} turns -> structured summary)",
                    msg_count
                );
            }
        }
        Ok(false) => {
            if let Some(shared) = tui {
                shared
                    .lock()
                    .expect("tui lock")
                    .push_dim("└ nothing to compact (conversation is empty)".to_string());
            } else {
                println!("nothing to compact (conversation is empty)");
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                shared
                    .lock()
                    .expect("tui lock")
                    .push_dim(format!("└ compaction failed: {e}"));
            } else {
                println!("compaction failed: {e}");
            }
        }
    }
}
