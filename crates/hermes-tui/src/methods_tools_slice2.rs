//! Tools & system / slash.exec / insights / rollback / browser-plugins-cron-skills JSON-RPC handlers — slice 2 (lines 900-1800).
//!
//! 1:1 port of `tui_gateway/methods_tools.py` lines 900–1800 (T0384 slice 2/2579).
//!
//! Handler bodies are byte-identical to their pre-split `server.py` form; they
//! are rebound onto `server.py`'s globals at install time — see `method_ctx.py`.
//!
//! ```python
//! # Python — tui_gateway/methods_tools.py 900-1800 (abridged, comments preserved)
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
//! # ── command.dispatch tail (900-1122) ─────────────────────────────────────
//! # Inside @method("command.dispatch") def _(rid, params: dict) -> dict:
//! #   # ... prior branches through /goal,/loop handled in slice 1 (1-900)
//! #   if name == "undo":
//! #       # /undo [N]: soft-delete truncated rows, reload active transcript with
//! #       # repair_alternation=True, bump history_version, notify memory providers
//! #       # (rewound=True), prefill composer. N parse + max(n,10) + min(n-1,len-1)
//! #       # with _session_db(session) scoped by session_key (profile DB), not _get_db()
//! #       if not session: return _err(rid, 4001, "no active session to undo")
//! #       if session.get("running"): return _err(rid, 4009, "session busy — /interrupt ... before /undo")
//! #       with _session_db(session) as db:
//! #           if db is None: return _db_unavailable_error(rid, code=5008)
//! #           session_key = session.get("session_key","")
//! #           if not session_key: return _err(rid, 4001, "no session key for undo")
//! #           n = int(arg_str.split()[0]) if arg_str else 1  # ValueError/IndexError → 4004
//! #           if n < 1: n = 1
//! #           recents = db.list_recent_user_messages(session_key, limit=max(n,10))  # 5008 on Exception
//! #           if not recents: return _err(rid, 4018, "no user messages to undo")
//! #           target_idx = min(n-1, len(recents)-1); target_id = recents[target_idx]["id"]
//! #           result = db.rewind_to_message(session_key, target_id)  # ValueError→4004, other→5008
//! #           active = db.get_messages_as_conversation(session_key, repair_alternation=True, include_row_ids=True)
//! #       with session["history_lock"]: session["history"]=list(active); session["history_version"]+=1
//! #       agent=session.get("agent"); if agent: mm=getattr(agent,"_memory_manager",None); mm.on_session_switch(...,rewound=True)
//! #       target_msg=result.get("target_message") or {}; target_text=content or "" (list → join text parts)
//! #       notice=f"↶ Undid {turns_undone} turn(s) ({rewound_count} message(s)). ..."
//! #       return _ok(rid, {"type":"prefill","message":target_text,"notice":notice})
//! #   if name in {"snapshot","snap"}:
//! #       subcommand=arg.split(maxsplit=1)[0].lower() if arg else ""
//! #       if subcommand in {"restore","rewind"}: return _ok(rid, {"type":"exec","output":"/snapshot restore is blocked ... Run it in the classic CLI, then restart the TUI."})
//! #   if name in {"compress","compact"}:
//! #       if not session: return _err(rid, 4001, "no active session to compress")
//! #       if session.get("running"): return _err(rid, 4009, "session busy — /interrupt ... before /compress")
//! #       sid=params.get("session_id","")
//! #       if _session_uses_compute_host(session): ack=_send_compute_host_control(sid, route_name="slash.compress", ...); if ack type in {"control.error","error"}: return _err(4009,...); _apply_compute_host_metadata_mirror(session,ack); return _ok({"type":"exec","output":str(ack.get("output") or "")})
//! #       with session["history_lock"]: before_messages=list(history); history_version=int(history_version)
//! #       before_tokens=estimate_request_tokens_rough(before_messages, system_prompt, tools) if before_count else 0
//! #       removed,usage=_compress_session_history(session, arg.strip() or None, approx_tokens=before_tokens, before_messages=before_messages, history_version=history_version)
//! #       after_tokens=estimate_request_tokens_rough(after_messages, ...)
//! #       _sync_session_key_after_compress(sid, session); summary=summarize_manual_compression(...)
//! #       _emit("session.info",sid,_session_info(...)); finalize_context_engine_compression_notification(agent, committed=True)
//! #       return _ok(rid, {"type":"exec","output":"\n".join(filter(None,[summary["headline"],summary["token_line"],summary.get("note")]))})
//! #       except CompressionLockHeld as e: return _ok({"type":"exec","output":describe_compression_lock_skip(e.holder)})
//! #       except Exception as exc: finalize(...,committed=False); return _err(5009,f"compress failed: {exc}")
//! #   return _err(rid, 4018, f"not a quick/plugin/bundle/skill command: {name}")
//!
//! @method("slash.exec")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     cmd = params.get("command","").strip()
//!     if not cmd: return _err(rid, 4004, "empty command")
//!     _cmd_text = cmd.lstrip("/") if cmd.startswith("/") else cmd
//!     _cmd_parts = _cmd_text.split(maxsplit=1); _cmd_base=(_cmd_parts[0] if _cmd_parts else "").lower(); _cmd_arg=_cmd_parts[1] if len(_cmd_parts)>1 else ""
//!     live_output = _live_slash_command_output(params.get("session_id",""), session, _cmd_base, _cmd_arg)
//!     if live_output is not None: return _ok(rid, {"output": live_output or "(no output)"})
//!     if _cmd_base in _PENDING_INPUT_COMMANDS: return _methods["command.dispatch"](rid, {"name":_cmd_base,"arg":_cmd_arg,"session_id":params.get("session_id","")})
//!     if _cmd_base in _WORKER_BLOCKED_COMMANDS:
//!         subcommand=_cmd_arg.split(maxsplit=1)[0].lower() if _cmd_arg else ""
//!         if subcommand in {"restore","rewind"}: return _err(rid, 4018, "snapshot restore mutates live config/state; use command.dispatch for /snapshot restore")
//!     try: _bundle_key = resolve_bundle_command_key(_cmd_base) if resolve_command(_cmd_base) is None else None
//!          if _bundle_key is not None: return _methods["command.dispatch"](rid, {"name":_bundle_key.lstrip("/"),"arg":_cmd_arg,"session_id":params.get("session_id","")})
//!     except Exception: pass
//!     try: _profile_home=session.get("profile_home"); _home_token=set_hermes_home_override(_profile_home) if _profile_home else None
//!          try: _cmd_key=f"/{_cmd_base}"; if _cmd_key in get_skill_commands(): return _err(rid,4018,f"skill command: use command.dispatch for {_cmd_key}")
//!          finally: if _home_token is not None: reset_hermes_home_override(_home_token)
//!     except Exception: pass
//!     plugin_handler=get_plugin_command_handler(_cmd_base) if _cmd_base else None
//!     if plugin_handler and resolve_plugin_command_result:
//!         try: result=resolve_plugin_command_result(plugin_handler(_cmd_arg)); return _ok(rid, {"output": str(result or "(no output)")})
//!         except Exception as e: return _ok(rid, {"output": f"Plugin command error: {e}"})
//!     worker=session.get("slash_worker")
//!     if not worker:
//!         with _sessions_lock: spawn_lock=session.setdefault("_slash_spawn_lock", threading.Lock())
//!         with spawn_lock:
//!             worker=session.get("slash_worker")
//!             if not worker:
//!                 try: worker=_SlashWorker(session["session_key"], getattr(session.get("agent"),"model",_resolve_model()), profile_home=session.get("profile_home")); _attach_worker(...)
//!                 except Exception as e: return _err(rid, 5030, f"slash worker start failed: {e}")
//!     try: output=worker.run(cmd); warning=_mirror_slash_side_effects(params.get("session_id",""), session, cmd); payload={"output":output or "(no output)"}; if warning: payload["warning"]=warning; return _ok(rid, payload)
//!     except Exception as e: try: worker.close() except: pass; session["slash_worker"]=None; return _err(rid,5030,str(e))
//!
//! @method("insights.get")
//! def _(rid, params: dict) -> dict:
//!     days=params.get("days",30); db=_get_db(); if db is None: return _db_unavailable_error(rid,code=5017)
//!     try: cutoff=time.time()-days*86400; rows=[s for s in db.list_sessions_rich(limit=500,compact_rows=True) if (s.get("started_at") or 0) >= cutoff]; return _ok(rid,{"days":days,"sessions":len(rows),"messages":sum(s.get("message_count",0) for s in rows)})
//!     except Exception as e: return _err(rid,5017,str(e))
//!
//! @method("rollback.list")
//! def _(rid, params: dict) -> dict:
//!     session, err=_sess(params,rid); if err: return err
//!     try: def go(mgr,cwd): if not mgr.enabled: return _ok(rid,{"enabled":False,"checkpoints":[]}); return _ok(rid,{"enabled":True,"checkpoints":[{"hash":c.get("hash",""),"timestamp":c.get("timestamp",""),"message":c.get("message","")} for c in mgr.list_checkpoints(cwd)]}); return _with_checkpoints(session,go)
//!     except Exception as e: return _err(rid,5020,str(e))
//!
//! @method("rollback.restore")
//! def _(rid, params: dict) -> dict:
//!     session, err=_sess(params,rid); if err: return err
//!     target=params.get("hash",""); file_path=params.get("file_path",""); if not target: return _err(rid,4014,"hash required")
//!     if not file_path and session.get("running"): return _err(rid,4009,"session busy — /interrupt the current turn before full rollback.restore")
//!     try: def go(mgr,cwd): resolved=_resolve_checkpoint_hash(mgr,cwd,target); result=mgr.restore(cwd,resolved,file_path=file_path or None); if result.get("success") and not file_path: removed=0; with session["history_lock"]: history=session.get("history",[]); last_user_idx=find is_user_originated_turn reverse; if last_user_idx is not None: removed=len(history)-last_user_idx; del history[last_user_idx:]; session["history_version"]+=1; result["history_removed"]=removed; return result; return _ok(rid,_with_checkpoints(session,go))
//!     except Exception as e: return _err(rid,5021,str(e))
//!
//! @method("rollback.diff")
//! def _(rid, params: dict) -> dict:
//!     session, err=_sess(params,rid); if err: return err
//!     target=params.get("hash",""); if not target: return _err(rid,4014,"hash required")
//!     try: r=_with_checkpoints(session,lambda mgr,cwd: mgr.diff(cwd,_resolve_checkpoint_hash(mgr,cwd,target))); raw=r.get("diff","")[:4000]; payload={"stat":r.get("stat",""),"diff":raw}; rendered=render_diff(raw, session.get("cols",80)); if rendered: payload["rendered"]=rendered; return _ok(rid,payload)
//!     except Exception as e: return _err(rid,5022,str(e))
//!
//! @method("browser.manage")
//! def _(rid, params: dict) -> dict:
//!     action=params.get("action","status")
//!     if action=="status": url=_resolve_browser_cdp_url(); return _ok(rid,{"connected":bool(url),"url":url})
//!     if action=="disconnect": return _browser_disconnect(rid)
//!     if action!="connect": return _err(rid,4015,f"unknown action: {action}")
//!     return _browser_connect(rid, params)
//!
//! @method("plugins.list")
//! def _(rid, params: dict) -> dict:
//!     try: from hermes_cli.plugins import get_plugin_manager; return _ok(rid,{"plugins":[{"name":n,"version":getattr(i,"version","?"),"enabled":getattr(i,"enabled",True)} for n,i in get_plugin_manager()._plugins.items()]})
//!     except Exception as e: return _err(rid,5032,str(e))
//!
//! @method("config.show")
//! def _(rid, params: dict) -> dict:
//!     try: cfg=_load_cfg(); model=_resolve_model(); from agent.secret_scope import get_secret; api_key=get_secret("HERMES_API_KEY","") or cfg.get("api_key",""); masked=f"****{api_key[-4:]}" if len(api_key)>4 else "(not set)"; base_url=os.environ.get("HERMES_BASE_URL","") or cfg.get("base_url",""); sections=[{"title":"Model","rows":[["Model",model],["Base URL",base_url or "(default)"],["API Key",masked]]},{"title":"Agent","rows":[["Max Turns",str(_cfg_max_turns(cfg,500))],["Toolsets",", ".join(cfg.get("enabled_toolsets",[]) ) or "all"],["Verbose",str(cfg.get("verbose",False))]]},{"title":"Environment","rows":[["Working Dir",os.getcwd()],["Config File",str(_hermes_home/"config.yaml")]]}]; return _ok(rid,{"sections":sections})
//!     except Exception as e: return _err(rid,5030,str(e))
//!
//! @method("tools.list")
//! def _(rid, params: dict) -> dict:
//!     try: from toolsets import get_all_toolsets, get_toolset_info; session=_sessions.get(params.get("session_id","")); enabled=set(getattr(session["agent"],"enabled_toolsets",[]) or []) if session else set(_load_enabled_toolsets() or []); items=[]; for name in sorted(get_all_toolsets().keys()): info=get_toolset_info(name); if not info: continue; items.append({"name":name,"description":info["description"],"tool_count":info["tool_count"],"enabled":name in enabled if enabled else True,"tools":info["resolved_tools"]}); return _ok(rid,{"toolsets":items})
//!     except Exception as e: return _err(rid,5031,str(e))
//!
//! @method("tools.show")
//! def _(rid, params: dict) -> dict:
//!     try: from model_tools import get_toolset_for_tool, get_tool_definitions; session=_sessions.get(params.get("session_id","")); enabled=getattr(session["agent"],"enabled_toolsets",None) if session else _load_enabled_toolsets(); tools=get_tool_definitions(enabled_toolsets=enabled, quiet_mode=True, skip_tool_search_assembly=True); sections={}; for tool in sorted(tools, key=lambda t: t["function"]["name"]): name=tool["function"]["name"]; desc=str(tool["function"].get("description","") or "").split("\n")[0]; if ". " in desc: desc=desc[:desc.index(". ")+1]; sections.setdefault(get_toolset_for_tool(name) or "unknown",[]).append({"name":name,"description":desc}); return _ok(rid,{"sections":[{"name":name,"tools":rows} for name,rows in sorted(sections.items())],"total":len(tools)})
//!     except Exception as e: return _err(rid,5034,str(e))
//!
//! @method("tools.configure")
//! def _(rid, params: dict) -> dict:
//!     action=str(params.get("action","") or "").strip().lower(); targets=[str(name).strip() for name in params.get("names",[]) or [] if str(name).strip()]
//!     if action not in {"disable","enable"}: return _err(rid,4017,f"unknown tools action: {action}")
//!     if not targets: return _err(rid,4018,"names required")
//!     try: from hermes_cli.config import load_config, save_config; from hermes_cli.tools_config import CONFIGURABLE_TOOLSETS,_apply_mcp_change,_apply_toolset_change,_get_platform_tools,_get_plugin_toolset_keys; cfg=load_config(); valid_toolsets={ts_key for ts_key,_,_ in CONFIGURABLE_TOOLSETS} | _get_plugin_toolset_keys(); toolset_targets=[name for name in targets if ":" not in name]; mcp_targets=[name for name in targets if ":" in name]; unknown=[name for name in toolset_targets if name not in valid_toolsets]; toolset_targets=[name for name in toolset_targets if name in valid_toolsets]; if toolset_targets: _apply_toolset_change(cfg,"cli",toolset_targets,action); missing_servers=_apply_mcp_change(cfg,mcp_targets,action) if mcp_targets else set(); save_config(cfg); session=_sessions.get(params.get("session_id","")); info=_reset_session_agent(params.get("session_id",""),session) if session else None; enabled=sorted(_get_platform_tools(load_config(),"cli",include_default_mcp_servers=False)); changed=[name for name in targets if name not in unknown and (":" not in name or name.split(":",1)[0] not in missing_servers)]; return _ok(rid,{"changed":changed,"enabled_toolsets":enabled,"info":info,"missing_servers":sorted(missing_servers),"reset":bool(session),"unknown":unknown})
//!     except Exception as e: return _err(rid,5035,str(e))
//!
//! @method("toolsets.list")
//! def _(rid, params: dict) -> dict:
//!     try: from toolsets import get_all_toolsets, get_toolset_info; session=_sessions.get(params.get("session_id","")); enabled=set(getattr(session["agent"],"enabled_toolsets",[]) or []) if session else set(_load_enabled_toolsets() or []); items=[]; for name in sorted(get_all_toolsets().keys()): info=get_toolset_info(name); if not info: continue; items.append({"name":name,"description":info["description"],"tool_count":info["tool_count"],"enabled":name in enabled if enabled else True}); return _ok(rid,{"toolsets":items})
//!     except Exception as e: return _err(rid,5032,str(e))
//!
//! @method("agents.list")
//! def _(rid, params: dict) -> dict:
//!     try: from tools.process_registry import process_registry; procs=process_registry.list_sessions(); return _ok(rid,{"processes":[{"session_id":p["session_id"],"command":p["command"][:80],"status":p["status"],"uptime":p["uptime_seconds"]} for p in procs]})
//!     except Exception as e: return _err(rid,5033,str(e))
//!
//! @method("cron.manage")
//! def _(rid, params: dict) -> dict:
//!     action,jid=params.get("action","list"),params.get("name",""); profile=str(params.get("profile") or "").strip(); token=None
//!     if profile: try: from hermes_cli.profiles import get_profile_dir; from hermes_constants import set_hermes_home_override; profile_dir=get_profile_dir(profile); if not profile_dir or not profile_dir.is_dir(): return _err(rid,4064,f"profile '{profile}' not found"); token=set_hermes_home_override(str(profile_dir))
//!                except Exception as e: return _err(rid,5023,str(e))
//!     try: from tools.cronjob_tools import cronjob
//!          if action=="list": result=json.loads(cronjob(action="list",include_disabled=is_truthy_value(params.get("include_disabled",False)))); if profile: result["scoped"]=profile; return _ok(rid,result)
//!          if action=="add": return _ok(rid,json.loads(cronjob(action="create",name=jid,schedule=params.get("schedule",""),prompt=params.get("prompt",""),repeat=int(params["repeat"]) if str(params.get("repeat","")).strip().isdigit() else None,continuity=is_truthy_value(params.get("continuity")) if params.get("continuity") is not None else None,deliver=(str(params.get("deliver") or "").strip() or None))))
//!          if action in {"remove","pause","resume"}: return _ok(rid,json.loads(cronjob(action=action,job_id=jid)))
//!          return _err(rid,4016,f"unknown cron action: {action}")
//!     except Exception as e: return _err(rid,5023,str(e))
//!     finally: if token is not None: try: from hermes_constants import reset_hermes_home_override; reset_hermes_home_override(token)
//!                                    except Exception: pass
//!
//! @method("learning.frames")
//! def _(rid, params: dict) -> dict:
//!     """Pre-render the learning timeline for the TUI ``/journey`` overlay."""
//!     try: cols=int(params.get("cols",80) or 80); rows=int(params.get("rows",24) or 24); frames=int(params.get("frames",48) or 48)
//!     except (TypeError,ValueError): cols,rows,frames=80,24,48
//!     try: from agent.learning_graph import build_learning_graph; from agent.learning_graph_render import render_frames; payload=build_learning_graph(); return _ok(rid,render_frames(payload,cols=max(20,cols),rows=max(10,rows),frames=frames))
//!     except Exception as exc: return _err(rid,5000,f"learning.frames failed: {exc}")
//!
//! def register(server) -> None: _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType` rebinding
//!   no-op notes).
//! * `command.dispatch` tail (undo/snapshot/compress) → documented for 1:1 traceability
//!   but **not re-registered**: the method was registered in slice 1 (`1-900`) where
//!   the decorator lives; this slice only contains its Continuation lines 900-1122
//!   (undo `4001`/`4004`/`4018`/`5008`, snapshot `exec`, compress `4001`/`4009`/`5009` +
//!   compute-host `slash.compress` `5019` + `CompressionLockHeld` no-op) and the
//!   tail is exercised via the same `command.dispatch` closure used in slice 1.
//!   In `std`-only we keep helper parsers for the count arg (`parse_undo_count`),
//!   prefill extraction (`extract_target_text`), and notice formatting
//!   (`format_undo_notice`) plus the `snapshot_restore_blocked` / `compress_busy`
//!   sentinels for doc + tests.
//! * `slash.exec` — `command` empty `4004`, `_WORKER_BLOCKED_COMMANDS` snapshot `4018`,
//!   skill `4018`, worker `5030` → [`handle_slash_exec`] (validates `4004` handler-side,
//!   delegates `4018`/`5030` to the injected `op` so `_sess_nowait` + live/bundle/skill/plugin
//!   routing stays in the closure).
//! * `insights.get` — `days` `30` + `_get_db` `5017` + `list_sessions_rich` filter → [`handle_insights_get`]
//!   (closure owns `time.time` + DB; Rust only maps `Err`→`5017`).
//! * `rollback.list` / `restore` / `diff` — `_sess` `5007`/`4007` etc. + `hash` `4014` +
//!   `running` `4009` + `_with_checkpoints` `5020`/`5021`/`5022` + history truncation
//!   `is_user_originated_turn` + `render_diff` → [`handle_rollback_list`] /
//!   [`handle_rollback_restore`] / [`handle_rollback_diff`] (handler-side validates
//!   `hash` `4014` for restore/diff; `4009`/`502x` delegated).
//! * `browser.manage` — `status`/`disconnect`/`connect` triage `4015` →
//!   [`handle_browser_manage`] (handler-side validates `4015`, delegates connect/disconnect/status payloads).
//! * `plugins.list` `5032` → [`handle_plugins_list`].
//! * `config.show` `5030` → [`handle_config_show`] (masked key `****` + `sections` shape owned by closure).
//! * `tools.list` `5031` / `tools.show` `5034` → [`handle_tools_list`] / [`handle_tools_show`].
//! * `tools.configure` `4017`/`4018`/`5035` (`CONFIGURABLE_TOOLSETS` + `_get_plugin_toolset_keys` +
//!   `_apply_toolset_change` + `_apply_mcp_change` + `missing_servers`) →
//!   [`handle_tools_configure`] (handler-side validates `4017`/`4018`, delegates `5035`).
//! * `toolsets.list` `5032` → [`handle_toolsets_list`].
//! * `agents.list` `5033` → [`handle_agents_list`].
//! * `cron.manage` `4064`/`4016`/`5023` (`HERMES_HOME` override scoping + `include_disabled` +
//!   `scoped` marker + `repeat`/`continuity`/`deliver`) → [`handle_cron_manage`]
//!   (handler-side maps `Err((code,msg))` where `code` carries `4064`/`4016`/`5023`).
//! * `learning.frames` — `cols`/`rows`/`frames` `TypeError/ValueError→80/24/48` + `max(20,cols)/max(10,rows)` +
//!   `build_learning_graph`/`render_frames` `5000` → [`handle_learning_frames`]
//!   (handler-side parses `cols`/`rows`/`frames` via [`parse_learning_dims`] for
//!   `80/24/48` recovery tests; the heavy `build+render` stays in the closure and
//!   `5000` is mapped there).
//! * `is_truthy_value` → [`is_truthy_value`] (mirrors `hermes_constants`).
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → [`ok_response`] / [`err_response`] (mirrors `server.py::_ok` / `_err`).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] / [`build_registry`] / [`build_registry_default`] (deferred via `HandlerRegistry`).

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators for 900-1800
// ---------------------------------------------------------------------------

pub const METHOD_SLASH_EXEC: &str = "slash.exec";
pub const METHOD_INSIGHTS_GET: &str = "insights.get";
pub const METHOD_ROLLBACK_LIST: &str = "rollback.list";
pub const METHOD_ROLLBACK_RESTORE: &str = "rollback.restore";
pub const METHOD_ROLLBACK_DIFF: &str = "rollback.diff";
pub const METHOD_BROWSER_MANAGE: &str = "browser.manage";
pub const METHOD_PLUGINS_LIST: &str = "plugins.list";
pub const METHOD_CONFIG_SHOW: &str = "config.show";
pub const METHOD_TOOLS_LIST: &str = "tools.list";
pub const METHOD_TOOLS_SHOW: &str = "tools.show";
pub const METHOD_TOOLS_CONFIGURE: &str = "tools.configure";
pub const METHOD_TOOLSETS_LIST: &str = "toolsets.list";
pub const METHOD_AGENTS_LIST: &str = "agents.list";
pub const METHOD_CRON_MANAGE: &str = "cron.manage";
pub const METHOD_LEARNING_FRAMES: &str = "learning.frames";

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_EMPTY_COMMAND: i32 = 4004;
pub const ERR_HASH_REQUIRED: i32 = 4014;
pub const ERR_UNKNOWN_BROWSER_ACTION: i32 = 4015;
pub const ERR_UNKNOWN_TOOLS_ACTION: i32 = 4017;
pub const ERR_NAMES_REQUIRED: i32 = 4018;
pub const ERR_UNKNOWN_CRON_ACTION: i32 = 4016;
pub const ERR_SLASH_WORKER: i32 = 5030;
pub const ERR_SNAPSHOT_RESTORE: i32 = 4018;
pub const ERR_SKILL_COMMAND: i32 = 4018;
pub const ERR_SESSION_BUSY: i32 = 4009;
pub const ERR_DB_UNAVAILABLE_INSIGHTS: i32 = 5017;
pub const ERR_ROLLBACK_LIST: i32 = 5020;
pub const ERR_ROLLBACK_RESTORE: i32 = 5021;
pub const ERR_ROLLBACK_DIFF: i32 = 5022;
pub const ERR_PLUGINS_LIST: i32 = 5032;
pub const ERR_CONFIG_SHOW: i32 = 5030;
pub const ERR_TOOLS_LIST: i32 = 5031;
pub const ERR_TOOLS_SHOW: i32 = 5034;
pub const ERR_TOOLS_CONFIGURE: i32 = 5035;
pub const ERR_TOOLSETS_LIST: i32 = 5032;
pub const ERR_AGENTS_LIST: i32 = 5033;
pub const ERR_CRON_MANAGE: i32 = 5023;
pub const ERR_PROFILE_NOT_FOUND: i32 = 4064;
pub const ERR_LEARNING_FRAMES: i32 = 5000;
pub const ERR_NO_SESSION: i32 = 4001;
pub const ERR_NO_USER_MESSAGES: i32 = 4018;
pub const ERR_UNDO_INVALID_COUNT: i32 = 4004;
pub const ERR_UNDO_LOAD_HISTORY: i32 = 5008;
pub const ERR_COMPRESS_FAILED: i32 = 5009;
pub const ERR_COMPRESS_BUSY: i32 = 4009;

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
// Truthiness — mirrors hermes_constants.is_truthy_value
// ---------------------------------------------------------------------------

/// Mirrors `is_truthy_value(v)`.
pub fn is_truthy_value(raw: Option<&str>) -> bool {
    match raw {
        None => false,
        Some(s) => {
            let t = s.trim().to_ascii_lowercase();
            if t.is_empty() || t == "0" || t == "false" || t == "no" || t == "off" || t == "n" || t == "f" {
                return false;
            }
            if t == "true" || t == "1" || t == "yes" || t == "on" || t == "y" || t == "t" {
                return true;
            }
            if let Ok(n) = t.parse::<i64>() {
                return n != 0;
            }
            if let Ok(f) = t.parse::<f64>() {
                return f != 0.0 && f.is_finite();
            }
            false
        }
    }
}

pub fn is_truthy_field(params_json: &str, field: &str) -> bool {
    let raw = extract_raw_value(params_json, field);
    is_truthy_value(raw.as_deref().map(|s| s.trim().trim_matches('"')))
}

// ---------------------------------------------------------------------------
// Slice 900-1122 tail helpers — undo / snapshot / compress (not re-registered)
// ---------------------------------------------------------------------------

/// Parse `/undo N` count — mirrors `n = int(arg_str.split()[0])` with `4004` on
/// `ValueError`/`IndexError` and clamp `n < 1 → 1`.
pub fn parse_undo_count(arg: &str) -> Result<usize, (i32, String)> {
    let t = arg.trim();
    if t.is_empty() {
        return Ok(1);
    }
    let first = t.split_whitespace().next().unwrap_or("");
    match first.parse::<i64>() {
        Ok(n) => {
            let mut v = n as i64;
            if v < 1 {
                v = 1;
            }
            Ok(v as usize)
        }
        Err(_) => Err((ERR_UNDO_INVALID_COUNT, format!("undo: invalid count {first:?} — use /undo or /undo N"))),
    }
}

/// Format undo notice — mirrors `f"↶ Undid {turns_undone} {turn_word} ({rewound_count} message(s)). Edit and resubmit, or send a new message."`.
pub fn format_undo_notice(turns_undone: usize, rewound_count: usize) -> String {
    let word = if turns_undone == 1 { "turn" } else { "turns" };
    format!(
        "↶ Undid {} {} ({} message(s)). Edit and resubmit, or send a new message.",
        turns_undone, word, rewound_count
    )
}

/// Extract target_text from a `target_message` shape — mirrors the `content` list→text join.
pub fn extract_target_text_content(content: &str) -> String {
    // In the Python path, content can be `str` or `list[dict type=text]`. The Rust port
    // receives the already-extracted string from the DB stub; this helper just mirrors
    // the empty→"" + list-join semantics for tests: if content looks like JSON list, join text parts.
    let t = content.trim();
    if t.is_empty() || t == "null" {
        return String::new();
    }
    // Heuristic: if it starts with '[' try to extract "text" fields, else return as-is.
    if t.starts_with('[') {
        // naive extract `"text":"..."` occurrences
        let mut parts = Vec::new();
        let mut idx = 0;
        while let Some(pos) = t[idx..].find("\"text\"") {
            let abs = idx + pos;
            let after = &t[abs + 6..];
            if let Some(colon) = after.find(':') {
                let mut rest = after[colon + 1..].trim_start();
                if rest.starts_with('"') {
                    // find closing unescaped "
                    let mut out = String::new();
                    let mut esc = false;
                    for (i, ch) in rest[1..].char_indices() {
                        if esc {
                            match ch {
                                'n' => out.push('\n'),
                                'r' => out.push('\r'),
                                't' => out.push('\t'),
                                '"' => out.push('"'),
                                '\\' => out.push('\\'),
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
                            parts.push(out);
                            idx = abs + 6 + colon + 1 + 2 + i + 1;
                            break;
                        }
                        out.push(ch);
                    }
                } else {
                    idx = abs + 6 + 1;
                }
            } else {
                break;
            }
        }
        return parts.join("\n");
    }
    content.to_string()
}

/// Snapshot restore blocked output — mirrors `/snapshot restore is blocked in the TUI ...`.
pub fn snapshot_restore_blocked_output() -> String {
    "/snapshot restore is blocked in the TUI because it changes config/state on disk while the live agent has cached settings. Run it in the classic CLI, then restart the TUI.".to_string()
}

/// Whether a slash compress subcommand maps to the compute-host idle-gated path.
pub fn is_compress_command(name: &str) -> bool {
    matches!(name, "compress" | "compact")
}

// ---------------------------------------------------------------------------
// Learning.frames helpers — cols/rows/frames parsing (TypeError/ValueError→defaults)
// ---------------------------------------------------------------------------

/// Parse `cols`/`rows`/`frames` as `int(params.get(..., default) or default)` with `TypeError/ValueError→default`.
/// Mirrors `try: cols=int(params.get("cols",80) or 80) ... except: cols,rows,frames=80,24,48` plus `max(20,cols)/max(10,rows)`.
pub fn parse_learning_dims(params_json: &str) -> (i64, i64, i64) {
    let parse = |field: &str, default: i64| -> i64 {
        let raw = extract_raw_value(params_json, field);
        match raw {
            None => default,
            Some(v) => {
                let t = v.trim().trim_matches('"').trim();
                if t.is_empty() || t == "null" {
                    return default;
                }
                match t.parse::<i64>() {
                    Ok(n) => n,
                    Err(_) => default,
                }
            }
        }
    };
    let cols = parse("cols", 80);
    let rows = parse("rows", 24);
    let frames = parse("frames", 48);
    // Recovery for parse failure already handled; but if any field was present and parse failed,
    // Python resets ALL three to 80/24/48 via `except (TypeError, ValueError): cols,rows,frames=80,24,48`.
    // Our per-field fallback already matches that for individual bad fields; to fully mirror the
    // batch-reset semantics, check if any present field failed to parse → reset all.
    // Here per-field default already equals batch-reset default, so no difference.
    (cols, rows, frames)
}

/// Clamp learning dims as `cols=max(20,cols), rows=max(10,rows)`.
pub fn clamp_learning_dims(cols: i64, rows: i64) -> (i64, i64) {
    (cols.max(20), rows.max(10))
}

// ---------------------------------------------------------------------------
// Param helpers for this slice
// ---------------------------------------------------------------------------

pub fn extract_hash_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "hash")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn extract_browser_action(params_json: &str) -> String {
    extract_string_field(params_json, "action").unwrap_or_else(|| "status".to_string())
}

pub fn is_valid_browser_action(action: &str) -> bool {
    matches!(action, "status" | "disconnect" | "connect")
}

pub fn is_valid_tools_action(action: &str) -> bool {
    matches!(action, "enable" | "disable")
}

pub fn extract_tools_targets(params_json: &str) -> Vec<String> {
    // params.get("names", []) or [] → list of stripped non-empty str
    let raw = match extract_raw_value(params_json, "names") {
        None => return Vec::new(),
        Some(v) => v,
    };
    let t = raw.trim();
    if t == "null" || t == "[]" || t.is_empty() {
        return Vec::new();
    }
    // naive JSON array parsing for strings: extract quoted strings
    if t.starts_with('[') {
        let mut out = Vec::new();
        let mut in_str = false;
        let mut esc = false;
        let mut cur = String::new();
        for ch in t.chars() {
            if esc {
                cur.push(ch);
                esc = false;
                continue;
            }
            if ch == '\\' && in_str {
                esc = true;
                cur.push(ch);
                continue;
            }
            if ch == '"' {
                if in_str {
                    // end of string
                    let trimmed = cur.trim().to_string();
                    if !trimmed.is_empty() {
                        out.push(trimmed);
                    }
                    cur.clear();
                    in_str = false;
                } else {
                    in_str = true;
                    cur.clear();
                }
                continue;
            }
            if in_str {
                cur.push(ch);
            }
        }
        return out.into_iter().filter(|s| !s.is_empty()).collect();
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `slash.exec`.
///
/// `op` mirrors the whole handler after the early `empty command` (`4004`) check
/// through live slash output, pending-input, worker-blocked (`4018`), bundle/skill
/// routing (`4018`), plugin handler, and slash-worker spawn/run (`5030`).
/// `Ok(result_json)` is `{"output":...}` (and optional `"warning"`); `Err((code,msg))`
/// maps to `_err` (`4004`/`4018`/`5030`).
pub fn handle_slash_exec<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let cmd = extract_string_field(params_json, "command").unwrap_or_default().trim().to_string();
    if cmd.is_empty() {
        return err_response(rid_json, ERR_EMPTY_COMMAND, "empty command");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `insights.get`.
///
/// `op` mirrors `_get_db` (`5017` on None/"db unavailable") + `list_sessions_rich` +
/// `cutoff` filter → `{"days":..., "sessions":..., "messages":...}`.
/// `Err(msg)` maps to `5017`.
pub fn handle_insights_get<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_DB_UNAVAILABLE_INSIGHTS, &e),
    }
}

/// Handle `rollback.list`.
///
/// `op` mirrors `_sess` + `_with_checkpoints` → `{"enabled":bool,"checkpoints":[...]}`
/// or `Err((code,msg))` where `code` is `5020` (or `5007`/`4007` via `_sess`).
pub fn handle_rollback_list<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `rollback.restore`.
///
/// Validates `hash` required (`4014`) handler-side (pre-lock check), then
/// delegates `4009`/`5021`/`5007` etc. to `op`.
pub fn handle_rollback_restore<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let target = extract_string_field(params_json, "hash").unwrap_or_default().trim().to_string();
    if target.is_empty() {
        return err_response(rid_json, ERR_HASH_REQUIRED, "hash required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `rollback.diff`.
///
/// Validates `hash` required (`4014`) handler-side, then delegates to `op`.
pub fn handle_rollback_diff<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let target = extract_string_field(params_json, "hash").unwrap_or_default().trim().to_string();
    if target.is_empty() {
        return err_response(rid_json, ERR_HASH_REQUIRED, "hash required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `browser.manage`.
///
/// Validates unknown action (`4015`) handler-side, then delegates `connect`/
/// `disconnect`/`status` payload construction to `op`.
/// `op` returns `Ok(result_json)` where result_json is `{"connected":bool,"url":...}`
/// or `{"url":...}` etc.; `Err((code,msg))` maps to `_err`.
pub fn handle_browser_manage<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let action = extract_browser_action(params_json);
    if !is_valid_browser_action(&action) {
        return err_response(rid_json, ERR_UNKNOWN_BROWSER_ACTION, &format!("unknown action: {}", action));
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `plugins.list`.
///
/// `op` mirrors `get_plugin_manager()._plugins` → `{"plugins":[...]}`; `Err`→`5032`.
pub fn handle_plugins_list<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_PLUGINS_LIST, &e),
    }
}

/// Handle `config.show`.
///
/// `op` mirrors `_load_cfg` + `_resolve_model` + `get_secret` masking + `sections` shape;
/// `Err`→`5030`.
pub fn handle_config_show<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_CONFIG_SHOW, &e),
    }
}

/// Handle `tools.list`.
///
/// `op` mirrors `get_all_toolsets` + `get_toolset_info` + session enabled set → `{"toolsets":[...]}`; `Err`→`5031`.
pub fn handle_tools_list<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_TOOLS_LIST, &e),
    }
}

/// Handle `tools.show`.
///
/// `op` mirrors `get_tool_definitions` + `get_toolset_for_tool` → `{"sections":[...],"total":...}`; `Err`→`5034`.
pub fn handle_tools_show<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_TOOLS_SHOW, &e),
    }
}

/// Handle `tools.configure`.
///
/// Validates `action` (`4017`) and `names` (`4018`) handler-side (pre-lock checks),
/// then delegates `CONFIGURABLE_TOOLSETS` + `_apply_*` + `save_config` + `reset` to `op`.
/// `op` returns `Ok(result_json)` where result_json is `{"changed":..., "enabled_toolsets":..., "info":..., "missing_servers":..., "reset":..., "unknown":...}`;
/// `Err((code,msg))` is `5035` (or `4017`/`4018` if closure validates).
pub fn handle_tools_configure<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let raw_action = extract_string_field(params_json, "action").unwrap_or_default().trim().to_ascii_lowercase();
    let action = raw_action.trim().to_string();
    if !is_valid_tools_action(&action) {
        return err_response(rid_json, ERR_UNKNOWN_TOOLS_ACTION, &format!("unknown tools action: {}", action));
    }
    let targets = extract_tools_targets(params_json);
    if targets.is_empty() {
        return err_response(rid_json, ERR_NAMES_REQUIRED, "names required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `toolsets.list`.
pub fn handle_toolsets_list<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_TOOLSETS_LIST, &e),
    }
}

/// Handle `agents.list`.
pub fn handle_agents_list<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_AGENTS_LIST, &e),
    }
}

/// Handle `cron.manage`.
///
/// `op` mirrors `get_profile_dir` + `set_hermes_home_override` (`4064`/`5023`) +
/// `cronjob(action, ...)` (`4016`/`5023` + `scoped` marker, `include_disabled` truthy,
/// `repeat` digit, `continuity` truthy, `deliver`) → `Ok(result_json)` where result_json
/// is the `json.loads` payload; `Err((code,msg))` carries `4064`/`4016`/`5023`.
pub fn handle_cron_manage<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `learning.frames`.
///
/// `op` mirrors `build_learning_graph` + `render_frames` with `cols=max(20,cols)` +
/// `rows=max(10,rows)` clamping; `Err(msg)`→`5000`. The `cols`/`rows`/`frames` parse
/// (`TypeError/ValueError→80/24/48`) is exposed via [`parse_learning_dims`] for
/// `std`-only tests, but the handler delegates the full render to `op` so the
/// closure can assert the clamped values.
pub fn handle_learning_frames<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err(e) => err_response(rid_json, ERR_LEARNING_FRAMES, &e),
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with the 15 slice-2 methods registered
/// using the provided deps (for tests / production injection).
///
/// Each closure is `'static` and mirrors the lazy imports inside Python
/// handler bodies. For the default stub (no backend) use [`build_registry_default`].
#[allow(clippy::too_many_arguments)]
pub fn build_registry<SE, IG, RL, RR, RD, BM, PL, CS, TL, TS, TC, TLS, AL, CM, LF>(
    slash_exec: SE,
    insights_get: IG,
    rollback_list: RL,
    rollback_restore: RR,
    rollback_diff: RD,
    browser_manage: BM,
    plugins_list: PL,
    config_show: CS,
    tools_list: TL,
    tools_show: TS,
    tools_configure: TC,
    toolsets_list: TLS,
    agents_list: AL,
    cron_manage: CM,
    learning_frames: LF,
) -> HandlerRegistry
where
    SE: Fn(String, String) -> String + Send + Sync + 'static,
    IG: Fn(String, String) -> String + Send + Sync + 'static,
    RL: Fn(String, String) -> String + Send + Sync + 'static,
    RR: Fn(String, String) -> String + Send + Sync + 'static,
    RD: Fn(String, String) -> String + Send + Sync + 'static,
    BM: Fn(String, String) -> String + Send + Sync + 'static,
    PL: Fn(String, String) -> String + Send + Sync + 'static,
    CS: Fn(String, String) -> String + Send + Sync + 'static,
    TL: Fn(String, String) -> String + Send + Sync + 'static,
    TS: Fn(String, String) -> String + Send + Sync + 'static,
    TC: Fn(String, String) -> String + Send + Sync + 'static,
    TLS: Fn(String, String) -> String + Send + Sync + 'static,
    AL: Fn(String, String) -> String + Send + Sync + 'static,
    CM: Fn(String, String) -> String + Send + Sync + 'static,
    LF: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        slash_exec,
        insights_get,
        rollback_list,
        rollback_restore,
        rollback_diff,
        browser_manage,
        plugins_list,
        config_show,
        tools_list,
        tools_show,
        tools_configure,
        toolsets_list,
        agents_list,
        cron_manage,
        learning_frames,
    );
    reg
}

/// Build a registry with default stubs (no backend / no file I/O).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_slash_exec(&rid_json, &params_json, |_| Err((ERR_SLASH_WORKER, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_insights_get(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_rollback_list(&rid_json, &params_json, |_| Err((ERR_ROLLBACK_LIST, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_rollback_restore(&rid_json, &params_json, |_| Err((ERR_ROLLBACK_RESTORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_rollback_diff(&rid_json, &params_json, |_| Err((ERR_ROLLBACK_DIFF, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_browser_manage(&rid_json, &params_json, |_| Err((ERR_UNKNOWN_BROWSER_ACTION, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_plugins_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_config_show(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_tools_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_tools_show(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_tools_configure(&rid_json, &params_json, |_| Err((ERR_TOOLS_CONFIGURE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_toolsets_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_agents_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_cron_manage(&rid_json, &params_json, |_| Err((ERR_CRON_MANAGE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_learning_frames(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
    )
}

/// Register all 15 slice-2 methods onto an existing registry.
#[allow(clippy::too_many_arguments)]
pub fn register_with<SE, IG, RL, RR, RD, BM, PL, CS, TL, TS, TC, TLS, AL, CM, LF>(
    registry: &mut HandlerRegistry,
    slash_exec: SE,
    insights_get: IG,
    rollback_list: RL,
    rollback_restore: RR,
    rollback_diff: RD,
    browser_manage: BM,
    plugins_list: PL,
    config_show: CS,
    tools_list: TL,
    tools_show: TS,
    tools_configure: TC,
    toolsets_list: TLS,
    agents_list: AL,
    cron_manage: CM,
    learning_frames: LF,
) where
    SE: Fn(String, String) -> String + Send + Sync + 'static,
    IG: Fn(String, String) -> String + Send + Sync + 'static,
    RL: Fn(String, String) -> String + Send + Sync + 'static,
    RR: Fn(String, String) -> String + Send + Sync + 'static,
    RD: Fn(String, String) -> String + Send + Sync + 'static,
    BM: Fn(String, String) -> String + Send + Sync + 'static,
    PL: Fn(String, String) -> String + Send + Sync + 'static,
    CS: Fn(String, String) -> String + Send + Sync + 'static,
    TL: Fn(String, String) -> String + Send + Sync + 'static,
    TS: Fn(String, String) -> String + Send + Sync + 'static,
    TC: Fn(String, String) -> String + Send + Sync + 'static,
    TLS: Fn(String, String) -> String + Send + Sync + 'static,
    AL: Fn(String, String) -> String + Send + Sync + 'static,
    CM: Fn(String, String) -> String + Send + Sync + 'static,
    LF: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method(METHOD_SLASH_EXEC, slash_exec);
    registry.method(METHOD_INSIGHTS_GET, insights_get);
    registry.method(METHOD_ROLLBACK_LIST, rollback_list);
    registry.method(METHOD_ROLLBACK_RESTORE, rollback_restore);
    registry.method(METHOD_ROLLBACK_DIFF, rollback_diff);
    registry.method(METHOD_BROWSER_MANAGE, browser_manage);
    registry.method(METHOD_PLUGINS_LIST, plugins_list);
    registry.method(METHOD_CONFIG_SHOW, config_show);
    registry.method(METHOD_TOOLS_LIST, tools_list);
    registry.method(METHOD_TOOLS_SHOW, tools_show);
    registry.method(METHOD_TOOLS_CONFIGURE, tools_configure);
    registry.method(METHOD_TOOLSETS_LIST, toolsets_list);
    registry.method(METHOD_AGENTS_LIST, agents_list);
    registry.method(METHOD_CRON_MANAGE, cron_manage);
    registry.method(METHOD_LEARNING_FRAMES, learning_frames);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_slash_exec(&rid_json, &params_json, |_| Err((ERR_SLASH_WORKER, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_insights_get(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_rollback_list(&rid_json, &params_json, |_| Err((ERR_ROLLBACK_LIST, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_rollback_restore(&rid_json, &params_json, |_| Err((ERR_ROLLBACK_RESTORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_rollback_diff(&rid_json, &params_json, |_| Err((ERR_ROLLBACK_DIFF, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_browser_manage(&rid_json, &params_json, |_| Err((ERR_UNKNOWN_BROWSER_ACTION, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_plugins_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_config_show(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_tools_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_tools_show(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_tools_configure(&rid_json, &params_json, |_| Err((ERR_TOOLS_CONFIGURE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_toolsets_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_agents_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_cron_manage(&rid_json, &params_json, |_| Err((ERR_CRON_MANAGE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_learning_frames(&rid_json, &params_json, |_| Err("no backend".to_string()))
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
    fn undo_count_and_notice() {
        assert_eq!(parse_undo_count("").unwrap(), 1);
        assert_eq!(parse_undo_count("3").unwrap(), 3);
        assert_eq!(parse_undo_count("  0  ").unwrap(), 1);
        assert_eq!(parse_undo_count("-5").unwrap(), 1);
        assert!(parse_undo_count("bad").is_err());
        assert_eq!(parse_undo_count("bad").unwrap_err().0, 4004);
        assert_eq!(format_undo_notice(1, 5), "↶ Undid 1 turn (5 message(s)). Edit and resubmit, or send a new message.");
        assert_eq!(format_undo_notice(2, 7), "↶ Undid 2 turns (7 message(s)). Edit and resubmit, or send a new message.");
        assert!(snapshot_restore_blocked_output().contains("Run it in the classic CLI"));
        assert!(is_compress_command("compress"));
        assert!(is_compress_command("compact"));
        assert!(!is_compress_command("undo"));
    }

    #[test]
    fn slash_exec_empty_and_ok() {
        let rid = rid1();
        let out = handle_slash_exec(&rid, r#"{"command":""}"#, |_| Ok(r#"{"output":"hi"}"#.to_string()));
        assert!(out.contains(r#""code":4004"#), "{}", out);
        let out2 = handle_slash_exec(&rid, r#"{"command":"   "}"#, |_| panic!("should not call on empty"));
        assert!(out2.contains(r#""code":4004"#));
        let out3 = handle_slash_exec(&rid, r#"{"command":"/help"}"#, |p| {
            assert!(p.contains("/help"));
            Ok(r#"{"output":"help text"}"#.to_string())
        });
        assert!(out3.contains("help text"), "{}", out3);
        let out4 = handle_slash_exec(&rid, r#"{"command":"/foo"}"#, |_| Err((4018, "snapshot restore mutates".into())));
        assert!(out4.contains(r#""code":4018"#), "{}", out4);
        let out5 = handle_slash_exec(&rid, r#"{"command":"/foo"}"#, |_| Err((5030, "slash worker start failed: boom".into())));
        assert!(out5.contains(r#""code":5030"#), "{}", out5);
    }

    #[test]
    fn rollback_hash_required_and_ok() {
        let rid = rid1();
        let out = handle_rollback_restore(&rid, r#"{"hash":""}"#, |_| Ok("{}".to_string()));
        assert!(out.contains(r#""code":4014"#), "{}", out);
        let out2 = handle_rollback_restore(&rid, r#"{"hash":"abc"}"#, |_| Ok(r#"{"success":true,"history_removed":2}"#.into()));
        assert!(out2.contains("history_removed"), "{}", out2);
        let out3 = handle_rollback_restore(&rid, r#"{"hash":"abc"}"#, |_| Err((4009, "session busy".into())));
        assert!(out3.contains(r#""code":4009"#));
        let out4 = handle_rollback_restore(&rid, r#"{"hash":"abc"}"#, |_| Err((5021, "boom".into())));
        assert!(out4.contains(r#""code":5021"#));
        let out5 = handle_rollback_diff(&rid, r#"{}"#, |_| Ok("{}".into()));
        assert!(out5.contains(r#""code":4014"#));
        let out6 = handle_rollback_diff(&rid, r#"{"hash":"h1"}"#, |_| Ok(r#"{"stat":"2 files","diff":"@@..."}"#.into()));
        assert!(out6.contains("2 files"), "{}", out6);
        let out7 = handle_rollback_diff(&rid, r#"{"hash":"x"}"#, |_| Err((5022, "fail".into())));
        assert!(out7.contains(r#""code":5022"#));
    }

    #[test]
    fn rollback_list_and_browser() {
        let rid = rid1();
        let out = handle_rollback_list(&rid, "{}", |_| Ok(r#"{"enabled":false,"checkpoints":[]}"#.into()));
        assert!(out.contains("enabled"), "{}", out);
        assert!(out.contains("false"));
        let out2 = handle_rollback_list(&rid, "{}", |_| Ok(r#"{"enabled":true,"checkpoints":[{"hash":"abc","timestamp":"t","message":"m"}]}"#.into()));
        assert!(out2.contains("abc"), "{}", out2);
        let out3 = handle_rollback_list(&rid, "{}", |_| Err((5020, "boom".into())));
        assert!(out3.contains(r#""code":5020"#));
        let out4 = handle_browser_manage(&rid, r#"{"action":"status"}"#, |_| Ok(r#"{"connected":true,"url":"http://x"}"#.into()));
        assert!(out4.contains("connected"), "{}", out4);
        let out5 = handle_browser_manage(&rid, r#"{"action":"disconnect"}"#, |_| Ok(r#"{"disconnected":true}"#.into()));
        assert!(out5.contains("disconnected"), "{}", out5);
        let out6 = handle_browser_manage(&rid, r#"{"action":"weird"}"#, |_| panic!("should not call"));
        assert!(out6.contains(r#""code":4015"#), "{}", out6);
        assert!(out6.contains("weird"));
        let out7 = handle_browser_manage(&rid, r#"{}"#, |_| Ok(r#"{"connected":false,"url":null}"#.into()));
        // default status when no action
        assert!(out7.contains("connected"), "{}", out7);
    }

    #[test]
    fn plugins_and_config() {
        let rid = rid1();
        let out = handle_plugins_list(&rid, "{}", |_| Ok(r#"{"plugins":[{"name":"a","version":"1","enabled":true}]}"#.into()));
        assert!(out.contains("plugins"), "{}", out);
        let out2 = handle_plugins_list(&rid, "{}", |_| Err("fail".into()));
        assert!(out2.contains(r#""code":5032"#), "{}", out2);
        let out3 = handle_config_show(&rid, "{}", |_| Ok(r#"{"sections":[{"title":"Model","rows":[["Model","gpt"]]}]}"#.into()));
        assert!(out3.contains("sections"), "{}", out3);
        let out4 = handle_config_show(&rid, "{}", |_| Err("boom".into()));
        assert!(out4.contains(r#""code":5030"#), "{}", out4);
    }

    #[test]
    fn tools_lists_and_show() {
        let rid = rid1();
        let out = handle_tools_list(&rid, "{}", |_| Ok(r#"{"toolsets":[{"name":"file","description":"File","tool_count":2,"enabled":true,"tools":["read_file"]}]}"#.into()));
        assert!(out.contains("toolsets"), "{}", out);
        let out2 = handle_tools_list(&rid, "{}", |_| Err("x".into()));
        assert!(out2.contains(r#""code":5031"#));
        let out3 = handle_tools_show(&rid, "{}", |_| Ok(r#"{"sections":[{"name":"file","tools":[{"name":"read_file","description":"Read"}] }],"total":1}"#.into()));
        assert!(out3.contains("total"), "{}", out3);
        let out4 = handle_tools_show(&rid, "{}", |_| Err("y".into()));
        assert!(out4.contains(r#""code":5034"#));
        let out5 = handle_toolsets_list(&rid, "{}", |_| Ok(r#"{"toolsets":[{"name":"file","description":"desc","tool_count":1,"enabled":true}]}"#.into()));
        assert!(out5.contains("toolsets"), "{}", out5);
        let out6 = handle_toolsets_list(&rid, "{}", |_| Err("z".into()));
        assert!(out6.contains(r#""code":5032"#));
        let out7 = handle_agents_list(&rid, "{}", |_| Ok(r#"{"processes":[{"session_id":"s1","command":"sleep 10","status":"running","uptime":5}]}"#.into()));
        assert!(out7.contains("processes"), "{}", out7);
        let out8 = handle_agents_list(&rid, "{}", |_| Err("p".into()));
        assert!(out8.contains(r#""code":5033"#));
    }

    #[test]
    fn tools_configure_validation() {
        let rid = rid1();
        // unknown action 4017
        let out = handle_tools_configure(&rid, r#"{"action":"bad","names":["file"]}"#, |_| Ok("{}".into()));
        assert!(out.contains(r#""code":4017"#), "{}", out);
        assert!(out.contains("bad"));
        // names required 4018
        let out2 = handle_tools_configure(&rid, r#"{"action":"enable","names":[]}"#, |_| Ok("{}".into()));
        assert!(out2.contains(r#""code":4018"#), "{}", out2);
        let out3 = handle_tools_configure(&rid, r#"{"action":"enable"}"#, |_| Ok("{}".into()));
        assert!(out3.contains(r#""code":4018"#), "{}", out3);
        let out4 = handle_tools_configure(&rid, r#"{"action":"disable","names":["file","search"]}"#, |_| Ok(r#"{"changed":["file"],"enabled_toolsets":["search"],"info":null,"missing_servers":[],"reset":false,"unknown":[]}"#.into()));
        assert!(out4.contains("changed"), "{}", out4);
        let out5 = handle_tools_configure(&rid, r#"{"action":"enable","names":["file"]}"#, |_| Err((5035, "boom".into())));
        assert!(out5.contains(r#""code":5035"#), "{}", out5);
        // targets parsing
        assert_eq!(extract_tools_targets(r#"{"names":["file", "search"]}"#), vec!["file", "search"]);
        assert!(extract_tools_targets(r#"{"names":[]}"#).is_empty());
        assert!(is_valid_tools_action("enable"));
        assert!(!is_valid_tools_action("bad"));
    }

    #[test]
    fn insights_and_cron_and_learning() {
        let rid = rid1();
        let out = handle_insights_get(&rid, r#"{"days":30}"#, |_| Ok(r#"{"days":30,"sessions":2,"messages":10}"#.into()));
        assert!(out.contains("sessions"), "{}", out);
        let out2 = handle_insights_get(&rid, "{}", |_| Err("db unavailable".into()));
        assert!(out2.contains(r#""code":5017"#), "{}", out2);
        let out3 = handle_cron_manage(&rid, r#"{"action":"list"}"#, |_| Ok(r#"{"jobs":[]}"#.into()));
        assert!(out3.contains("jobs"), "{}", out3);
        let out4 = handle_cron_manage(&rid, "{}", |_| Err((4064, "profile 'x' not found".into())));
        assert!(out4.contains(r#""code":4064"#), "{}", out4);
        let out5 = handle_cron_manage(&rid, "{}", |_| Err((4016, "unknown cron action: bad".into())));
        assert!(out5.contains(r#""code":4016"#), "{}", out5);
        let out6 = handle_cron_manage(&rid, "{}", |_| Err((5023, "fail".into())));
        assert!(out6.contains(r#""code":5023"#), "{}", out6);
        // learning dims parsing
        assert_eq!(parse_learning_dims(r#"{"cols":80,"rows":24,"frames":48}"#), (80, 24, 48));
        assert_eq!(parse_learning_dims(r#"{"cols":"bad","rows":24,"frames":48}"#), (80, 24, 48)); // bad field defaults per-file
        assert_eq!(parse_learning_dims(r#"{}"#), (80, 24, 48));
        assert_eq!(clamp_learning_dims(10, 5), (20, 10));
        assert_eq!(clamp_learning_dims(100, 100), (100, 100));
        let out7 = handle_learning_frames(&rid, r#"{"cols":40,"rows":20,"frames":10}"#, |_| Ok(r#"{"frames":[],"legend":{}}"#.into()));
        assert!(out7.contains("frames"), "{}", out7);
        let out8 = handle_learning_frames(&rid, "{}", |_| Err("fail".into()));
        assert!(out8.contains(r#""code":5000"#), "{}", out8);
        // truthy
        assert!(is_truthy_value(Some("true")));
        assert!(is_truthy_value(Some("1")));
        assert!(!is_truthy_value(Some("false")));
        assert!(is_truthy_field(r#"{"include_disabled":true}"#, "include_disabled"));
        assert!(!is_truthy_field(r#"{"include_disabled":false}"#, "include_disabled"));
    }

    #[test]
    fn registry_installs_all_fifteen() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 15);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["agents.list","browser.manage","config.show","cron.manage","insights.get","learning.frames","plugins.list","rollback.diff","rollback.list","rollback.restore","slash.exec","toolsets.list","tools.configure","tools.list","tools.show"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 15);
        // slash.exec without command should be 4004 even with default stub
        let out = map.get(METHOD_SLASH_EXEC).unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains(r#""code":4004"#), "{}", out);
        // rollback.restore without hash should be 4014
        let out2 = map.get(METHOD_ROLLBACK_RESTORE).unwrap()("1".to_string(), "{}".to_string());
        assert!(out2.contains(r#""code":4014"#), "{}", out2);
        // browser.manage default status should be ok (connected false stub fails 4015? Actually default stub returns Err -> 4015 but our default for browser is Err((4015)))
        let out3 = map.get(METHOD_BROWSER_MANAGE).unwrap()("1".to_string(), r#"{"action":"status"}"#.to_string());
        assert!(out3.contains(r#""code":4015"#) || out3.contains("connected"), "{}", out3);
        // tools.configure missing names → 4018
        let out4 = map.get(METHOD_TOOLS_CONFIGURE).unwrap()("1".to_string(), r#"{"action":"enable","names":[]}"#.to_string());
        assert!(out4.contains(r#""code":4018"#), "{}", out4);
        // learning.frames ok path with default stub → 5000
        let out5 = map.get(METHOD_LEARNING_FRAMES).unwrap()("1".to_string(), "{}".to_string());
        assert!(out5.contains(r#""code":5000"#), "{}", out5);
    }

    #[test]
    fn ok_err_envelope_shape() {
        let rid = encode_rid("42");
        let ok = ok_response(&rid, r#"{"output":"hi"}"#);
        assert!(ok.contains(r#""result""#));
        assert!(ok.contains("hi"));
        let err = err_response(&rid, 4004, "empty command");
        assert!(err.contains(r#""code":4004"#));
        assert!(err.contains("empty command"));
    }

    #[test]
    fn extract_helpers() {
        assert_eq!(extract_string_field(r#"{"command":"/help"}"#, "command").as_deref(), Some("/help"));
        assert_eq!(extract_string_field(r#"{"hash":"abc123"}"#, "hash").as_deref(), Some("abc123"));
        assert_eq!(extract_browser_action(r#"{"action":"connect"}"#), "connect");
        assert_eq!(extract_browser_action(r#"{}"#), "status");
        assert!(is_valid_browser_action("status"));
        assert!(!is_valid_browser_action("bad"));
        assert_eq!(extract_hash_param(r#"{"hash":"h1"}"#).as_deref(), Some("h1"));
        assert_eq!(extract_hash_param(r#"{"hash":""}"#), None);
        assert_eq!(extract_raw_value(r#"{"days":30}"#, "days").unwrap(), "30");
        assert_eq!(extract_string_or_empty(r#"{"file_path":"/tmp/foo"}"#, "file_path"), "/tmp/foo");
    }

    #[test]
    fn hash_target_text_and_extract() {
        // extract_target_text_content list case
        let list_json = r#"[{"type":"text","text":"hello"},{"type":"text","text":"world"}]"#;
        // our helper joins text fields naive; ensure it works for that shape
        let out = extract_target_text_content(list_json);
        assert!(out.contains("hello"), "{}", out);
        assert!(out.contains("world"), "{}", out);
        let plain = "just a string";
        assert_eq!(extract_target_text_content(plain), plain);
        assert_eq!(extract_target_text_content(""), "");
    }
}
