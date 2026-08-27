//! Tools & system / slash.exec / insights / rollback / browser-plugins-cron-skills JSON-RPC handlers — slice 1 (lines 1-900).
//!
//! 1:1 port of `tui_gateway/methods_tools.py` lines 1–900 (T0384 slice 1/2579).
//!
//! Handler bodies are byte-identical to their pre-split `server.py` form; they
//! are rebound onto `server.py`'s globals at install time — see `method_ctx.py`.
//!
//! ```python
//! # Python — tui_gateway/methods_tools.py 1-900 (abridged, comments preserved)
//! """Tools & system / slash.exec / insights / rollback / browser-plugins-cron-skills JSON-RPC handlers (moved verbatim from server.py).
//!
//! Handler bodies are byte-identical to their pre-split server.py form; they
//! are rebound onto server.py's globals at install time — see method_ctx.py.
//! """
//! from .method_ctx import HandlerRegistry
//! _registry = HandlerRegistry()
//! method = _registry.method
//! _profile_scoped = _registry.profile_scoped
//!
//! @method("system.battery")
//! def _(rid, params: dict) -> dict:
//!     try:
//!         from agent.battery import battery_category, read_battery
//!         batt = read_battery()
//!         return _ok(rid, {"available": batt.available, "percent": batt.percent, "plugged": batt.plugged, "category": battery_category(batt)})
//!     except Exception:
//!         return _ok(rid, {"available": False, "percent": None, "plugged": None, "category": "dim"})
//!
//! @method("process.stop")
//! def _(rid, params: dict) -> dict:
//!     try: from tools.process_registry import process_registry; return _ok(rid, {"killed": process_registry.kill_all()})
//!     except Exception as e: return _err(rid, 5010, str(e))
//!
//! @method("process.list")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess(params, rid)
//!     if err: return err
//!     try: return _ok(rid, {"processes": _session_processes(session)})
//!     except Exception as e: return _err(rid, 5010, str(e))
//!
//! @method("process.kill")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess(params, rid)
//!     if err: return err
//!     proc_id = str(params.get("process_id") or "")
//!     if not proc_id: return _err(rid, 4012, "process_id required")
//!     try:
//!         from tools.process_registry import process_registry
//!         proc = process_registry.get(proc_id)
//!         if proc is None or str(getattr(proc, "session_key", "") or "") != str(session.get("session_key") or ""):
//!             return _err(rid, 4044, f"no such process: {proc_id}")
//!         return _ok(rid, process_registry.kill_process(proc_id))
//!     except Exception as e: return _err(rid, 5010, str(e))
//!
//! @method("reload.mcp")
//! def _(rid, params: dict) -> dict:
//!     session = _sessions.get(params.get("session_id", ""))
//!     try:
//!         user_confirm = bool(params.get("confirm", False))
//!         if not user_confirm:
//!             try: from hermes_cli.config import load_config as _load_config; _cfg = _load_config(); _approvals = _cfg.get("approvals") if isinstance(_cfg, dict) else None; _confirm_required = True; ... 
//!             except Exception: _confirm_required = True
//!             if _confirm_required: return _ok(rid, {"status": "confirm_required", "message": "⚠️  /reload-mcp invalidates the prompt cache ..."})
//!         if session and _session_uses_compute_host(session):
//!             try: ack = _get_compute_host_supervisor().reload_mcp(str(params.get("session_id") or ""), request_id=f"reload-mcp-{rid}")
//!             except Exception as exc: return _err(rid, 5019, f"compute-host reload_mcp failed: {exc}")
//!             return _ok(rid, {"status": "reloaded", "turn_isolation": True, "host_ack": ack})
//!         from tools.mcp_tool import shutdown_mcp_servers, discover_mcp_tools
//!         def _refresh_session_agent() -> None: ...
//!         global _mcp_reload_gen, _mcp_reload_loaded_rev
//!         req_rev = str(params.get("rev") or "")
//!         def _do_full_reload() -> None: loaded = _compute_mcp_rev(); for _ in range(_MCP_RELOAD_MAX_PASSES): shutdown_mcp_servers(); discover_mcp_tools(); after = _compute_mcp_rev(); if after == loaded: break; loaded = after; _refresh_session_agent(); _mcp_reload_loaded_rev = loaded; _mcp_reload_gen += 1
//!         if _mcp_reload_lock.acquire(blocking=False):
//!             try: _do_full_reload()
//!             finally: _mcp_reload_lock.release()
//!             return _finish_reload(rid, params, coalesced=False)
//!         gen_before = _mcp_reload_gen
//!         with _mcp_reload_lock:
//!             leader_completed = _mcp_reload_gen > gen_before; rev_satisfied = not req_rev or req_rev == _mcp_reload_loaded_rev
//!             if leader_completed and rev_satisfied: _refresh_session_agent(); coalesced = True
//!             else: _do_full_reload(); coalesced = False
//!         return _finish_reload(rid, params, coalesced=coalesced)
//!     except Exception as e: return _err(rid, 5015, str(e))
//!
//! @method("reload.env")
//! def _(rid, params: dict) -> dict:
//!     try: from hermes_cli.config import reload_env; count = reload_env(); return _ok(rid, {"updated": int(count)})
//!     except Exception as e: return _err(rid, 5015, str(e))
//!
//! @method("commands.catalog")
//! def _(rid, params: dict) -> dict:
//!     try:
//!         from hermes_cli.commands import COMMAND_REGISTRY, SUBCOMMANDS, _build_description
//!         all_pairs: list[list[str]] = []; canon: dict[str, str] = {}; categories: list[dict] = []; cat_map: dict[str, list[list[str]]] = {}; cat_order: list[str] = []
//!         for cmd in COMMAND_REGISTRY:
//!             if cmd.name in _TUI_HIDDEN or cmd.gateway_only: continue
//!             c = f"/{cmd.name}"; canon[c.lower()] = c; ...; desc = _build_description(cmd); all_pairs.append([c, desc]); cat = cmd.category; ...
//!         for name, desc, cat in _TUI_EXTRA: if name.lower() in canon: continue; canon[name.lower()] = name; all_pairs.append([name, desc]); ...
//!         warning = ""
//!         try: qcmds = _load_cfg().get("quick_commands", {}) or {}; ... ; for qname, qc in sorted(qcmds.items()): ... ; cat_map[bucket].append([key, qdesc])
//!         except Exception as e: if not warning: warning = f"quick_commands discovery unavailable: {e}"
//!         skill_count = 0; skills: dict[str, dict] = {}
//!         try: from agent.skill_commands import scan_skill_commands; usage, origin_of = _skill_usage_lookup(); for k, info in sorted(scan_skill_commands().items()): ...; skills[k] = {"usage": usage(name), "origin": origin_of(name)}; skill_count += 1
//!         except Exception as e: warning = f"skill discovery unavailable: {e}"
//!         for cat in cat_order: categories.append({"name": cat, "pairs": cat_map[cat]})
//!         sub = {k: v[:] for k, v in SUBCOMMANDS.items()}
//!         return _ok(rid, {"pairs": all_pairs, "sub": sub, "canon": canon, "categories": categories, "skills": skills, "skill_count": skill_count, "warning": warning})
//!     except Exception as e: return _err(rid, 5020, str(e))
//!
//! @method("cli.exec")
//! def _(rid, params: dict) -> dict:
//!     argv = params.get("argv", [])
//!     if not isinstance(argv, list) or not all(isinstance(x, str) for x in argv): return _err(rid, 4003, "argv must be list[str]")
//!     hint = _cli_exec_blocked(argv)
//!     if hint: return _ok(rid, {"blocked": True, "hint": hint, "code": -1, "output": ""})
//!     try:
//!         from hermes_cli._subprocess_compat import windows_hide_flags
//!         r = subprocess.run([sys.executable, "-m", "hermes_cli.main", *argv], capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=min(int(params.get("timeout", 240)), 600), cwd=os.getcwd(), env=hermes_subprocess_env(inherit_credentials=True), stdin=subprocess.DEVNULL, creationflags=windows_hide_flags())
//!         parts = [r.stdout or "", r.stderr or ""]; out = "\n".join(p for p in parts if p).strip() or "(no output)"; return _ok(rid, {"blocked": False, "code": r.returncode, "output": out[:48_000]})
//!     except subprocess.TimeoutExpired: return _err(rid, 5016, "cli.exec: timeout")
//!     except Exception as e: return _err(rid, 5017, str(e))
//!
//! @method("command.resolve")
//! def _(rid, params: dict) -> dict:
//!     try: from hermes_cli.commands import resolve_command; r = resolve_command(params.get("name", "")); if r: return _ok(rid, {"canonical": r.name, "description": r.description, "category": r.category}); return _err(rid, 4011, f"unknown command: {params.get('name')}")
//!     except Exception as e: return _err(rid, 5012, str(e))
//!
//! @method("command.dispatch")
//! def _(rid, params: dict) -> dict:
//!     name, arg = params.get("name", "").lstrip("/"), params.get("arg", ""); resolved = _resolve_name(name); if resolved != name: name = resolved; session = _sessions.get(params.get("session_id", ""))
//!     qcmds = _load_cfg().get("quick_commands", {})
//!     if name in qcmds: qc = qcmds[name]; if qc.get("type") == "exec": sanitized_env = build_subprocess_env(); r = subprocess.run(qc.get("command",""), shell=True, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=30, stdin=subprocess.DEVNULL, env=sanitized_env, creationflags=windows_hide_flags()); output = ((r.stdout or "") + ("\n" if r.stdout and r.stderr else "") + (r.stderr or "")).strip()[:4000]; if output: from agent.redact import redact_sensitive_text; output = redact_sensitive_text(output); if r.returncode != 0: return _err(rid, 4018, output or f"quick command failed with exit code {r.returncode}"); return _ok(rid, {"type": "exec", "output": output}); if qc.get("type") == "alias": return _ok(rid, {"type": "alias", "target": qc.get("target","")})
//!     try: from hermes_cli.plugins import get_plugin_command_handler, resolve_plugin_command_result; handler = get_plugin_command_handler(name); if handler: result = resolve_plugin_command_result(handler(arg)); return _ok(rid, {"type": "plugin", "output": str(result or "")})
//!     except Exception: pass
//!     try: from agent.skill_bundles import build_bundle_invocation_message, get_skill_bundles, resolve_bundle_command_key; from hermes_cli.commands import resolve_command; bundle_key = resolve_bundle_command_key(name) if resolve_command(name) is None else None
//!     except Exception: bundle_key = None
//!     if bundle_key is not None: try: bundle_result = build_bundle_invocation_message(bundle_key, arg, task_id=session.get("session_key","") if session else "", platform=_resolve_session_platform())
//!         except Exception as exc: return _err(rid, 4018, f"bundle dispatch failed: {exc}"); if not bundle_result: return _err(rid, 4018, f"failed to load bundle: {bundle_key}"); msg, loaded_names, missing = bundle_result; bundle_info = get_skill_bundles().get(bundle_key, {}); bundle_name = bundle_info.get("name", bundle_key.lstrip("/")); notice = f"⚡ Loading bundle: {bundle_name} ({len(loaded_names)} skills)"; if missing: notice += f"\nSkipped missing skills: {', '.join(missing)}"; return _ok(rid, {"type": "send", "message": msg, "notice": notice, "display": _skill_scaffold_projection(msg)})
//!     try: from agent.skill_commands import scan_skill_commands, build_skill_invocation_message; cmds = scan_skill_commands(); key = f"/{name}"; if key in cmds: msg = build_skill_invocation_message(key, arg, task_id=session.get("session_key","") if session else ""); if msg: return _ok(rid, {"type": "skill", "message": msg, "name": cmds[key].get("name", name), "display": _skill_scaffold_projection(msg)})
//!     except Exception: pass
//!     # ... queue/q, learn, init, moa, focus, retry, steer, goal, loop truncated at line 900 inside undo (continues in slice 2)
//!     if name in {"queue", "q"}: if not arg: return _err(rid, 4004, "usage: /queue <prompt>"); return _ok(rid, {"type": "send", "message": arg})
//!     if name == "learn": from agent.learn_prompt import build_learn_prompt; return _ok(rid, {"type": "send", "message": build_learn_prompt(arg)})
//!     # ... (undo handling up to line 900, truncated — see slice 2 for snapshot/compress tail and slash.exec)
//!
//! def register(server) -> None: _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType`
//!   rebinding no-op notes).
//! * `agent.battery.read_battery` / `battery_category` → injected
//!   `battery_fn: Fn() -> Result<BatteryInfo,String>` where `Err` →
//!   `{"available":false,"percent":null,"plugged":null,"category":"dim"}` (always
//!   `_ok`, never `_err`; exception path preserved). See [`BatteryInfo`] +
//!   [`handle_system_battery`].
//! * `process_registry.kill_all` / `kill_process` / `get` +
//!   `_sess` / `_session_processes` → injected `process_op: Fn(&str) -> Result<String,(i32,String)>`
//!   where `params_json` carries `session_id`; `None`/`Err` maps to `_err` with
//!   the original code (`5010`/`4012`/`4044`). `process_id` required (`4012`)
//!   stays in Rust (mirrors `if not proc_id: return _err(rid,4012,...)`). See
//!   [`handle_process_stop`] / [`handle_process_list`] / [`handle_process_kill`].
//! * `_mcp_reload_lock` / `_compute_mcp_rev` / `shutdown_mcp_servers` /
//!   `discover_mcp_tools` / `_refresh_session_agent` / `_finish_reload` /
//!   leader vs follower coalesce → injected `reload_mcp_fn: Fn(&str)->Result<String,(i32,String)>`
//!   that owns the lock + rev + confirm gate + compute-host branch. Rust keeps
//!   the `_ok` `confirm_required` message shape via the closure's payload;
//!   exception → `5015`/`5019` is preserved by the closure returning `Err`.
//!   See [`handle_reload_mcp`].
//! * `hermes_cli.config.reload_env` → injected `Fn()->Result<usize,String>`
//!   (exception → `5015`) → [`handle_reload_env`].
//! * `COMMAND_REGISTRY` / `SUBCOMMANDS` / `_TUI_EXTRA` / `quick_commands` /
//!   `scan_skill_commands` / `_skill_usage_lookup` → injected
//!   `Fn(&str)->Result<String,String>` returning the full `{"pairs":...}` payload;
//!   exception → `5020` → [`handle_commands_catalog`].
//! * `_cli_exec_blocked` / `subprocess.run` vs blocked hint → injected
//!   `Fn(&str)->Result<String,(i32,String)>` where `params_json` contains `argv`+`timeout`;
//!   Rust keeps `argv must be list[str]` (`4003`) and forwards the closure's
//!   `Ok(payload)` as blocked `true`/`false` (`_ok`); timeout (`5016`) and
//!   exception (`5017`) are `Err` codes → [`handle_cli_exec`].
//! * `hermes_cli.commands.resolve_command` → injected
//!   `Fn(&str)->Result<Option<ResolveInfo>,(i32,String)>` (`4011`/`5012`) →
//!   [`handle_command_resolve`].
//! * `command.dispatch` quick → plugin → bundle → skill → queue/learn/init/moa/focus/retry/steer/goal/loop/undo (truncated at 900) →
//!   injected `dispatch_fn: Fn(&str)->Result<String,(i32,String)>` owning the
//!   `quick_commands` exec/alias, plugin-handle, bundle/scaffold, skill scan,
//!   and all slash sugar branches (`moa_one_shot_restore` / `focus` via
//!   `config.set` / `retry` `is_user_originated_turn` / `steer` / `goal` /
//!   `loop` / `undo` soft-delete etc). Slice 1 covers through the `undo`
//!   `session_key` guard at ~900; `snapshot`/`compress`/`slash.exec` tails live
//!   in slice 2. Validation for empty `/queue` (`4004`) and similar leaves in Rust
//!   is also owned by the closure to keep the truncated slice faithful. See
//!   [`handle_command_dispatch`] (truncated, delegated).
//! * `is_truthy_value` → [`is_truthy_value`] (mirrors `hermes_constants` truthy).
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → [`ok_response`] /
//!   [`err_response`] (mirrors `server.py::_ok` / `_err`).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] /
//!   [`build_registry`] / [`build_registry_default`] (deferred via `HandlerRegistry`).

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators in 1-900
// ---------------------------------------------------------------------------

pub const METHOD_SYSTEM_BATTERY: &str = "system.battery";
pub const METHOD_PROCESS_STOP: &str = "process.stop";
pub const METHOD_PROCESS_LIST: &str = "process.list";
pub const METHOD_PROCESS_KILL: &str = "process.kill";
pub const METHOD_RELOAD_MCP: &str = "reload.mcp";
pub const METHOD_RELOAD_ENV: &str = "reload.env";
pub const METHOD_COMMANDS_CATALOG: &str = "commands.catalog";
pub const METHOD_CLI_EXEC: &str = "cli.exec";
pub const METHOD_COMMAND_RESOLVE: &str = "command.resolve";
pub const METHOD_COMMAND_DISPATCH: &str = "command.dispatch";

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_PROCESS: i32 = 5010;
pub const ERR_PROCESS_ID_REQUIRED: i32 = 4012;
pub const ERR_PROCESS_NOT_FOUND: i32 = 4044;
pub const ERR_RELOAD_MCP_COMPUTE_HOST: i32 = 5019;
pub const ERR_RELOAD_MCP: i32 = 5015;
pub const ERR_RELOAD_ENV: i32 = 5015;
pub const ERR_CATALOG: i32 = 5020;
pub const ERR_CLI_ARGV: i32 = 4003;
pub const ERR_CLI_TIMEOUT: i32 = 5016;
pub const ERR_CLI_EXEC: i32 = 5017;
pub const ERR_COMMAND_UNKNOWN: i32 = 4011;
pub const ERR_COMMAND_RESOLVE: i32 = 5012;
pub const ERR_COMMAND_DISPATCH: i32 = 4018;
pub const ERR_QUEUE_USAGE: i32 = 4004;
pub const ERR_UNDO_SESSION: i32 = 4001;
pub const ERR_UNDO_INVALID_COUNT: i32 = 4004;

// ---------------------------------------------------------------------------
// Small helpers — JSON envelope, rid encoding, field extraction
// ---------------------------------------------------------------------------

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(ch),
        }
    }
    out
}

pub fn ok_response(rid_json: &str, result_json: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, rid_json, result_json)
}

pub fn err_response(rid_json: &str, code: i32, msg: &str) -> String {
    let esc = json_escape(msg);
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"{}"}}}}"#,
        rid_json, code, esc
    )
}

pub fn encode_rid(rid: &str) -> String {
    let t = rid.trim();
    if t.is_empty() {
        return "null".to_string();
    }
    if t == "null" || t == "true" || t == "false" {
        return t.to_string();
    }
    if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
        if t.eq_ignore_ascii_case("inf")
            || t.eq_ignore_ascii_case("nan")
            || t.eq_ignore_ascii_case("-inf")
        {
            return format!("\"{}\"", json_escape(t));
        }
        return t.to_string();
    }
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        return format!("\"{}\"", json_escape(&t[1..t.len() - 1]));
    }
    format!("\"{}\"", json_escape(t))
}

pub fn extract_string_field(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let after = &json[pos + key.len()..];
    let colon = after.find(':')?;
    let mut val = after[colon + 1..].trim_start();
    if val.starts_with("null") {
        return None;
    }
    if val.starts_with('\'') {
        let end = val[1..].find('\'')?;
        return Some(val[1..1 + end].to_string());
    }
    if !val.starts_with('"') {
        let end = val.find(|c| c == ',' || c == '}').unwrap_or(val.len());
        let raw = val[..end].trim().trim_matches('"').trim_matches('\'');
        if raw.is_empty() || raw == "null" {
            return None;
        }
        return Some(raw.to_string());
    }
    let mut out = String::new();
    let mut chars = val[1..].chars();
    let mut esc = false;
    for ch in chars {
        if esc {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'u' => {
                    for _ in 0..3 {
                        chars.next();
                    }
                    out.push('?');
                }
                _ => out.push(ch),
            }
            esc = false;
            continue;
        }
        if ch == '\\' {
            esc = true;
            continue;
        }
        if ch == '"' {
            return Some(out);
        }
        out.push(ch);
    }
    None
}

pub fn extract_string_or_empty(json: &str, field: &str) -> String {
    extract_string_field(json, field).unwrap_or_default()
}

pub fn extract_raw_value(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let after = &json[pos + key.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if rest.is_empty() {
        return None;
    }
    if rest.starts_with('[') || rest.starts_with('{') {
        let open = rest.chars().next().unwrap();
        let close = if open == '[' { ']' } else { '}' };
        let mut depth = 0usize;
        let mut in_str = false;
        let mut esc = false;
        let mut end_idx: Option<usize> = None;
        for (i, ch) in rest.char_indices() {
            if esc {
                esc = false;
                continue;
            }
            if ch == '\\' && in_str {
                esc = true;
                continue;
            }
            if ch == '"' && !esc {
                in_str = !in_str;
                continue;
            }
            if in_str {
                continue;
            }
            if ch == open {
                depth += 1;
            } else if ch == close {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        end_idx = Some(i);
                        break;
                    }
                }
            }
        }
        if let Some(e) = end_idx {
            return Some(rest[..=e].to_string());
        }
        return None;
    }
    if rest.starts_with('"') || rest.starts_with('\'') {
        let qc = rest.chars().next().unwrap();
        let mut esc = false;
        for (i, ch) in rest[1..].char_indices() {
            if esc {
                esc = false;
                continue;
            }
            if ch == '\\' {
                esc = true;
                continue;
            }
            if ch == qc {
                return Some(rest[..=i + 1].to_string());
            }
        }
        return None;
    }
    let end = rest.find(|c| c == ',' || c == '}').unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

pub fn extract_bool_field(json: &str, field: &str) -> Option<bool> {
    let raw = extract_raw_value(json, field)?;
    match raw.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Battery helpers — mirrors agent.battery + methods_tools 1-37
// ---------------------------------------------------------------------------

/// Battery snapshot — mirrors `read_battery()` return shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatteryInfo {
    pub available: bool,
    pub percent: Option<i32>,
    pub plugged: Option<bool>,
    pub category: String,
}

impl BatteryInfo {
    pub fn unavailable() -> Self {
        Self { available: false, percent: None, plugged: None, category: "dim".to_string() }
    }
    pub fn new(percent: Option<i32>, plugged: Option<bool>, category: impl Into<String>) -> Self {
        Self { available: true, percent, plugged, category: category.into() }
    }
    pub fn to_json(&self) -> String {
        let pct = match self.percent {
            Some(n) => n.to_string(),
            None => "null".to_string(),
        };
        let plugged = match self.plugged {
            Some(b) => if b { "true".to_string() } else { "false".to_string() },
            None => "null".to_string(),
        };
        format!(
            r#"{{"available":{},"percent":{},"plugged":{},"category":"{}"}}"#,
            if self.available { "true" } else { "false" },
            pct,
            plugged,
            json_escape(&self.category)
        )
    }
}

// ---------------------------------------------------------------------------
// Process helpers — mirrors process.* branches
// ---------------------------------------------------------------------------

/// Returns `true` when `process_id` param is valid (non-empty after trim).
pub fn is_valid_process_id(raw: &str) -> bool {
    !raw.trim().is_empty()
}

// ---------------------------------------------------------------------------
// Cli helpers — mirrors cli.exec argv validation and timeout clamp
// ---------------------------------------------------------------------------

/// Default timeout in seconds. Mirrors `params.get("timeout",240)`.
pub const DEFAULT_CLI_TIMEOUT_S: i64 = 240;
/// Hard clamp for timeout. Mirrors `min(...,600)`.
pub const MAX_CLI_TIMEOUT_S: i64 = 600;

/// Clamp `timeout` raw to `[0,MAX]`. Mirrors `min(int(params.get("timeout",240)),600)`.
pub fn clamp_cli_timeout(raw: Option<&str>) -> i64 {
    let v = match raw {
        None => DEFAULT_CLI_TIMEOUT_S,
        Some(s) => {
            let t = s.trim().trim_matches('"');
            match t.parse::<i64>() {
                Ok(n) => n,
                Err(_) => DEFAULT_CLI_TIMEOUT_S,
            }
        }
    };
    if v < 0 { 0 } else if v > MAX_CLI_TIMEOUT_S { MAX_CLI_TIMEOUT_S } else { v }
}

/// Mirrors `isinstance(argv, list) and all(isinstance(x,str))` for JSON array payload.
///
/// `raw` is the JSON raw for `argv` (e.g. `["a","b"]`). Returns `true` when valid.
pub fn is_valid_argv_json(raw: &str) -> bool {
    let t = raw.trim();
    if !t.starts_with('[') || !t.ends_with(']') {
        return false;
    }
    let inner = t[1..t.len()-1].trim();
    if inner.is_empty() {
        return true; // empty list valid
    }
    // Very cheap check: all top-level tokens are quoted strings (no unquoted tokens)
    // For faithful std-only check, scan tokens.
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    let mut qc = '"';
    for ch in inner.chars() {
        if esc { cur.push(ch); esc = false; continue; }
        if ch == '\\' && in_str { cur.push(ch); esc = true; continue; }
        if (ch == '"' || ch == '\'') && !esc {
            if !in_str { in_str = true; qc = ch; } else if ch == qc { in_str = false; }
            cur.push(ch); continue;
        }
        if ch == ',' && !in_str {
            tokens.push(cur.trim().to_string()); cur.clear(); continue;
        }
        cur.push(ch);
    }
    if !cur.trim().is_empty() { tokens.push(cur.trim().to_string()); }
    for tok in tokens {
        let tt = tok.trim();
        if tt.is_empty() { return false; }
        if !( (tt.starts_with('"') && tt.ends_with('"') && tt.len() >= 2) || (tt.starts_with('\'') && tt.ends_with('\'') && tt.len() >= 2) ) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Command registry helpers — mirrors commands.catalog + resolve
// ---------------------------------------------------------------------------

/// Resolve info for `command.resolve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveInfo {
    pub canonical: String,
    pub description: String,
    pub category: String,
}

impl ResolveInfo {
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"canonical":"{}","description":"{}","category":"{}"}}"#,
            json_escape(&self.canonical),
            json_escape(&self.description),
            json_escape(&self.category)
        )
    }
}

/// Truncate long skill description to 120 + ellipsis — mirrors `d[:120] + ("…" if len(d)>120 else "")`.
pub fn truncate_desc(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    if chars.len() > 120 {
        chars.truncate(120);
        chars.push('…');
        chars.into_iter().collect()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body (injected for std-only testing)
// ---------------------------------------------------------------------------

/// Handle `system.battery`.
///
/// `battery` mirrors `read_battery()` + `battery_category` → `BatteryInfo`.
/// Exception path (`Err`) maps to unavailable (`_ok` with `available:false`),
/// never `_err` — mirrors `except Exception: return _ok(rid, {"available":False,...})`.
pub fn handle_system_battery<F>(rid_json: &str, battery: F) -> String
where
    F: Fn() -> Result<BatteryInfo, String>,
{
    match battery() {
        Ok(info) => ok_response(rid_json, &info.to_json()),
        Err(_) => ok_response(rid_json, &BatteryInfo::unavailable().to_json()),
    }
}

/// Handle `process.stop`.
///
/// `op` mirrors `process_registry.kill_all()` → `{"killed": n}` payload fragment.
/// `Err(e)` → `5010`.
pub fn handle_process_stop<F>(rid_json: &str, op: F) -> String
where
    F: Fn() -> Result<String, String>,
{
    match op() {
        Ok(payload) => {
            let t = payload.trim();
            let result = if t.starts_with('{') { t.to_string() } else { format!(r#"{{"killed":{}}}"#, t) };
            ok_response(rid_json, &result)
        }
        Err(e) => err_response(rid_json, ERR_PROCESS, &e),
    }
}

/// Handle `process.list`.
///
/// `op` mirrors `_sess` + `_session_processes` → `{"processes": [...]}`.
/// `Err((code,msg))` is returned verbatim (session error vs `5010`).
pub fn handle_process_list<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `process.kill`.
///
/// Validates `process_id` required (`4012`) before delegating to `op` which owns
/// `_sess` session scoping + `process_registry.get`/`kill_process`.
/// `op` `Err((4012|4044|5010,msg))` maps verbatim.
pub fn handle_process_kill<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str, &str) -> Result<String, (i32, String)>,
{
    let proc_id = extract_string_field(params_json, "process_id")
        .or_else(|| extract_string_field(params_json, "processId"))
        .unwrap_or_default();
    let pid = proc_id.trim();
    if pid.is_empty() {
        // also check raw params.get("process_id") stringified as maybe not quoted? Use fallback:
        let raw = extract_raw_value(params_json, "process_id");
        let maybe = raw.as_deref().map(|s| s.trim().trim_matches('"').trim().to_string()).unwrap_or_default();
        if maybe.is_empty() {
            return err_response(rid_json, ERR_PROCESS_ID_REQUIRED, "process_id required");
        }
        // if raw was present but empty string, already handled
        if maybe.trim().is_empty() {
            return err_response(rid_json, ERR_PROCESS_ID_REQUIRED, "process_id required");
        }
        // use maybe for op call
        return match op(params_json, maybe.trim()) {
            Ok(payload) => ok_response(rid_json, payload.trim()),
            Err((code, msg)) => err_response(rid_json, code, &msg),
        };
    }
    match op(params_json, pid) {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `reload.mcp`.
///
/// `op` mirrors the whole `try:` body: confirm gate → `confirm_required`
/// `_ok({"status":"confirm_required",...})`, compute-host branch (`5019`),
/// `shutdown_mcp_servers`/`discover_mcp_tools` + `_mcp_reload_lock` generation
/// coalesce + `_refresh_session_agent` + `_finish_reload`. Returns `Ok(payload_json)`
/// where payload_json is the full result object (`{"status":"reloaded",...}` or
/// `{"status":"confirm_required",...}`), `Err((code,msg))` maps to `_err` (`5015`/`5019`).
pub fn handle_reload_mcp<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `reload.env`.
///
/// `op` mirrors `reload_env()` → `{"updated": n}`. `Err(e)` → `5015`.
pub fn handle_reload_env<F>(rid_json: &str, op: F) -> String
where
    F: Fn() -> Result<String, String>,
{
    match op() {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err(e) => err_response(rid_json, ERR_RELOAD_ENV, &e),
    }
}

/// Handle `commands.catalog`.
///
/// `op` mirrors `COMMAND_REGISTRY` + `_TUI_EXTRA` + `quick_commands` + `scan_skill_commands`
/// → full `{"pairs":...,"sub":...,"canon":...,"categories":...,"skills":...,"skill_count":...,"warning":...}`.
/// `Err(e)` → `5020`.
pub fn handle_commands_catalog<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err(e) => err_response(rid_json, ERR_CATALOG, &e),
    }
}

/// Handle `cli.exec`.
///
/// Validates `argv must be list[str]` (`4003`) in Rust before delegating.
/// `op` mirrors `_cli_exec_blocked` hint → `{"blocked":true,"hint":...,}` (`_ok`) vs
/// `subprocess.run` → `{"blocked":false,"code":..., "output":...}` (`_ok`) vs
/// `TimeoutExpired` (`5016`) vs other exception (`5017`).
pub fn handle_cli_exec<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    // Validate argv present and list of strings
    let raw_argv = extract_raw_value(params_json, "argv");
    match raw_argv {
        None => return err_response(rid_json, ERR_CLI_ARGV, "argv must be list[str]"),
        Some(raw) => {
            if !is_valid_argv_json(&raw) {
                return err_response(rid_json, ERR_CLI_ARGV, "argv must be list[str]");
            }
        }
    }
    // timeout clamp is handled inside op but we validate parse for completeness
    let _timeout = clamp_cli_timeout(extract_raw_value(params_json, "timeout").as_deref().map(|s| s.as_str()));
    match op(params_json) {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `command.resolve`.
///
/// `op` mirrors `resolve_command(name)` → `Some(ResolveInfo)` → `_ok({"canonical":...})`
/// vs `None` → `_err 4011` vs exception → `5012`.
pub fn handle_command_resolve<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<Option<ResolveInfo>, (i32, String)>,
{
    let name = extract_string_field(params_json, "name").unwrap_or_default();
    match op(&name) {
        Ok(Some(info)) => ok_response(rid_json, &info.to_json()),
        Ok(None) => err_response(rid_json, ERR_COMMAND_UNKNOWN, &format!("unknown command: {}", name)),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `command.dispatch` (truncated at line 900 — slice 1).
///
/// `op` owns the full `quick_commands` exec/alias + `get_plugin_command_handler` +
/// `resolve_bundle_command_key`/`build_bundle_invocation_message` +
/// `scan_skill_commands`/`build_skill_invocation_message` + all sugar branches
/// (`/queue` `4004`, `/learn`, `/init`, `/moa` `4004`/`5030`, `/focus` `4004`,
/// `/retry` `4001`/`4009`/`4018`, `/steer` `4004`, `/goal` `4001`/`5030`/`4004`,
/// `/loop` `4001`/`5030`, `/undo` `4001`/`4009`/`5008` + `session_key` guard at
/// ~900). Slice 1 covers through `if not session_key: return _err(4001, "no session key for undo")`
/// at 900; `/snapshot`/`/compress`/`slash.exec` tails live in slice 2. The
/// delta `undo` soft-delete + `rewind_to_message` + `get_messages_as_conversation`
/// + `display text prefill` etc is delegated to the closure.
///
/// Returns `Ok(result_json)` where result_json is the dispatched `result`
/// (`{"type":"exec","output":...}` / `{"type":"send","message":...}` / etc.),
/// `Err((code,msg))` → `_err`.
pub fn handle_command_dispatch<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    // Slice-1 has no early validation that stays in Rust: `name` empty vs `not a ...` is `4018`
    // owned by the closure to keep the truncated slice faithful.
    match op(params_json) {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with the ten slice-1 methods registered.
///
/// Each closure is `'static` and mirrors the lazy imports inside Python handler
/// bodies. For the default stub (no backend) use [`build_registry_default`].
pub fn build_registry<SB, PS, PL, PK, RM, RE, CC, CE, CR, CD>(
    system_battery: SB,
    process_stop: PS,
    process_list: PL,
    process_kill: PK,
    reload_mcp: RM,
    reload_env: RE,
    commands_catalog: CC,
    cli_exec: CE,
    command_resolve: CR,
    command_dispatch: CD,
) -> HandlerRegistry
where
    SB: Fn(String, String) -> String + Send + Sync + 'static,
    PS: Fn(String, String) -> String + Send + Sync + 'static,
    PL: Fn(String, String) -> String + Send + Sync + 'static,
    PK: Fn(String, String) -> String + Send + Sync + 'static,
    RM: Fn(String, String) -> String + Send + Sync + 'static,
    RE: Fn(String, String) -> String + Send + Sync + 'static,
    CC: Fn(String, String) -> String + Send + Sync + 'static,
    CE: Fn(String, String) -> String + Send + Sync + 'static,
    CR: Fn(String, String) -> String + Send + Sync + 'static,
    CD: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        system_battery,
        process_stop,
        process_list,
        process_kill,
        reload_mcp,
        reload_env,
        commands_catalog,
        cli_exec,
        command_resolve,
        command_dispatch,
    );
    reg
}

/// Build a registry with default stubs (no backend / no file I/O / always `available:false`).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, _params_json| {
            let rid_json = encode_rid(&rid);
            handle_system_battery(&rid_json, || Err("no backend".to_string()))
        },
        |rid, _| {
            let rid_json = encode_rid(&rid);
            handle_process_stop(&rid_json, || Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_process_list(&rid_json, &params_json, |_| Err((ERR_PROCESS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_process_kill(&rid_json, &params_json, |_, _| Err((ERR_PROCESS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_reload_mcp(&rid_json, &params_json, |_| Err((ERR_RELOAD_MCP, "no backend".to_string())))
        },
        |rid, _| {
            let rid_json = encode_rid(&rid);
            handle_reload_env(&rid_json, || Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_commands_catalog(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_cli_exec(&rid_json, &params_json, |_| Err((ERR_CLI_EXEC, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_command_resolve(&rid_json, &params_json, |_| Err((ERR_COMMAND_RESOLVE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_command_dispatch(&rid_json, &params_json, |_| Err((ERR_COMMAND_DISPATCH, "no backend".to_string())))
        },
    )
}

/// Register all ten slice-1 methods onto an existing registry.
pub fn register_with<SB, PS, PL, PK, RM, RE, CC, CE, CR, CD>(
    registry: &mut HandlerRegistry,
    system_battery: SB,
    process_stop: PS,
    process_list: PL,
    process_kill: PK,
    reload_mcp: RM,
    reload_env: RE,
    commands_catalog: CC,
    cli_exec: CE,
    command_resolve: CR,
    command_dispatch: CD,
) where
    SB: Fn(String, String) -> String + Send + Sync + 'static,
    PS: Fn(String, String) -> String + Send + Sync + 'static,
    PL: Fn(String, String) -> String + Send + Sync + 'static,
    PK: Fn(String, String) -> String + Send + Sync + 'static,
    RM: Fn(String, String) -> String + Send + Sync + 'static,
    RE: Fn(String, String) -> String + Send + Sync + 'static,
    CC: Fn(String, String) -> String + Send + Sync + 'static,
    CE: Fn(String, String) -> String + Send + Sync + 'static,
    CR: Fn(String, String) -> String + Send + Sync + 'static,
    CD: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method(METHOD_SYSTEM_BATTERY, system_battery);
    registry.method(METHOD_PROCESS_STOP, process_stop);
    registry.method(METHOD_PROCESS_LIST, process_list);
    registry.method(METHOD_PROCESS_KILL, process_kill);
    registry.method(METHOD_RELOAD_MCP, reload_mcp);
    registry.method(METHOD_RELOAD_ENV, reload_env);
    registry.method(METHOD_COMMANDS_CATALOG, commands_catalog);
    registry.method(METHOD_CLI_EXEC, cli_exec);
    registry.method(METHOD_COMMAND_RESOLVE, command_resolve);
    registry.method(METHOD_COMMAND_DISPATCH, command_dispatch);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, _params_json| {
            let rid_json = encode_rid(&rid);
            handle_system_battery(&rid_json, || Err("no backend".to_string()))
        },
        |rid, _| {
            let rid_json = encode_rid(&rid);
            handle_process_stop(&rid_json, || Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_process_list(&rid_json, &params_json, |_| Err((ERR_PROCESS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_process_kill(&rid_json, &params_json, |_, _| Err((ERR_PROCESS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_reload_mcp(&rid_json, &params_json, |_| Err((ERR_RELOAD_MCP, "no backend".to_string())))
        },
        |rid, _| {
            let rid_json = encode_rid(&rid);
            handle_reload_env(&rid_json, || Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_commands_catalog(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_cli_exec(&rid_json, &params_json, |_| Err((ERR_CLI_EXEC, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_command_resolve(&rid_json, &params_json, |_| Err((ERR_COMMAND_RESOLVE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_command_dispatch(&rid_json, &params_json, |_| Err((ERR_COMMAND_DISPATCH, "no backend".to_string())))
        },
    )
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn rid1() -> String { encode_rid("1") }

    #[test]
    fn battery_unavailable_on_err() {
        let rid = rid1();
        let ok = handle_system_battery(&rid, || Err("boom".into()));
        assert!(ok.contains(r#""available":false"#), "{}", ok);
        assert!(ok.contains(r#""category":"dim""#));
        assert!(ok.contains(r#""percent":null"#));
    }

    #[test]
    fn battery_success() {
        let rid = rid1();
        let ok = handle_system_battery(&rid, || Ok(BatteryInfo::new(Some(80), Some(true), "ok")));
        assert!(ok.contains(r#""available":true"#), "{}", ok);
        assert!(ok.contains(r#""percent":80"#));
        assert!(ok.contains(r#""plugged":true"#));
        assert!(ok.contains(r#""category":"ok""#));
    }

    #[test]
    fn process_stop_ok_and_err() {
        let rid = rid1();
        let ok = handle_process_stop(&rid, || Ok(r#"{"killed":3}"#.to_string()));
        assert!(ok.contains(r#""killed":3"#), "{}", ok);
        let ok2 = handle_process_stop(&rid, || Ok("5".to_string()));
        assert!(ok2.contains(r#""killed":5"#));
        let err = handle_process_stop(&rid, || Err("fail".into()));
        assert!(err.contains(r#""code":5010"#), "{}", err);
    }

    #[test]
    fn process_list_ok_and_err() {
        let rid = rid1();
        let ok = handle_process_list(&rid, r#"{"session_id":"a"}"#, |_| Ok(r#"{"processes":[]}"#.to_string()));
        assert!(ok.contains(r#""processes":[]"#), "{}", ok);
        let err = handle_process_list(&rid, r#"{"session_id":"a"}"#, |_| Err((5010, "boom".into())));
        assert!(err.contains(r#""code":5010"#));
        // session error maps verbatim
        let err2 = handle_process_list(&rid, r#"{"session_id":"bad"}"#, |_| Err((4001, "no session".into())));
        assert!(err2.contains(r#""code":4001"#));
    }

    #[test]
    fn process_kill_requires_id() {
        let rid = rid1();
        let out = handle_process_kill(&rid, r#"{"session_id":"s","process_id":""}"#, |_, _| Ok(r#"{"killed":1}"#.into()));
        assert!(out.contains(r#""code":4012"#), "{}", out);
        assert!(out.contains("process_id required"));
        let out2 = handle_process_kill(&rid, r#"{"session_id":"s"}"#, |_, _| Ok(r#"{"killed":1}"#.into()));
        assert!(out2.contains(r#""code":4012"#));
    }

    #[test]
    fn process_kill_dispatch_ok_and_404() {
        let rid = rid1();
        let ok = handle_process_kill(&rid, r#"{"session_id":"s","process_id":"p1"}"#, |_, pid| {
            assert_eq!(pid, "p1");
            Ok(r#"{"killed":1}"#.to_string())
        });
        assert!(ok.contains(r#""killed":1"#), "{}", ok);
        let nf = handle_process_kill(&rid, r#"{"session_id":"s","process_id":"bad"}"#, |_, _| Err((ERR_PROCESS_NOT_FOUND, "no such process: bad".into())));
        assert!(nf.contains(r#""code":4044"#));
    }

    #[test]
    fn reload_mcp_ok_and_err() {
        let rid = rid1();
        let ok = handle_reload_mcp(&rid, r#"{"session_id":"a","confirm":true}"#, |_| Ok(r#"{"status":"reloaded","turn_isolation":true}"#.to_string()));
        assert!(ok.contains(r#""status":"reloaded""#), "{}", ok);
        let confirm = handle_reload_mcp(&rid, r#"{"session_id":"a"}"#, |_| Ok(r#"{"status":"confirm_required","message":"⚠️"}"#.to_string()));
        assert!(confirm.contains("confirm_required"), "{}", confirm);
        let err = handle_reload_mcp(&rid, r#"{}"#, |_| Err((5015, "boom".into())));
        assert!(err.contains(r#""code":5015"#));
        let host_err = handle_reload_mcp(&rid, r#"{}"#, |_| Err((5019, "compute-host reload_mcp failed".into())));
        assert!(host_err.contains(r#""code":5019"#));
    }

    #[test]
    fn reload_env_ok_and_err() {
        let rid = rid1();
        let ok = handle_reload_env(&rid, || Ok(r#"{"updated":2}"#.to_string()));
        assert!(ok.contains(r#""updated":2"#), "{}", ok);
        let err = handle_reload_env(&rid, || Err("fail".into()));
        assert!(err.contains(r#""code":5015"#));
    }

    #[test]
    fn commands_catalog_ok_and_err() {
        let rid = rid1();
        let ok = handle_commands_catalog(&rid, "{}", |_| Ok(r#"{"pairs":[["/help","desc"]],"sub":{},"canon":{},"categories":[],"skills":{},"skill_count":0,"warning":""}"#.to_string()));
        assert!(ok.contains(r#""pairs""#), "{}", ok);
        assert!(ok.contains("/help"));
        let err = handle_commands_catalog(&rid, "{}", |_| Err("boom".into()));
        assert!(err.contains(r#""code":5020"#));
    }

    #[test]
    fn cli_exec_argv_validation() {
        let rid = rid1();
        let err = handle_cli_exec(&rid, r#"{}"#, |_| Ok(r#"{"blocked":false,"code":0,"output":"hi"}"#.to_string()));
        assert!(err.contains(r#""code":4003"#), "{}", err);
        assert!(err.contains("argv must be list[str]"));
        let err2 = handle_cli_exec(&rid, r#"{"argv":"notalist"}"#, |_| Ok(r#"{"blocked":false}"#.to_string()));
        assert!(err2.contains(r#""code":4003"#));
        let err3 = handle_cli_exec(&rid, r#"{"argv":[1,2]}"#, |_| Ok(r#"{"blocked":false}"#.to_string()));
        assert!(err3.contains(r#""code":4003"#));
    }

    #[test]
    fn cli_exec_blocked_and_success_and_timeout() {
        let rid = rid1();
        let blocked = handle_cli_exec(&rid, r#"{"argv":["--help"]}"#, |_| Ok(r#"{"blocked":true,"hint":"blocked","code":-1,"output":""}"#.to_string()));
        assert!(blocked.contains(r#""blocked":true"#), "{}", blocked);
        assert!(blocked.contains("blocked"));
        let ok = handle_cli_exec(&rid, r#"{"argv":["status"]}"#, |_| Ok(r#"{"blocked":false,"code":0,"output":"hello"}"#.to_string()));
        assert!(ok.contains(r#""blocked":false"#));
        assert!(ok.contains("hello"));
        let timeout = handle_cli_exec(&rid, r#"{"argv":["slow"]}"#, |_| Err((ERR_CLI_TIMEOUT, "cli.exec: timeout".into())));
        assert!(timeout.contains(r#""code":5016"#));
        let err = handle_cli_exec(&rid, r#"{"argv":["x"]}"#, |_| Err((ERR_CLI_EXEC, "boom".into())));
        assert!(err.contains(r#""code":5017"#));
    }

    #[test]
    fn cli_timeout_clamp() {
        assert_eq!(clamp_cli_timeout(None), 240);
        assert_eq!(clamp_cli_timeout(Some("100")), 100);
        assert_eq!(clamp_cli_timeout(Some("1000")), 600);
        assert_eq!(clamp_cli_timeout(Some("-5")), 0);
        assert_eq!(clamp_cli_timeout(Some("bad")), 240);
        assert_eq!(clamp_cli_timeout(Some("\"300\"")), 300);
    }

    #[test]
    fn argv_validation_helpers() {
        assert!(is_valid_argv_json(r#"[]"#));
        assert!(is_valid_argv_json(r#"["a","b"]"#));
        assert!(is_valid_argv_json(r#"['a','b']"#));
        assert!(!is_valid_argv_json(r#"["a",1]"#));
        assert!(!is_valid_argv_json(r#""not array""#));
        assert!(!is_valid_argv_json(r#"{}"#));
    }

    #[test]
    fn command_resolve_ok_unknown_and_err() {
        let rid = rid1();
        let ok = handle_command_resolve(&rid, r#"{"name":"help"}"#, |name| {
            assert_eq!(name, "help");
            Ok(Some(ResolveInfo { canonical: "help".into(), description: "show help".into(), category: "Info".into() }))
        });
        assert!(ok.contains(r#""canonical":"help""#), "{}", ok);
        let unk = handle_command_resolve(&rid, r#"{"name":"bogus"}"#, |_| Ok(None));
        assert!(unk.contains(r#""code":4011"#));
        assert!(unk.contains("unknown command"));
        let err = handle_command_resolve(&rid, r#"{"name":"x"}"#, |_| Err((ERR_COMMAND_RESOLVE, "boom".into())));
        assert!(err.contains(r#""code":5012"#));
    }

    #[test]
    fn command_dispatch_ok_and_err() {
        let rid = rid1();
        let ok = handle_command_dispatch(&rid, r#"{"name":"help","arg":"","session_id":"s"}"#, |_| Ok(r#"{"type":"exec","output":"help text"}"#.to_string()));
        assert!(ok.contains(r#""type":"exec""#), "{}", ok);
        let send = handle_command_dispatch(&rid, r#"{"name":"queue","arg":"hi"}"#, |_| Ok(r#"{"type":"send","message":"hi"}"#.to_string()));
        assert!(send.contains(r#""type":"send""#));
        let err = handle_command_dispatch(&rid, r#"{"name":"bad","arg":""}"#, |_| Err((4018, "not a quick/plugin/bundle/skill command: bad".into())));
        assert!(err.contains(r#""code":4018"#));
        // retry / goal / undo style errors
        let busy = handle_command_dispatch(&rid, r#"{"name":"retry","arg":""}"#, |_| Err((4009, "session busy — /interrupt".into())));
        assert!(busy.contains(r#""code":4009"#));
    }

    #[test]
    fn truncate_desc() {
        assert_eq!(truncate_desc("short"), "short");
        let long = "a".repeat(200);
        let tr = truncate_desc(&long);
        assert!(tr.chars().count() == 121);
        assert!(tr.ends_with('…'));
        assert_eq!(truncate_desc(&"a".repeat(120)).len(), 120);
        assert!(truncate_desc(&"a".repeat(121)).ends_with('…'));
    }

    #[test]
    fn battery_info_json() {
        let info = BatteryInfo::new(Some(50), Some(false), "ok");
        let j = info.to_json();
        assert!(j.contains(r#""percent":50"#));
        assert!(j.contains(r#""plugged":false"#));
        let unav = BatteryInfo::unavailable();
        assert!(unav.to_json().contains(r#""available":false"#));
    }

    #[test]
    fn ok_err_envelope_shape() {
        let rid = encode_rid("42");
        let ok = ok_response(&rid, r#"{"available":true}"#);
        assert!(ok.contains(r#""result""#));
        let err = err_response(&rid, 4012, "process_id required");
        assert!(err.contains(r#""code":4012"#));
        assert!(err.contains("process_id required"));
    }

    #[test]
    fn build_registry_installs() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 10);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["cli.exec","commands.catalog","command.dispatch","command.resolve","process.kill","process.list","process.stop","reload.env","reload.mcp","system.battery"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 10);
        assert!(map.contains_key(METHOD_SYSTEM_BATTERY));
        assert!(map.contains_key(METHOD_CLI_EXEC));
        // system.battery stub returns available false (no backend)
        let out = map.get(METHOD_SYSTEM_BATTERY).unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains(r#""available":false"#), "{}", out);
        // cli.exec stub without argv should be 4003
        let out2 = map.get(METHOD_CLI_EXEC).unwrap()("1".to_string(), "{}".to_string());
        assert!(out2.contains(r#""code":4003"#), "{}", out2);
        // process.kill stub without process_id → 4012
        let out3 = map.get(METHOD_PROCESS_KILL).unwrap()("1".to_string(), "{}".to_string());
        assert!(out3.contains(r#""code":4012"#), "{}", out3);
    }

    #[test]
    fn extract_helpers() {
        assert_eq!(extract_string_field(r#"{"process_id":"p1"}"#, "process_id").as_deref(), Some("p1"));
        assert_eq!(extract_string_field(r#"{"name":"help"}"#, "name").as_deref(), Some("help"));
        assert_eq!(extract_raw_value(r#"{"argv":["a","b"]}"#, "argv").unwrap(), r#"["a","b"]"#);
        assert!(extract_bool_field(r#"{"confirm":true}"#, "confirm").unwrap());
        assert!(!extract_bool_field(r#"{"confirm":false}"#, "confirm").unwrap());
    }
}
