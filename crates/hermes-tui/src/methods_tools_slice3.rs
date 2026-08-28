//! Tools & system / learning detail+edit+delete / skills / mcp catalog+servers+oauth / plugins / shell handlers — slice 3 (lines 1800-2579).
//!
//! 1:1 port of `tui_gateway/methods_tools.py` lines 1800–2579 (T0384 slice 3/2579).
//!
//! Handler bodies are byte-identical to their pre-split `server.py` form; they
//! are rebound onto `server.py`'s globals at install time — see `method_ctx.py`.
//!
//! ```python
//! # Python — tui_gateway/methods_tools.py 1800-2579 (abridged, comments preserved)
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
//! @method("learning.frames")
//! def _(rid, params: dict) -> dict:
//!     # covered in slice 2 (900-1800 tail: cols/rows/frames 80/24/48 + max(20,cols)/max(10,rows) + build_learning_graph/render_frames 5000)
//!     ...
//!
//! @method("learning.detail")
//! def _(rid, params: dict) -> dict:
//!     try: from agent.learning_mutations import node_detail; return _ok(rid, node_detail(str(params.get("id",""))))
//!     except Exception as exc: return _err(rid, 5000, f"learning.detail failed: {exc}")
//!
//! @method("learning.delete")
//! def _(rid, params: dict) -> dict:
//!     try: from agent.learning_mutations import delete_node; return _ok(rid, delete_node(str(params.get("id",""))))
//!     except Exception as exc: return _err(rid, 5000, f"learning.delete failed: {exc}")
//!
//! @method("learning.edit")
//! def _(rid, params: dict) -> dict:
//!     try: from agent.learning_mutations import edit_node; return _ok(rid, edit_node(str(params.get("id","")), str(params.get("content",""))))
//!     except Exception as exc: return _err(rid, 5000, f"learning.edit failed: {exc}")
//!
//! @method("skills.manage")
//! def _(rid, params: dict) -> dict:
//!     action, query = params.get("action","list"), params.get("query","")
//!     profile = str(params.get("profile") or "").strip(); token=None
//!     if profile: try: from hermes_cli.profiles import get_profile_dir; from hermes_constants import set_hermes_home_override; profile_dir=get_profile_dir(profile); if not profile_dir or not profile_dir.is_dir(): return _err(rid,4064,f"profile '{profile}' not found"); token=set_hermes_home_override(str(profile_dir))
//!                except Exception as e: return _err(rid,5024,str(e))
//!     try:
//!         if action=="list": from hermes_cli.banner import get_available_skills; return _ok(rid,{"skills":get_available_skills()})
//!         if action=="search": from tools.skills_hub import GitHubAuth, create_source_router, unified_search; raw=unified_search(query,create_source_router(GitHubAuth()),source_filter="all",limit=20) or []; return _ok(rid,{"results":[{"name":r.name,"description":r.description} for r in raw]})
//!         if action=="install": from hermes_cli.skills_hub import do_install; class _Q: def print(self,*a,**k): pass; do_install(query,skip_confirm=True,console=_Q()); return _ok(rid,{"installed":True,"name":query})
//!         if action=="browse": from hermes_cli.skills_hub import browse_skills; pg=int(params.get("page",0) or 0) or (int(query) if query.isdigit() else 1); return _ok(rid,browse_skills(page=pg,page_size=int(params.get("page_size",20))))
//!         if action=="inspect": from hermes_cli.skills_hub import inspect_skill; return _ok(rid,{"info":inspect_skill(query) or {}})
//!         return _err(rid,4017,f"unknown skills action: {action}")
//!     except Exception as e: return _err(rid,5024,str(e))
//!     finally: if token is not None: try: from hermes_constants import reset_hermes_home_override; reset_hermes_home_override(token) except: pass
//!
//! @method("mcp.catalog")
//! def _(rid, params: dict) -> dict:
//!     profile=str(params.get("profile") or "").strip(); token=None
//!     try: if profile: from hermes_cli.profiles import get_profile_dir; from hermes_constants import set_hermes_home_override; profile_dir=get_profile_dir(profile); if not profile_dir or not profile_dir.is_dir(): return _err(rid,4064,f"profile '{profile}' not found"); token=set_hermes_home_override(str(profile_dir))
//!          from hermes_cli import mcp_catalog; out=[]; for entry in mcp_catalog.list_catalog(): try: requires=[str(k) for k in (getattr(entry,"env_keys",None) or [])] except: requires=[]; out.append({"name":entry.name,"description":getattr(entry,"description","") or "","installed":bool(mcp_catalog.is_installed(entry.name)),"enabled":bool(mcp_catalog.is_enabled(entry.name)),"requires":requires,"transport":str(getattr(getattr(entry,"transport",None),"kind","") or getattr(entry,"transport","") or "stdio")}); return _ok(rid,{"servers":out})
//!     except Exception as e: return _err(rid,5024,str(e))
//!     finally: if token is not None: try: from hermes_constants import reset_hermes_home_override; reset_hermes_home_override(token) except: pass
//!
//! # ─── Per-profile MCP server lifecycle (mcp.servers.*) ────────────────────────
//! # Gateway RPCs mirroring dashboard REST surface (hermes_cli/web_routers/mcp.py) so desktop plugin can manage MCP servers for ANY profile
//! # Shared helpers (resolve_profile/reset_profile/summarize_server) live in tui_gateway.mcp_rpc_helpers
//!
//! @method("mcp.servers.list")
//! def _(rid, params: dict) -> dict:
//!     token,err=_mcp_resolve_profile(rid,params); if err: return err
//!     try: from hermes_cli.mcp_config import _get_mcp_servers; servers=_get_mcp_servers(); return _ok(rid,{"servers":[_mcp_summarize_server(name,cfg) for name,cfg in sorted(servers.items())]})
//!     except Exception as e: return _err(rid,5024,str(e))
//!     finally: _mcp_reset_profile(token)
//!
//! @method("mcp.servers.add")
//! def _(rid, params: dict) -> dict:
//!     name=str(params.get("name") or "").strip(); if not name: return _err(rid,4063,"name required")
//!     token,err=_mcp_resolve_profile(rid,params); if err: return err
//!     try: from hermes_cli.mcp_config import _apply_mcp_preset,_get_mcp_servers,_save_bearer_auth_token,_save_mcp_server; if name in _get_mcp_servers(): return _err(rid,4090,f"server '{name}' already exists"); preset=str(params.get("preset") or "").strip(); raw_cfg=params.get("config"); server_config=dict(raw_cfg) if isinstance(raw_cfg,dict) else {};
//!          if preset: _apply_mcp_preset(name,preset_name=preset,url=server_config.get("url"),command=server_config.get("command"),cmd_args=list(server_config.get("args") or []),server_config=server_config)
//!          if not server_config.get("url") and not server_config.get("command"): return _err(rid,4063,"config must specify a 'url' (http) or 'command' (stdio), or a valid 'preset'")
//!          bearer_token=params.get("bearer_token"); if bearer_token: server_config["headers"]=_save_bearer_auth_token(name,str(bearer_token))
//!          if not _save_mcp_server(name,server_config): return _err(rid,4001,f"server '{name}' rejected: suspicious command/args configuration"); saved=_get_mcp_servers().get(name,server_config); return _ok(rid,{"ok":True,"name":name,"server":_mcp_summarize_server(name,saved)})
//!     except Exception as e: return _err(rid,5024,str(e))
//!     finally: _mcp_reset_profile(token)
//!
//! @method("mcp.servers.set_api_key")
//! def _(rid, params: dict) -> dict:
//!     name=str(params.get("name") or "").strip(); if not name: return _err(rid,4063,"name required"); value=params.get("value"); if value is None or str(value)=="": return _err(rid,4063,"value required")
//!     token,err=_mcp_resolve_profile(rid,params); if err: return err
//!     try: from hermes_cli.config import load_config,save_config,save_env_value; from hermes_cli.mcp_config import _bearer_auth_headers,_env_key_for_server,_get_mcp_servers,_strip_bearer_prefix; servers=_get_mcp_servers(); if name not in servers: return _err(rid,4064,f"server '{name}' not found"); env_var=str(params.get("env_var") or "").strip() or _env_key_for_server(name); entry=servers[name]; if not isinstance(entry,dict): return _err(rid,4001,"malformed server config"); if entry.get("url"): normalized=_strip_bearer_prefix(str(value)); if not normalized or normalized.lower()=="bearer": return _err(rid,4063,"value is not a valid credential"); save_env_value(env_var,normalized); headers=_bearer_auth_headers(name) if env_var==_env_key_for_server(name) else {"Authorization":f"Bearer ${{{env_var}}}"}; entry["headers"]=headers; else: save_env_value(env_var,str(value)); env_block=entry.get("env"); if not isinstance(env_block,dict): env_block={}; env_block[env_var]=f"${{{env_var}}}"; entry["env"]=env_block; cfg=load_config(); cfg.setdefault("mcp_servers",{})[name]=entry; save_config(cfg); return _ok(rid,{"ok":True,"name":name,"env_var":env_var,"server":_mcp_summarize_server(name,entry)})
//!     except Exception as e: return _err(rid,5024,str(e))
//!     finally: _mcp_reset_profile(token)
//!
//! @method("mcp.servers.test")
//! def _(rid, params: dict) -> dict:
//!     name=str(params.get("name") or "").strip(); if not name: return _err(rid,4063,"name required")
//!     token,err=_mcp_resolve_profile(rid,params); if err: return err
//!     try: from hermes_cli.mcp_config import _get_mcp_servers,_oauth_tokens_present,_probe_single_server; servers=_get_mcp_servers(); if name not in servers: return _err(rid,4064,f"server '{name}' not found"); cfg=servers[name]; needs_oauth_token=cfg.get("auth")=="oauth"; details={}; try: tools=_probe_single_server(name,cfg,details=details); token_present=_oauth_tokens_present(name) if needs_oauth_token else True; except Exception as exc: return _ok(rid,{"ok":False,"error":str(exc),"tools":[],"oauth_needed":needs_oauth_token,"oauth_tokens_present":_oauth_tokens_present(name) if needs_oauth_token else None}); if not token_present: return _ok(rid,{"ok":False,"error":"OAuth authentication required — no token found.","tools":[],"oauth_needed":True,"oauth_tokens_present":False}); return _ok(rid,{"ok":True,"tools":[{"name":t,"description":d} for t,d in tools],"prompts":details.get("prompts",0),"resources":details.get("resources",0),"oauth_needed":needs_oauth_token,"oauth_tokens_present":True if needs_oauth_token else None})
//!     except Exception as e: return _err(rid,5024,str(e))
//!     finally: _mcp_reset_profile(token)
//!
//! @method("mcp.servers.remove")
//! def _(rid, params: dict) -> dict:
//!     name=str(params.get("name") or "").strip(); if not name: return _err(rid,4063,"name required")
//!     token,err=_mcp_resolve_profile(rid,params); if err: return err
//!     try: from hermes_cli.mcp_config import _remove_mcp_server; removed=_remove_mcp_server(name); if not removed: return _err(rid,4064,f"server '{name}' not found"); return _ok(rid,{"ok":True,"removed":True})
//!     except Exception as e: return _err(rid,5024,str(e))
//!     finally: _mcp_reset_profile(token)
//!
//! @method("mcp.servers.oauth.start")
//! def _(rid, params: dict) -> dict:
//!     name=str(params.get("name") or "").strip(); if not name: return _err(rid,4063,"name required")
//!     token,err=_mcp_resolve_profile(rid,params); if err: return err
//!     try: from hermes_cli.mcp_config import _get_mcp_servers; from hermes_constants import get_hermes_home; from tui_gateway import mcp_oauth_sessions; servers=_get_mcp_servers(); if name not in servers: return _err(rid,4064,f"server '{name}' not found"); cfg=dict(servers[name]); if not cfg.get("url"): return _err(rid,4001,"stdio servers authenticate via env keys, not OAuth"); if cfg.get("headers") and cfg.get("auth")!="oauth": return _err(rid,4001,"this server uses header/API-key auth, not OAuth"); cfg["auth"]="oauth"; hermes_home=str(get_hermes_home().expanduser().resolve(strict=False)); result=mcp_oauth_sessions.start_flow(hermes_home,name,cfg); return _ok(rid,{"ok":True,"session_id":result["session_id"],"auth_url":result["auth_url"],"flow":result["flow"]})
//!     except Exception as e: return _err(rid,5024,str(e))
//!     finally: _mcp_reset_profile(token)
//!
//! @method("mcp.servers.oauth.poll")
//! def _(rid, params: dict) -> dict:
//!     name=str(params.get("name") or "").strip(); if not name: return _err(rid,4063,"name required"); session_id=str(params.get("session_id") or "").strip(); if not session_id: return _err(rid,4063,"session_id required")
//!     token,err=_mcp_resolve_profile(rid,params); if err: return err
//!     try: from tui_gateway import mcp_oauth_sessions; result=mcp_oauth_sessions.poll_flow(session_id,name); return _ok(rid,{"ok":True,**result})
//!     except Exception as e: return _err(rid,5024,str(e))
//!     finally: _mcp_reset_profile(token)
//!
//! @method("skills.reload")
//! def _(rid, params: dict) -> dict:
//!     try: from agent.skill_commands import reload_skills; result=reload_skills(); added=result.get("added") or []; removed=result.get("removed") or []; total=int(result.get("total") or 0); lines=["Reloading skills..."]; if not added and not removed: lines.append("No new skills detected."); if added: lines.append("Added skills:"); lines.extend(f"  - {item.get('name','')}" for item in added); if removed: lines.append("Removed skills:"); lines.extend(f"  - {item.get('name','')}" for item in removed); lines.append(f"{total} skill(s) available"); return _ok(rid,{"output":"\n".join(lines),"result":result})
//!     except Exception as e: return _err(rid,5025,str(e))
//!
//! @method("plugins.manage")
//! def _(rid, params: dict) -> dict:
//!     action=params.get("action","list")
//!     token,err=_mcp_resolve_profile(rid,params); if err: return err
//!     try: from hermes_cli.plugins_cmd import _bundled_default_on,_discover_all_plugins,_get_disabled_set,_get_enabled_set,_is_portable_plugin_dir,_plugin_status; def _rows(): enabled=_get_enabled_set(); disabled=_get_disabled_set(); out=[]; for name,version,desc,source,_dir,key in sorted(_discover_all_plugins()): status=_plugin_status(name,enabled,disabled,key=key); if status=="not enabled" and source=="bundled" and _bundled_default_on(_dir): status="enabled"; out.append({"name":name,"key":key,"version":str(version or ""),"description":desc or "","source":source,"status":status,"portable":_is_portable_plugin_dir(_dir)}); return out
//!          if action=="list": rows=_rows(); user_count=sum(1 for r in rows if r["source"]!="bundled"); return _ok(rid,{"plugins":rows,"user_count":user_count,"bundled_count":len(rows)-user_count})
//!          if action=="toggle": from hermes_cli.plugins_cmd import dashboard_set_agent_plugin_enabled; ident=(params.get("key") or params.get("name") or "").strip(); if not ident: return _err(rid,4019,"plugins.toggle requires a 'key' or 'name'"); enable=bool(params.get("enable")); result=dashboard_set_agent_plugin_enabled(ident,enabled=enable); if not result.get("ok"): return _err(rid,5026,result.get("error") or "toggle failed"); row=next((r for r in _rows() if ident in (r["key"],r["name"])),None); return _ok(rid,{"ok":True,"unchanged":bool(result.get("unchanged")),"name":ident,"plugin":row})
//!          if action=="install": from hermes_cli.plugins_cmd import dashboard_install_plugin; ident=(params.get("identifier") or params.get("repo") or "").strip(); if not ident: return _err(rid,4019,"plugins.install requires 'identifier' or 'repo'"); result=dashboard_install_plugin(ident,force=bool(params.get("force")),enable=params.get("enable",True)); if not result.get("ok"): return _err(rid,5026,result.get("error") or "install failed"); return _ok(rid,result)
//!          return _err(rid,4017,f"unknown plugins action: {action}")
//!     except Exception as e: return _err(rid,5026,str(e))
//!     finally: _mcp_reset_profile(token)
//!
//! @method("shell.exec")
//! def _(rid, params: dict) -> dict:
//!     cmd=params.get("command",""); if not cmd: return _err(rid,4004,"empty command")
//!     try: from tools.approval import detect_dangerous_command,detect_hardline_command; is_hardline,hardline_desc=detect_hardline_command(cmd); if is_hardline: return _err(rid,4005,f"blocked (hardline): {hardline_desc}. Use the agent for dangerous commands."); is_dangerous,_,desc=detect_dangerous_command(cmd); if is_dangerous: return _err(rid,4005,f"blocked: {desc}. Use the agent for dangerous commands.")
//!     except ImportError: return _err(rid,5001,"shell.exec unavailable: approval safety module not importable")
//!     try: from hermes_cli._subprocess_compat import windows_hide_flags; r=subprocess.run(cmd,shell=True,capture_output=True,text=True,timeout=30,cwd=os.getcwd(),encoding="utf-8",errors="replace",stdin=subprocess.DEVNULL,creationflags=windows_hide_flags()); return _ok(rid,{"stdout":r.stdout[-4000:],"stderr":r.stderr[-2000:],"code":r.returncode})
//!     except subprocess.TimeoutExpired: return _err(rid,5002,"command timed out (30s)")
//!     except Exception as e: return _err(rid,5003,str(e))
//!
//! def register(server) -> None: _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType` rebinding
//!   no-op notes).
//! * `learning.detail` — `id` `5000` → [`handle_learning_detail`] (closure owns
//!   `node_detail` + `5000` mapping).
//! * `learning.delete` — `id` `5000` → [`handle_learning_delete`].
//! * `learning.edit` — `id`+`content` `5000` → [`handle_learning_edit`].
//! * `skills.manage` — `profile` `4064`/`5024` + `action` `4017` +
//!   `list/search/install/browse/inspect` all `5024` except success → [`handle_skills_manage`]
//!   (handler-side validates unknown `4017` pre-dispatch is optional; closure carries
//!   `HERMES_HOME` override `set_hermes_home_override`/`reset` in `finally`).
//! * `mcp.catalog` — `profile` `4064`/`5024` + `list_catalog`/`is_installed`/`is_enabled` → [`handle_mcp_catalog`].
//! * `mcp.servers.list` — `_mcp_resolve_profile` `4064` → `5024` → [`handle_mcp_servers_list`].
//! * `mcp.servers.add` — `name` `4063` + `4090` exists + `4063` url/command + `4001` suspicious +
//!   `bearer_token` `save_bearer_auth_token` + `_save_mcp_server` → [`handle_mcp_servers_add`]
//!   (handler-side validates `name` `4063`).
//! * `mcp.servers.set_api_key` — `name` `4063` + `value` `4063` + `4064` not found + `4001` malformed + `4063` bad credential → [`handle_mcp_servers_set_api_key`].
//! * `mcp.servers.test` — `name` `4063` + `4064` + `ok:false` vs `ok:true` probe `5024` → [`handle_mcp_servers_test`].
//! * `mcp.servers.remove` — `name` `4063` + `4064` + `5024` → [`handle_mcp_servers_remove`].
//! * `mcp.servers.oauth.start` — `name` `4063` + `4064` + stdio `4001` + header-vs-oauth `4001` + `5024` → [`handle_mcp_servers_oauth_start`].
//! * `mcp.servers.oauth.poll` — `name` `4063` + `session_id` `4063` + `5024` → [`handle_mcp_servers_oauth_poll`].
//! * `skills.reload` — `reload_skills` `added/removed/total` lines `5025` → [`handle_skills_reload`].
//! * `plugins.manage` — `action` `list` (`plugins`+`user_count`+`bundled_count`) / `toggle` (`key`/`name` `4019` + `5026`) / `install` (`identifier`/`repo` `4019` + `5026`) + unknown `4017` → [`handle_plugins_manage`].
//! * `shell.exec` — `command` empty `4004` + hardline/dangerous `4005` + `ImportError` `5001` + timeout `5002` + other `5003` → [`handle_shell_exec`]
//!   (handler-side validates `4004`).
//! * `is_truthy_value` not needed in this slice (used in slice 2 cron); no new truthy helper here.
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → [`ok_response`] / [`err_response`] (mirrors `server.py::_ok` / `_err`).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] / [`build_registry`] / [`build_registry_default`] (deferred via `HandlerRegistry`).

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators for 1800-2579
// ---------------------------------------------------------------------------

pub const METHOD_LEARNING_DETAIL: &str = "learning.detail";
pub const METHOD_LEARNING_DELETE: &str = "learning.delete";
pub const METHOD_LEARNING_EDIT: &str = "learning.edit";
pub const METHOD_SKILLS_MANAGE: &str = "skills.manage";
pub const METHOD_MCP_CATALOG: &str = "mcp.catalog";
pub const METHOD_MCP_SERVERS_LIST: &str = "mcp.servers.list";
pub const METHOD_MCP_SERVERS_ADD: &str = "mcp.servers.add";
pub const METHOD_MCP_SERVERS_SET_API_KEY: &str = "mcp.servers.set_api_key";
pub const METHOD_MCP_SERVERS_TEST: &str = "mcp.servers.test";
pub const METHOD_MCP_SERVERS_REMOVE: &str = "mcp.servers.remove";
pub const METHOD_MCP_SERVERS_OAUTH_START: &str = "mcp.servers.oauth.start";
pub const METHOD_MCP_SERVERS_OAUTH_POLL: &str = "mcp.servers.oauth.poll";
pub const METHOD_SKILLS_RELOAD: &str = "skills.reload";
pub const METHOD_PLUGINS_MANAGE: &str = "plugins.manage";
pub const METHOD_SHELL_EXEC: &str = "shell.exec";

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_LEARNING_DETAIL: i32 = 5000;
pub const ERR_LEARNING_DELETE: i32 = 5000;
pub const ERR_LEARNING_EDIT: i32 = 5000;
pub const ERR_SKILLS_MANAGE: i32 = 5024;
pub const ERR_SKILLS_MANAGE_PROFILE: i32 = 4064;
pub const ERR_SKILLS_UNKNOWN_ACTION: i32 = 4017;
pub const ERR_MCP_CATALOG: i32 = 5024;
pub const ERR_MCP_PROFILE_NOT_FOUND: i32 = 4064;
pub const ERR_MCP_NAME_REQUIRED: i32 = 4063;
pub const ERR_MCP_SERVER_NOT_FOUND: i32 = 4064;
pub const ERR_MCP_SERVER_EXISTS: i32 = 4090;
pub const ERR_MCP_MALFORMED: i32 = 4001;
pub const ERR_MCP_SERVERS: i32 = 5024;
pub const ERR_SKILLS_RELOAD: i32 = 5025;
pub const ERR_PLUGINS_UNKNOWN_ACTION: i32 = 4017;
pub const ERR_PLUGINS_IDENT_REQUIRED: i32 = 4019;
pub const ERR_PLUGINS_MANAGE: i32 = 5026;
pub const ERR_SHELL_EMPTY: i32 = 4004;
pub const ERR_SHELL_BLOCKED: i32 = 4005;
pub const ERR_SHELL_UNAVAILABLE: i32 = 5001;
pub const ERR_SHELL_TIMEOUT: i32 = 5002;
pub const ERR_SHELL_FAILED: i32 = 5003;

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
// Action / param helpers for this slice
// ---------------------------------------------------------------------------

pub fn extract_action(params_json: &str) -> String {
    extract_string_field(params_json, "action").unwrap_or_else(|| "list".to_string())
}

pub fn is_valid_skills_action(action: &str) -> bool {
    matches!(action, "list" | "search" | "install" | "browse" | "inspect")
}

pub fn is_valid_plugins_action(action: &str) -> bool {
    matches!(action, "list" | "toggle" | "install")
}

pub fn extract_name_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "name")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn extract_session_id_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "session_id")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn extract_command_param(params_json: &str) -> String {
    extract_string_field(params_json, "command").unwrap_or_default()
}

pub fn is_empty_command(cmd: &str) -> bool {
    cmd.trim().is_empty()
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `learning.detail`.
///
/// `op` mirrors `node_detail(str(params.get("id","")))` → `Ok(result_json)` where
/// result_json is the node detail payload; `Err(msg)` maps to `5000`.
pub fn handle_learning_detail<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_LEARNING_DETAIL, &e),
    }
}

/// Handle `learning.delete`.
pub fn handle_learning_delete<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_LEARNING_DELETE, &e),
    }
}

/// Handle `learning.edit`.
pub fn handle_learning_edit<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_LEARNING_EDIT, &e),
    }
}

/// Handle `skills.manage`.
///
/// `op` mirrors `get_profile_dir` + `set_hermes_home_override` (`4064`/`5024`) +
/// `get_available_skills`/`unified_search`/`do_install`/`browse_skills`/`inspect_skill` →
/// `Ok(result_json)`; unknown action `4017` and exception `5024` are `Err((code,msg))`.
pub fn handle_skills_manage<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    // Validate unknown action handler-side for std-only traceability (mirrors `return _err(rid,4017,...)`).
    let action = extract_string_field(params_json, "action")
        .unwrap_or_else(|| "list".to_string());
    if !is_valid_skills_action(&action) {
        return err_response(rid_json, ERR_SKILLS_UNKNOWN_ACTION, &format!("unknown skills action: {}", action));
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `mcp.catalog`.
///
/// `op` mirrors `get_profile_dir` + `set_hermes_home_override` + `mcp_catalog.list_catalog()` →
/// `{"servers":[...]}`; profile not found `4064` and other `5024` are `Err`.
pub fn handle_mcp_catalog<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `mcp.servers.list`.
pub fn handle_mcp_servers_list<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `mcp.servers.add`.
///
/// Validates `name` required (`4063`) handler-side (pre-lock check), then
/// delegates `4090`/`4001`/`5024` etc. to `op`.
pub fn handle_mcp_servers_add<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let name = extract_string_field(params_json, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return err_response(rid_json, ERR_MCP_NAME_REQUIRED, "name required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `mcp.servers.set_api_key`.
///
/// Validates `name` `4063` and `value` `4063` handler-side.
pub fn handle_mcp_servers_set_api_key<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let name = extract_string_field(params_json, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return err_response(rid_json, ERR_MCP_NAME_REQUIRED, "name required");
    }
    let value = extract_raw_value(params_json, "value");
    let empty = match &value {
        None => true,
        Some(v) => {
            let t = v.trim().trim_matches('"').trim();
            t.is_empty() || t == "null"
        }
    };
    if empty {
        // also check string field fallback for empty string
        let s = extract_string_field(params_json, "value").unwrap_or_default();
        if s.trim().is_empty() {
            return err_response(rid_json, ERR_MCP_NAME_REQUIRED, "value required");
        }
    }
    // Second check: if raw was present but value is empty string literal, ensure error
    let s_val = extract_string_field(params_json, "value").unwrap_or_default();
    if s_val.trim().is_empty() && empty {
        return err_response(rid_json, ERR_MCP_NAME_REQUIRED, "value required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `mcp.servers.test`.
///
/// Validates `name` `4063` handler-side; probe `ok:false` vs `ok:true` stays in closure (`_ok`).
pub fn handle_mcp_servers_test<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let name = extract_string_field(params_json, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return err_response(rid_json, ERR_MCP_NAME_REQUIRED, "name required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `mcp.servers.remove`.
pub fn handle_mcp_servers_remove<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let name = extract_string_field(params_json, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return err_response(rid_json, ERR_MCP_NAME_REQUIRED, "name required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `mcp.servers.oauth.start`.
pub fn handle_mcp_servers_oauth_start<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let name = extract_string_field(params_json, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return err_response(rid_json, ERR_MCP_NAME_REQUIRED, "name required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `mcp.servers.oauth.poll`.
///
/// Validates `name` `4063` and `session_id` `4063` handler-side.
pub fn handle_mcp_servers_oauth_poll<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let name = extract_string_field(params_json, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return err_response(rid_json, ERR_MCP_NAME_REQUIRED, "name required");
    }
    let sid = extract_string_field(params_json, "session_id").unwrap_or_default().trim().to_string();
    if sid.is_empty() {
        return err_response(rid_json, ERR_MCP_NAME_REQUIRED, "session_id required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `skills.reload`.
///
/// `op` mirrors `reload_skills()` → `{"output":...,"result":...}`; `Err`→`5025`.
pub fn handle_skills_reload<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_SKILLS_RELOAD, &e),
    }
}

/// Handle `plugins.manage`.
///
/// `op` mirrors `_mcp_resolve_profile` + `_discover_all_plugins` + dashboard helpers →
/// `Ok(result_json)` where result_json is `{"plugins":...}` for `list`,
/// `{"ok":true,"plugin":...}` for `toggle`, or install result; `Err((4017|4019|5026,msg))`.
pub fn handle_plugins_manage<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let action = extract_string_field(params_json, "action").unwrap_or_else(|| "list".to_string());
    if !is_valid_plugins_action(&action) {
        return err_response(rid_json, ERR_PLUGINS_UNKNOWN_ACTION, &format!("unknown plugins action: {}", action));
    }
    if action == "toggle" {
        let ident = extract_string_field(params_json, "key")
            .or_else(|| extract_string_field(params_json, "name"))
            .unwrap_or_default().trim().to_string();
        if ident.is_empty() {
            return err_response(rid_json, ERR_PLUGINS_IDENT_REQUIRED, "plugins.toggle requires a 'key' or 'name'");
        }
    }
    if action == "install" {
        let ident = extract_string_field(params_json, "identifier")
            .or_else(|| extract_string_field(params_json, "repo"))
            .unwrap_or_default().trim().to_string();
        if ident.is_empty() {
            return err_response(rid_json, ERR_PLUGINS_IDENT_REQUIRED, "plugins.install requires 'identifier' or 'repo'");
        }
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `shell.exec`.
///
/// Validates `command` empty `4004` handler-side, then delegates
/// hardline/dangerous `4005`, unavailable `5001`, timeout `5002`, other `5003`.
pub fn handle_shell_exec<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let cmd = extract_string_field(params_json, "command").unwrap_or_default();
    if cmd.trim().is_empty() {
        // also check raw maybe missing
        let raw = extract_raw_value(params_json, "command");
        match raw {
            None => return err_response(rid_json, ERR_SHELL_EMPTY, "empty command"),
            Some(v) => {
                let t = v.trim().trim_matches('"').trim();
                if t.is_empty() || t == "null" {
                    return err_response(rid_json, ERR_SHELL_EMPTY, "empty command");
                }
            }
        }
        if cmd.trim().is_empty() {
            return err_response(rid_json, ERR_SHELL_EMPTY, "empty command");
        }
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with the 15 slice-3 methods registered.
///
/// Each closure is `'static` and mirrors the lazy imports inside Python handler
/// bodies. For the default stub (no backend) use [`build_registry_default`].
#[allow(clippy::too_many_arguments)]
pub fn build_registry<LD, LX, LE, SM, MC, ML, MA, MK, MT, MR, OS, OP, SR, PM, SH>(
    learning_detail: LD,
    learning_delete: LX,
    learning_edit: LE,
    skills_manage: SM,
    mcp_catalog: MC,
    mcp_servers_list: ML,
    mcp_servers_add: MA,
    mcp_servers_set_api_key: MK,
    mcp_servers_test: MT,
    mcp_servers_remove: MR,
    mcp_servers_oauth_start: OS,
    mcp_servers_oauth_poll: OP,
    skills_reload: SR,
    plugins_manage: PM,
    shell_exec: SH,
) -> HandlerRegistry
where
    LD: Fn(String, String) -> String + Send + Sync + 'static,
    LX: Fn(String, String) -> String + Send + Sync + 'static,
    LE: Fn(String, String) -> String + Send + Sync + 'static,
    SM: Fn(String, String) -> String + Send + Sync + 'static,
    MC: Fn(String, String) -> String + Send + Sync + 'static,
    ML: Fn(String, String) -> String + Send + Sync + 'static,
    MA: Fn(String, String) -> String + Send + Sync + 'static,
    MK: Fn(String, String) -> String + Send + Sync + 'static,
    MT: Fn(String, String) -> String + Send + Sync + 'static,
    MR: Fn(String, String) -> String + Send + Sync + 'static,
    OS: Fn(String, String) -> String + Send + Sync + 'static,
    OP: Fn(String, String) -> String + Send + Sync + 'static,
    SR: Fn(String, String) -> String + Send + Sync + 'static,
    PM: Fn(String, String) -> String + Send + Sync + 'static,
    SH: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        learning_detail,
        learning_delete,
        learning_edit,
        skills_manage,
        mcp_catalog,
        mcp_servers_list,
        mcp_servers_add,
        mcp_servers_set_api_key,
        mcp_servers_test,
        mcp_servers_remove,
        mcp_servers_oauth_start,
        mcp_servers_oauth_poll,
        skills_reload,
        plugins_manage,
        shell_exec,
    );
    reg
}

/// Build a registry with default stubs (no backend / no file I/O).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_learning_detail(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_learning_delete(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_learning_edit(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_skills_manage(&rid_json, &params_json, |_| Err((ERR_SKILLS_MANAGE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_catalog(&rid_json, &params_json, |_| Err((ERR_MCP_CATALOG, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_list(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_add(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_set_api_key(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_test(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_remove(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_oauth_start(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_oauth_poll(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_skills_reload(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_plugins_manage(&rid_json, &params_json, |_| Err((ERR_PLUGINS_MANAGE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_shell_exec(&rid_json, &params_json, |_| Err((ERR_SHELL_FAILED, "no backend".to_string())))
        },
    )
}

/// Register all 15 slice-3 methods onto an existing registry.
#[allow(clippy::too_many_arguments)]
pub fn register_with<LD, LX, LE, SM, MC, ML, MA, MK, MT, MR, OS, OP, SR, PM, SH>(
    registry: &mut HandlerRegistry,
    learning_detail: LD,
    learning_delete: LX,
    learning_edit: LE,
    skills_manage: SM,
    mcp_catalog: MC,
    mcp_servers_list: ML,
    mcp_servers_add: MA,
    mcp_servers_set_api_key: MK,
    mcp_servers_test: MT,
    mcp_servers_remove: MR,
    mcp_servers_oauth_start: OS,
    mcp_servers_oauth_poll: OP,
    skills_reload: SR,
    plugins_manage: PM,
    shell_exec: SH,
) where
    LD: Fn(String, String) -> String + Send + Sync + 'static,
    LX: Fn(String, String) -> String + Send + Sync + 'static,
    LE: Fn(String, String) -> String + Send + Sync + 'static,
    SM: Fn(String, String) -> String + Send + Sync + 'static,
    MC: Fn(String, String) -> String + Send + Sync + 'static,
    ML: Fn(String, String) -> String + Send + Sync + 'static,
    MA: Fn(String, String) -> String + Send + Sync + 'static,
    MK: Fn(String, String) -> String + Send + Sync + 'static,
    MT: Fn(String, String) -> String + Send + Sync + 'static,
    MR: Fn(String, String) -> String + Send + Sync + 'static,
    OS: Fn(String, String) -> String + Send + Sync + 'static,
    OP: Fn(String, String) -> String + Send + Sync + 'static,
    SR: Fn(String, String) -> String + Send + Sync + 'static,
    PM: Fn(String, String) -> String + Send + Sync + 'static,
    SH: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method(METHOD_LEARNING_DETAIL, learning_detail);
    registry.method(METHOD_LEARNING_DELETE, learning_delete);
    registry.method(METHOD_LEARNING_EDIT, learning_edit);
    registry.method(METHOD_SKILLS_MANAGE, skills_manage);
    registry.method(METHOD_MCP_CATALOG, mcp_catalog);
    registry.method(METHOD_MCP_SERVERS_LIST, mcp_servers_list);
    registry.method(METHOD_MCP_SERVERS_ADD, mcp_servers_add);
    registry.method(METHOD_MCP_SERVERS_SET_API_KEY, mcp_servers_set_api_key);
    registry.method(METHOD_MCP_SERVERS_TEST, mcp_servers_test);
    registry.method(METHOD_MCP_SERVERS_REMOVE, mcp_servers_remove);
    registry.method(METHOD_MCP_SERVERS_OAUTH_START, mcp_servers_oauth_start);
    registry.method(METHOD_MCP_SERVERS_OAUTH_POLL, mcp_servers_oauth_poll);
    registry.method(METHOD_SKILLS_RELOAD, skills_reload);
    registry.method(METHOD_PLUGINS_MANAGE, plugins_manage);
    registry.method(METHOD_SHELL_EXEC, shell_exec);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_learning_detail(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_learning_delete(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_learning_edit(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_skills_manage(&rid_json, &params_json, |_| Err((ERR_SKILLS_MANAGE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_catalog(&rid_json, &params_json, |_| Err((ERR_MCP_CATALOG, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_list(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_add(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_set_api_key(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_test(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_remove(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_oauth_start(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_servers_oauth_poll(&rid_json, &params_json, |_| Err((ERR_MCP_SERVERS, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_skills_reload(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_plugins_manage(&rid_json, &params_json, |_| Err((ERR_PLUGINS_MANAGE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_shell_exec(&rid_json, &params_json, |_| Err((ERR_SHELL_FAILED, "no backend".to_string())))
        },
    )
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn rid1() -> String {
        encode_rid("1")
    }

    #[test]
    fn learning_detail_delete_edit_ok_and_err() {
        let rid = rid1();
        let ok = handle_learning_detail(&rid, r#"{"id":"node1"}"#, |_| Ok(r#"{"id":"node1","content":"hello"}"#.to_string()));
        assert!(ok.contains("node1"), "{}", ok);
        let err = handle_learning_detail(&rid, "{}", |_| Err("learning.detail failed: boom".into()));
        assert!(err.contains(r#""code":5000"#), "{}", err);
        assert!(err.contains("learning.detail failed"));

        let ok2 = handle_learning_delete(&rid, r#"{"id":"n2"}"#, |_| Ok(r#"{"deleted":true}"#.to_string()));
        assert!(ok2.contains("deleted"), "{}", ok2);
        let err2 = handle_learning_delete(&rid, "{}", |_| Err("learning.delete failed: x".into()));
        assert!(err2.contains(r#""code":5000"#));

        let ok3 = handle_learning_edit(&rid, r#"{"id":"n3","content":"new"}"#, |_| Ok(r#"{"ok":true}"#.to_string()));
        assert!(ok3.contains("ok"), "{}", ok3);
        let err3 = handle_learning_edit(&rid, "{}", |_| Err("learning.edit failed: y".into()));
        assert!(err3.contains(r#""code":5000"#));
    }

    #[test]
    fn skills_manage_unknown_action_and_ok() {
        let rid = rid1();
        let out = handle_skills_manage(&rid, r#"{"action":"bogus","query":"x"}"#, |_| Ok("{}".into()));
        assert!(out.contains(r#""code":4017"#), "{}", out);
        assert!(out.contains("bogus"));

        let out2 = handle_skills_manage(&rid, r#"{"action":"list"}"#, |_| Ok(r#"{"skills":[]}"#.to_string()));
        assert!(out2.contains("skills"), "{}", out2);

        let out3 = handle_skills_manage(&rid, r#"{"action":"search","query":"github"}"#, |_| Ok(r#"{"results":[{"name":"a","description":"b"}]}"#.into()));
        assert!(out3.contains("results"), "{}", out3);

        let out4 = handle_skills_manage(&rid, r#"{"action":"list"}"#, |_| Err((4064, "profile 'x' not found".into())));
        assert!(out4.contains(r#""code":4064"#), "{}", out4);

        let out5 = handle_skills_manage(&rid, r#"{"action":"list"}"#, |_| Err((5024, "boom".into())));
        assert!(out5.contains(r#""code":5024"#));
    }

    #[test]
    fn mcp_catalog_ok_and_profile_err() {
        let rid = rid1();
        let ok = handle_mcp_catalog(&rid, "{}", |_| Ok(r#"{"servers":[{"name":"a","description":"desc","installed":true,"enabled":false,"requires":[],"transport":"stdio"}]}"#.to_string()));
        assert!(ok.contains("servers"), "{}", ok);
        assert!(ok.contains("stdio"));

        let err = handle_mcp_catalog(&rid, r#"{"profile":"bad"}"#, |_| Err((4064, "profile 'bad' not found".into())));
        assert!(err.contains(r#""code":4064"#));

        let err2 = handle_mcp_catalog(&rid, "{}", |_| Err((5024, "fail".into())));
        assert!(err2.contains(r#""code":5024"#));
    }

    #[test]
    fn mcp_servers_list_ok_and_err() {
        let rid = rid1();
        let ok = handle_mcp_servers_list(&rid, "{}", |_| Ok(r#"{"servers":[{"name":"s1","transport":"http","enabled":true}]}"#.to_string()));
        assert!(ok.contains("s1"), "{}", ok);
        let err = handle_mcp_servers_list(&rid, "{}", |_| Err((5024, "boom".into())));
        assert!(err.contains(r#""code":5024"#));
        let err2 = handle_mcp_servers_list(&rid, r#"{"profile":"x"}"#, |_| Err((4064, "profile 'x' not found".into())));
        assert!(err2.contains(r#""code":4064"#));
    }

    #[test]
    fn mcp_servers_add_validation_and_ok() {
        let rid = rid1();
        let out = handle_mcp_servers_add(&rid, r#"{"name":""}"#, |_| panic!("should not call on empty name"));
        assert!(out.contains(r#""code":4063"#), "{}", out);
        assert!(out.contains("name required"));
        let out2 = handle_mcp_servers_add(&rid, r#"{}"#, |_| panic!("should not call"));
        assert!(out2.contains(r#""code":4063"#));

        let ok = handle_mcp_servers_add(&rid, r#"{"name":"myserver","config":{"url":"https://x"}}"#, |_| Ok(r#"{"ok":true,"name":"myserver","server":{"name":"myserver"}}"#.to_string()));
        assert!(ok.contains("myserver"), "{}", ok);

        let dup = handle_mcp_servers_add(&rid, r#"{"name":"exists","config":{"url":"https://x"}}"#, |_| Err((4090, "server 'exists' already exists".into())));
        assert!(dup.contains(r#""code":4090"#));

        let bad_cfg = handle_mcp_servers_add(&rid, r#"{"name":"n","config":{}}"#, |_| Err((4063, "config must specify a 'url'".into())));
        assert!(bad_cfg.contains(r#""code":4063"#));

        let suspicious = handle_mcp_servers_add(&rid, r#"{"name":"n","config":{"command":"evil"}}"#, |_| Err((4001, "server 'n' rejected: suspicious".into())));
        assert!(suspicious.contains(r#""code":4001"#));
    }

    #[test]
    fn mcp_servers_set_api_key_validation() {
        let rid = rid1();
        let out = handle_mcp_servers_set_api_key(&rid, r#"{"name":""}"#, |_| panic!("should not call"));
        assert!(out.contains(r#""code":4063"#));
        assert!(out.contains("name required"));

        let out2 = handle_mcp_servers_set_api_key(&rid, r#"{"name":"s","value":""}"#, |_| panic!("should not call"));
        assert!(out2.contains(r#""code":4063"#), "{}", out2);
        assert!(out2.contains("value required"));

        let out3 = handle_mcp_servers_set_api_key(&rid, r#"{"name":"s"}"#, |_| panic!("should not call"));
        assert!(out3.contains(r#""code":4063"#));

        let ok = handle_mcp_servers_set_api_key(&rid, r#"{"name":"s","value":"tok123"}"#, |_| Ok(r#"{"ok":true,"name":"s","env_var":"MCP_S_API_KEY","server":{}}"#.to_string()));
        assert!(ok.contains("MCP_S_API_KEY"), "{}", ok);

        let notfound = handle_mcp_servers_set_api_key(&rid, r#"{"name":"bad","value":"v"}"#, |_| Err((4064, "server 'bad' not found".into())));
        assert!(notfound.contains(r#""code":4064"#));

        let bad_cred = handle_mcp_servers_set_api_key(&rid, r#"{"name":"s","value":"bearer"}"#, |_| Err((4063, "value is not a valid credential".into())));
        assert!(bad_cred.contains(r#""code":4063"#));
    }

    #[test]
    fn mcp_servers_test_poll_validation() {
        let rid = rid1();
        let out = handle_mcp_servers_test(&rid, r#"{"name":""}"#, |_| panic!("should not call"));
        assert!(out.contains(r#""code":4063"#));

        let ok = handle_mcp_servers_test(&rid, r#"{"name":"s"}"#, |_| Ok(r#"{"ok":true,"tools":[{"name":"t","description":"d"}],"prompts":0,"resources":0}"#.to_string()));
        assert!(ok.contains("tools"), "{}", ok);
        assert!(ok.contains("t"));

        let probe_fail = handle_mcp_servers_test(&rid, r#"{"name":"s"}"#, |_| Ok(r#"{"ok":false,"error":"timeout","tools":[],"oauth_needed":false}"#.to_string()));
        assert!(probe_fail.contains("ok"), "{}", probe_fail);
        assert!(probe_fail.contains("false"));

        let notfound = handle_mcp_servers_test(&rid, r#"{"name":"bad"}"#, |_| Err((4064, "server 'bad' not found".into())));
        assert!(notfound.contains(r#""code":4064"#));

        let out2 = handle_mcp_servers_remove(&rid, r#"{"name":""}"#, |_| panic!("should not call"));
        assert!(out2.contains(r#""code":4063"#));
        let ok2 = handle_mcp_servers_remove(&rid, r#"{"name":"s"}"#, |_| Ok(r#"{"ok":true,"removed":true}"#.to_string()));
        assert!(ok2.contains("removed"), "{}", ok2);

        let out3 = handle_mcp_servers_oauth_start(&rid, r#"{"name":""}"#, |_| panic!("should not call"));
        assert!(out3.contains(r#""code":4063"#));
        let ok3 = handle_mcp_servers_oauth_start(&rid, r#"{"name":"s"}"#, |_| Ok(r#"{"ok":true,"session_id":"sid","auth_url":"https://auth","flow":"pkce"}"#.to_string()));
        assert!(ok3.contains("auth_url"), "{}", ok3);
        let stdio_err = handle_mcp_servers_oauth_start(&rid, r#"{"name":"stdio_s"}"#, |_| Err((4001, "stdio servers authenticate via env keys, not OAuth".into())));
        assert!(stdio_err.contains(r#""code":4001"#));

        let out4 = handle_mcp_servers_oauth_poll(&rid, r#"{"name":"s","session_id":""}"#, |_| panic!("should not call missing sid"));
        assert!(out4.contains(r#""code":4063"#));
        assert!(out4.contains("session_id required"));
        let out5 = handle_mcp_servers_oauth_poll(&rid, r#"{"name":"","session_id":"sid"}"#, |_| panic!("should not call missing name"));
        assert!(out5.contains(r#""code":4063"#));
        let ok4 = handle_mcp_servers_oauth_poll(&rid, r#"{"name":"s","session_id":"sid123"}"#, |_| Ok(r#"{"ok":true,"status":"approved"}"#.to_string()));
        assert!(ok4.contains("approved"), "{}", ok4);
    }

    #[test]
    fn skills_reload_ok_and_err() {
        let rid = rid1();
        let ok = handle_skills_reload(&rid, "{}", |_| Ok(r#"{"output":"Reloading skills...\nNo new skills detected.\n2 skill(s) available","result":{"added":[],"removed":[],"total":2}}"#.to_string()));
        assert!(ok.contains("Reloading"), "{}", ok);
        assert!(ok.contains("skill(s) available"));
        let err = handle_skills_reload(&rid, "{}", |_| Err("boom".into()));
        assert!(err.contains(r#""code":5025"#));
    }

    #[test]
    fn plugins_manage_validation_and_ok() {
        let rid = rid1();
        let out = handle_plugins_manage(&rid, r#"{"action":"weird"}"#, |_| panic!("should not call"));
        assert!(out.contains(r#""code":4017"#), "{}", out);
        assert!(out.contains("weird"));

        let out2 = handle_plugins_manage(&rid, r#"{"action":"toggle"}"#, |_| panic!("should not call missing ident"));
        assert!(out2.contains(r#""code":4019"#), "{}", out2);
        assert!(out2.contains("toggle"));

        let out3 = handle_plugins_manage(&rid, r#"{"action":"toggle","name":""}"#, |_| panic!("should not call"));
        assert!(out3.contains(r#""code":4019"#));

        let out4 = handle_plugins_manage(&rid, r#"{"action":"install"}"#, |_| panic!("should not call"));
        assert!(out4.contains(r#""code":4019"#));

        let ok_list = handle_plugins_manage(&rid, r#"{"action":"list"}"#, |_| Ok(r#"{"plugins":[{"name":"a","key":"a","version":"1","description":"desc","source":"bundled","status":"enabled","portable":false}],"user_count":0,"bundled_count":1}"#.to_string()));
        assert!(ok_list.contains("plugins"), "{}", ok_list);
        assert!(ok_list.contains("bundled_count"));

        let ok_toggle = handle_plugins_manage(&rid, r#"{"action":"toggle","key":"a","enable":true}"#, |_| Ok(r#"{"ok":true,"unchanged":false,"name":"a","plugin":{"name":"a"}}"#.to_string()));
        assert!(ok_toggle.contains("ok"), "{}", ok_toggle);

        let ok_install = handle_plugins_manage(&rid, r#"{"action":"install","identifier":"org/repo"}"#, |_| Ok(r#"{"ok":true,"name":"org/repo"}"#.to_string()));
        assert!(ok_install.contains("org/repo"), "{}", ok_install);

        let err = handle_plugins_manage(&rid, r#"{"action":"list"}"#, |_| Err((5026, "toggle failed".into())));
        assert!(err.contains(r#""code":5026"#));
    }

    #[test]
    fn shell_exec_empty_and_blocked_and_ok() {
        let rid = rid1();
        let out = handle_shell_exec(&rid, r#"{"command":""}"#, |_| panic!("should not call on empty"));
        assert!(out.contains(r#""code":4004"#), "{}", out);
        assert!(out.contains("empty command"));

        let out2 = handle_shell_exec(&rid, r#"{}"#, |_| panic!("should not call"));
        assert!(out2.contains(r#""code":4004"#));

        let out3 = handle_shell_exec(&rid, r#"{"command":"echo hi"}"#, |_| Ok(r#"{"stdout":"hi","stderr":"","code":0}"#.to_string()));
        assert!(out3.contains("stdout"), "{}", out3);
        assert!(out3.contains("hi"));

        let blocked = handle_shell_exec(&rid, r#"{"command":"rm -rf /"}"#, |_| Err((4005, "blocked (hardline): rm -rf".into())));
        assert!(blocked.contains(r#""code":4005"#));

        let unavailable = handle_shell_exec(&rid, r#"{"command":"echo hi"}"#, |_| Err((5001, "shell.exec unavailable".into())));
        assert!(unavailable.contains(r#""code":5001"#));

        let timeout = handle_shell_exec(&rid, r#"{"command":"sleep 40"}"#, |_| Err((5002, "command timed out (30s)".into())));
        assert!(timeout.contains(r#""code":5002"#));

        let fail = handle_shell_exec(&rid, r#"{"command":"false"}"#, |_| Err((5003, "boom".into())));
        assert!(fail.contains(r#""code":5003"#));
    }

    #[test]
    fn extract_helpers_and_validation() {
        assert_eq!(extract_string_field(r#"{"name":"myserver"}"#, "name").as_deref(), Some("myserver"));
        assert_eq!(extract_string_field(r#"{"session_id":"sid123"}"#, "session_id").as_deref(), Some("sid123"));
        assert_eq!(extract_name_param(r#"{"name":"s1"}"#).as_deref(), Some("s1"));
        assert_eq!(extract_name_param(r#"{"name":""}"#), None);
        assert_eq!(extract_session_id_param(r#"{"session_id":"abc"}"#).as_deref(), Some("abc"));
        assert!(is_valid_skills_action("list"));
        assert!(is_valid_skills_action("search"));
        assert!(!is_valid_skills_action("bogus"));
        assert!(is_valid_plugins_action("list"));
        assert!(is_valid_plugins_action("toggle"));
        assert!(!is_valid_plugins_action("bad"));
        assert_eq!(extract_action(r#"{"action":"search"}"#), "search");
        assert_eq!(extract_action(r#"{}"#), "list");
        assert!(is_empty_command("   "));
        assert!(!is_empty_command("echo hi"));
        assert_eq!(extract_command_param(r#"{"command":"ls"}"#), "ls");
        assert_eq!(extract_raw_value(r#"{"command":"echo hi"}"#, "command").unwrap(), "\"echo hi\"");
    }

    #[test]
    fn registry_installs_all_fifteen() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 15);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["learning.delete","learning.detail","learning.edit","mcp.catalog","mcp.servers.add","mcp.servers.list","mcp.servers.oauth.poll","mcp.servers.oauth.start","mcp.servers.remove","mcp.servers.set_api_key","mcp.servers.test","plugins.manage","shell.exec","skills.manage","skills.reload"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 15);
        // learning.detail stub -> 5000
        let out = map.get(METHOD_LEARNING_DETAIL).unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains(r#""code":5000"#), "{}", out);
        // skills.manage unknown action -> 4017
        let out2 = map.get(METHOD_SKILLS_MANAGE).unwrap()("1".to_string(), r#"{"action":"bogus"}"#.to_string());
        assert!(out2.contains(r#""code":4017"#), "{}", out2);
        // mcp.servers.add without name -> 4063
        let out3 = map.get(METHOD_MCP_SERVERS_ADD).unwrap()("1".to_string(), "{}".to_string());
        assert!(out3.contains(r#""code":4063"#), "{}", out3);
        // mcp.servers.set_api_key without name -> 4063
        let out4 = map.get(METHOD_MCP_SERVERS_SET_API_KEY).unwrap()("1".to_string(), "{}".to_string());
        assert!(out4.contains(r#""code":4063"#), "{}", out4);
        // mcp.servers.oauth.poll without session_id -> 4063
        let out5 = map.get(METHOD_MCP_SERVERS_OAUTH_POLL).unwrap()("1".to_string(), r#"{"name":"s"}"#.to_string());
        assert!(out5.contains(r#""code":4063"#), "{}", out5);
        // plugins.manage unknown -> 4017
        let out6 = map.get(METHOD_PLUGINS_MANAGE).unwrap()("1".to_string(), r#"{"action":"bad"}"#.to_string());
        assert!(out6.contains(r#""code":4017"#), "{}", out6);
        // shell.exec without command -> 4004
        let out7 = map.get(METHOD_SHELL_EXEC).unwrap()("1".to_string(), "{}".to_string());
        assert!(out7.contains(r#""code":4004"#), "{}", out7);
        // mcp.servers.test without name -> 4063
        let out8 = map.get(METHOD_MCP_SERVERS_TEST).unwrap()("1".to_string(), "{}".to_string());
        assert!(out8.contains(r#""code":4063"#), "{}", out8);
        // skills.reload stub -> 5025
        let out9 = map.get(METHOD_SKILLS_RELOAD).unwrap()("1".to_string(), "{}".to_string());
        assert!(out9.contains(r#""code":5025"#), "{}", out9);
        // shell.exec ok path inject - direct handle
        let ok = handle_shell_exec(&encode_rid("1"), r#"{"command":"echo hi"}"#, |_| Ok(r#"{"stdout":"hi","stderr":"","code":0}"#.to_string()));
        assert!(ok.contains("hi"), "{}", ok);
    }

    #[test]
    fn ok_err_envelope_shape() {
        let rid = encode_rid("42");
        let ok = ok_response(&rid, r#"{"ok":true}"#);
        assert!(ok.contains(r#""result""#));
        assert!(ok.contains("ok"));
        let err = err_response(&rid, 4004, "empty command");
        assert!(err.contains(r#""code":4004"#));
        assert!(err.contains("empty command"));
        let err2 = err_response(&rid, 5000, "learning.detail failed: boom");
        assert!(err2.contains(r#""code":5000"#));
    }
}
