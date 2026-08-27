//! Session / delegation / spawn-tree / billing / pet JSON-RPC handlers — slice 2 (lines 900-1800).
//!
//! 1:1 port of `tui_gateway/methods_session.py` lines 900–1800 (T0383 slice 2/3633).
//!
//! Handler bodies are byte-identical to their pre-split `server.py` form; they
//! are rebound onto `server.py`'s globals at install time — see `method_ctx.py`.
//!
//! ```python
//! # Python — tui_gateway/methods_session.py 900-1800 (abridged, comments preserved)
//! """Session / delegation / spawn-tree / billing / pet JSON-RPC handlers (moved verbatim from server.py).
//!
//! Handler bodies are byte-identical to their pre-split server.py form; they
//! are rebound onto server.py's globals at install time — see method_ctx.py.
//! """
//! from .method_ctx import HandlerRegistry
//! _registry = HandlerRegistry()
//! method = _registry.method
//! _profile_scoped = _registry.profile_scoped
//!
//! # ── session.resume tail (900-1078) ──────────────────────────────────────
//! # raw_history = db.get_messages_as_conversation(target, repair_alternation=True, include_row_ids=True)
//! # display_history = []  # etc, db.get_resume_conversations, sanitize_replay_history
//! # history = sanitize_replay_history(raw_history)
//! # messages = [] if omit_messages else _history_to_messages(display_history)
//! # display_history_prefix = [] if omit_messages else db.get_ancestor_display_prefix(target)
//! # tokens = _set_session_context(target)
//! # stored_runtime_overrides = _stored_session_runtime_overrides(found)
//! # agent = _make_agent(sid, target, session_id=target, session_db=db, platform_override=source, **overrides)
//! # ... owns_db / home_token / secret_token / SessionDB / set_hermes_home_override / set_secret_scope
//! # Double-checked locking: with _session_resume_lock: live=_find_live_session_by_key(target)
//! #   if live: agent.close(); lease.release(); return _reuse_live_response(*live)
//! #   with set_hermes_home_override ...
//! #     _init_session(sid, target, agent, history, cols=cols, cwd=profile_resume_cwd, session_db=db, source=source)
//! #     if owns_db: _transfer_db_to_agent(agent, db); owns_db=False
//! #   _sessions[sid]["model_override"]=...; ["display_history_prefix"]=...; ["profile_home"]=...; ["active_session_lease"]=lease
//! # finally: if owns_db and db: db.close()
//! # auto_continue = _maybe_schedule_auto_continue(sid, session, target) if session else None
//! # return _ok(rid, {"session_id":sid,"resumed":target,"message_count":..., "messages":messages, ... "info":_session_info(...)})
//!
//! @method("session.cwd.set")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     if session.get("running"): return _err(rid, 4009, "session busy")
//!     raw = str(params.get("cwd", "") or "").strip()
//!     if not raw: return _err(rid, 4016, "cwd required")
//!     try: cwd = _set_session_cwd(session, raw)
//!     except ValueError as e: return _err(rid, 4017, str(e))
//!     agent = session.get("agent")
//!     info = _session_info(agent, session) if agent is not None else {"cwd":cwd,"branch":_git_branch_for_cwd(cwd),"project":_project_info_for_cwd(cwd),"lazy":True}
//!     _emit("session.info", params.get("session_id",""), info)
//!     return _ok(rid, info)
//!
//! @method("session.workspace.move")
//! def _(rid, params: dict) -> dict:
//!     target = str(params.get("session_key") or "").strip()
//!     if not target: return _err(rid, 4007, "session_key required")
//!     raw = str(params.get("cwd", "") or "").strip()
//!     if not raw: return _err(rid, 4016, "cwd required")
//!     from hermes_constants import translate_cwd_for_wsl_backend
//!     resolved = os.path.abspath(os.path.expanduser(translate_cwd_for_wsl_backend(raw)))
//!     if not os.path.isdir(resolved): return _err(rid, 4017, f"working directory does not exist: {raw}")
//!     live, live_sid = snapshot _sessions under _sessions_lock by session_key==target
//!     branch = _git_branch_for_cwd(resolved); root = _git_common_repo_root_for_cwd(resolved)
//!     with _profile_db(params) as db:
//!         if db is None: return _db_unavailable_error(rid, code=5007)
//!         row_exists = bool(db.get_session(target))
//!         if not row_exists and live is None: return _err(rid, 4007, "session not found")
//!         if row_exists: db.update_session_cwd(target, resolved, branch, root, replace_git_meta=True)
//!     if live is not None: _set_session_cwd(live, resolved); _emit("session.info", live_sid, info)
//!     return _ok(rid, {"cwd":resolved,"branch":branch,"git_repo_root":root})
//!
//! @method("session.active_list")
//! def _(rid, params: dict) -> dict:
//!     current = str(params.get("current_session_id") or "")
//!     try: with _sessions_lock: snapshot = list(_sessions.items())
//!     except Exception as e: return _err(rid, 5036, f"could not enumerate active sessions: {e}")
//!     rows = [_session_live_item(sid, session, current) for sid, session in snapshot if not session.get("_finalized")]
//!     return _ok(rid, {"sessions":rows})
//!
//! @method("session.activate")
//! def _(rid, params: dict) -> dict:
//!     sid = str(params.get("session_id") or "")
//!     session, err = _sess_nowait({"session_id":sid}, rid)
//!     if err: return err
//!     return _ok(rid, _live_session_payload(sid, session, touch=True, transport=current_transport() or _stdio_transport, omit_messages=is_truthy_value(params.get("omit_messages", False))))
//!
//! @method("session.delete")
//! def _(rid, params: dict) -> dict:
//!     target = params.get("session_id","")
//!     if not target: return _err(rid, 4006, "session_id required")
//!     try: with _sessions_lock: snapshot = list(_sessions.values())
//!     except Exception as e: return _err(rid, 5036, f"could not enumerate active sessions: {e}")
//!     active = {s.get("session_key") for s in snapshot if s.get("session_key")}
//!     if target in active: return _err(rid, 4023, "cannot delete an active session")
//!     profile = (params.get("profile") or "").strip() or None; profile_home = _profile_home(profile)
//!     with _profile_db(params) as db:
//!         if db is None: return _db_unavailable_error(rid, code=5036)
//!         sessions_dir = Path(profile_home)/"sessions" if profile_home else get_hermes_home()/"sessions"
//!         try: deleted = db.delete_session(target, sessions_dir=sessions_dir)
//!         except Exception as e: return _err(rid, 5036, f"delete failed: {e}")
//!         if not deleted: return _err(rid, 4007, "session not found")
//!         return _ok(rid, {"deleted":target})
//!
//! @method("session.title")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     with _session_db(session) as db:
//!         if db is None: return _db_unavailable_error(rid, code=5007)
//!         key = session["session_key"]
//!         if "title" not in params:
//!             fallback = session.get("pending_title") or ""
//!             try:
//!                 resolved_title = db.get_session_title(key) or ""
//!                 if fallback and db.set_session_title(key, fallback): session["pending_title"]=None; resolved_title=fallback
//!                 # ... existing_row fallback branches
//!             except Exception: resolved_title=fallback
//!             _emit_session_info_for_session(params.get("session_id",""), session)
//!             return _ok(rid, {"title":resolved_title,"session_key":key})
//!         title = (params.get("title","") or "").strip()
//!         if not title: return _err(rid, 4021, "title required")
//!         try:
//!             if db.set_session_title(key, title): session["pending_title"]=None; _emit...; return _ok(..., {"pending":False,"title":title})
//!             existing_row = db.get_session(key)
//!             if existing_row: ... return _ok(... pending False ...)
//!             _ensure_session_db_row(session); with _session_db(session) as scoped_db: if scoped_db.set_session_title(key,title): ... return pending False
//!             session["pending_title"]=title; return _ok(..., {"pending":True,"title":title})
//!         except ValueError as e: return _err(rid, 4022, str(e))
//!         except Exception as e: return _err(rid, 5007, str(e))
//!
//! @method("session.set_hidden")
//! def _(rid, params: dict) -> dict:
//!     hidden = is_truthy_value(params.get("hidden", True))
//!     session, err = _sess_nowait(params, rid)
//!     if session is not None:
//!         with _session_db(session) as db:
//!             if db is None: return _db_unavailable_error(rid, code=5007)
//!             key = session["session_key"]
//!             try: changed = db.set_session_hidden(key, hidden); if not changed: session["pending_hidden"]=hidden; return _ok(rid, {"hidden":hidden,"session_key":key})
//!             except Exception as e: return _err(rid, 5007, str(e))
//!     target = str(params.get("session_id") or "").strip()
//!     with _profile_db(params) as db:
//!         if db is None: return _db_unavailable_error(rid, code=5007)
//!         try: resolved = db.resolve_session_id(target) if hasattr(db,"resolve_session_id") else target; if not resolved: return err; db.set_session_hidden(resolved, hidden); return _ok(rid, {"hidden":hidden,"session_key":resolved})
//!         except Exception as e: return _err(rid, 5007, str(e))
//!
//! @method("message.react")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     newest_role = str(params.get("newest_role") or "").strip()
//!     row_id = params.get("row_id")
//!     if row_id is None and newest_role not in {"user","assistant"}: return _err(rid, 4023, "row_id or newest_role required")
//!     emoji = params.get("emoji"); if emoji is not None: emoji=str(emoji).strip(); if not emoji: return _err(rid, 4024, "emoji must be a non-empty string or null")
//!     author = str(params.get("author") or "user").strip()
//!     if author not in {"user","agent"}: return _err(rid, 4025, "author must be 'user' or 'agent'")
//!     with _session_db(session) as db:
//!         if db is None: return _db_unavailable_error(rid, code=5007)
//!         try:
//!             if row_id is None: row_id = db.latest_message_row_id(session["session_key"], role=newest_role); if row_id is None: return _err(rid, 4040, "no message to react to yet")
//!             reactions = db.set_message_reaction(session["session_key"], int(row_id), emoji, author=author)
//!         except Exception as e: return _err(rid, 5007, str(e))
//!     if reactions is None: return _err(rid, 4040, "message not found in this session")
//!     return _ok(rid, {"row_id":int(row_id),"reactions":reactions})
//!
//! @method("llm.oneshot")
//! def _(rid, params: dict) -> dict:
//!     template = (params.get("template") or "").strip() or None; instructions = params.get("instructions") or ""; user_input = params.get("input") or ""; variables = params.get("variables") if isinstance(...dict) else {}
//!     task = (params.get("task") or "title_generation").strip() or "title_generation"
//!     try: max_tokens = int(params.get("max_tokens") or 1024) except: max_tokens=1024
//!     temperature = float(...) if not None else None
//!     if not template and not str(instructions).strip() and not str(user_input).strip(): return _err(rid, 4030, "llm.oneshot requires a template or instructions/input")
//!     session = _sessions.get(params.get("session_id") or ""); main_runtime = _main_runtime_from_agent(session.get("agent")) if session else None
//!     try: from agent.oneshot import run_oneshot; text = run_oneshot(instructions, user_input, template, variables, task, max_tokens, temperature or 0.3, main_runtime)
//!     except KeyError as e: return _err(rid, 4031, str(e))
//!     except ValueError as e: return _err(rid, 4032, str(e))
//!     except Exception as e: logger.warning(...); return _err(rid, 5030, f"one-shot generation failed: {e}")
//!     return _ok(rid, {"text":text})
//!
//! @method("handoff.request")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     if session.get("running"): return _err(rid, 4009, "session busy — wait ...")
//!     platform_name = (params.get("platform","") or "").strip().lower()
//!     if not platform_name: return _err(rid, 4023, "platform required")
//!     from gateway.config import Platform, load_gateway_config
//!     try: platform = Platform(platform_name)
//!     except (ValueError, KeyError): return _err(rid, 4024, f"unknown platform '{platform_name}'")
//!     try: gw_config = load_gateway_config()
//!     except Exception as e: return _err(rid, 5021, f"could not load gateway config: {e}")
//!     pcfg = gw_config.platforms.get(platform)
//!     if not pcfg or not pcfg.enabled: return _err(rid, 4025, f"platform '{platform_name}' is not configured/enabled ...")
//!     home = gw_config.get_home_channel(platform)
//!     if not home or not home.chat_id: return _err(rid, 4026, f"no home channel configured for {platform_name} ...")
//!     _ensure_session_db_row(session)
//!     with _session_db(session) as db:
//!         if db is None: return _db_unavailable_error(rid, code=5007)
//!         key = session["session_key"]
//!         try: if not db.get_session(key): db.set_session_title(key, f"handoff-{key[:8]}"); ok = db.request_handoff(key, platform_name)
//!         except Exception as e: return _err(rid, 5007, str(e))
//!     if not ok: return _err(rid, 4027, "session is already in flight ...")
//!     return _ok(rid, {"queued":True,"session_key":key,"platform":platform_name,"home_name":home.name})
//!
//! @method("handoff.state")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     with _session_db(session) as db:
//!         if db is None: return _db_unavailable_error(rid, code=5007)
//!         record = db.get_handoff_state(session["session_key"])
//!     record = record or {}
//!     return _ok(rid, {"state":record.get("state") or "","platform":record.get("platform") or "","error":record.get("error") or ""})
//!
//! @method("handoff.fail")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     reason = str(params.get("error") or "handoff failed").strip()[:500]
//!     with _session_db(session) as db:
//!         if db is None: return _db_unavailable_error(rid, code=5007)
//!         key = session["session_key"]; record = db.get_handoff_state(key) or {}; state = record.get("state") or ""
//!         if state in {"pending","running"}: db.fail_handoff(key, reason); return _ok(rid, {"failed":True,"state":"failed"})
//!     return _ok(rid, {"failed":False,"state":state})
//!
//! @method("session.usage")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     agent = session.get("agent"); usage: dict = _session_usage_snapshot(session)
//!     if agent is None and not usage: usage={"calls":0,"input":0,"output":0,"total":0}
//!     try: from agent.account_usage import nous_credits_lines; credits=nous_credits_lines(); if credits: usage["credits_lines"]=credits
//!     except Exception: pass
//!     return _ok(rid, usage)
//!
//! @method("session.context_breakdown")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     agent = session.get("agent")
//!     if agent is None: usage=_session_usage_snapshot(session) or _get_usage(None); return _ok(rid, {"categories":[],"context_max":usage.get("context_max",0) or 0, ... "model":_metadata_mirror(session).get("model","")})
//!     with session["history_lock"]: history = list(session.get("history",[]))
//!     try: from agent.context_breakdown import compute_session_context_breakdown; payload=compute_session_context_breakdown(agent, history)
//!     except Exception as exc: return _err(rid, 5000, f"Could not compute context breakdown: {exc}")
//!     return _ok(rid, payload)
//!
//! @method("pet.info")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     try: enabled, pet, scale = _pet_active_selection()
//!          if not enabled or pet is None or not pet.exists: return _ok(rid, {"enabled":False})
//!          payload = {"enabled":True, **_pet_sprite_payload(pet, scale=scale)}
//!          known_revision = str(params.get("knownRevision","") or "")
//!          if known_revision and known_revision==payload.get("spritesheetRevision"): payload.pop("spritesheetBase64",None); payload["spritesheetUnchanged"]=True
//!          return _ok(rid, payload)
//!     except Exception as exc: logger.debug("pet.info failed: %s", exc); return _ok(rid, {"enabled":False})
//!
//! @method("pet.info.meta")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     try: enabled, pet, scale = _pet_active_selection()
//!          if not enabled or pet is None or not pet.exists: return _ok(rid, {"enabled":False})
//!          return _ok(rid, {"enabled":True,"slug":pet.slug,"displayName":pet.display_name,"scale":scale,"spritesheetRevision":_pet_sheet_revision(pet.spritesheet)})
//!     except Exception as exc: logger.debug("pet.info.meta failed: %s", exc); return _ok(rid, {"enabled":False})
//!
//! @method("pet.cells")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     try: from agent.pet import constants, render, store; from agent.pet.render import PetRenderer
//!          cfg = load_config() -> display.pet -> pet_cfg
//!          if not is_truthy_value(pet_cfg.get("enabled"), default=False): return _ok(rid, {"enabled":False})
//!          pet = store.resolve_active_pet(str(pet_cfg.get("slug","") or ""))
//!          if pet is None or not pet.exists: return _ok(rid, {"enabled":False})
//!          state = str(params.get("state") or constants.PetState.IDLE.value)
//!          scale = float(pet_cfg.get("scale", constants.DEFAULT_SCALE) or constants.DEFAULT_SCALE)
//!          cols = int(params.get("cols") or 0) or constants.resolve_cols(scale, pet_cfg.get("unicode_cols",0))
//!          if params.get("graphics"):
//!              configured = str(pet_cfg.get("render_mode","auto") or "auto").lower()
//!              gmode = render.detect_terminal_graphics() if configured in ("", "auto") else configured
//!              if gmode=="kitty":
//!                  image_id = render.kitty_image_id(pet.slug); payload = PetRenderer(str(pet.spritesheet), mode="kitty", scale=scale).kitty_payload(state, image_id=image_id)
//!                  if payload: kcount=len(payload["frames"]) or 1; return _ok(rid, {"enabled":True,"slug":pet.slug, ... "graphics":"kitty","placeholder":...,"frames":...,"frameMs":constants.LOOP_MS / max(1,kcount),"scale":scale})
//!          # truncated at line 1800 inside kitty branch before unicode fallback — continues in next slice
//!
//! def register(server) -> None: _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType`
//!   rebinding no-op notes). Profile-scoped handlers use
//!   [`HandlerRegistry::method_profile_scoped`], plain handlers use
//!   [`HandlerRegistry::method`].
//! * `_profile_db(params)` / `_get_db()` / `SessionDB` → injected
//!   `Fn(&str) -> Result<Option<String>, String>` where `Ok(None)` means
//!   `db is None` → `5007`/`5006`/`5036`/`5000`, `Err(e)` → same with `str(e)`.
//! * `is_truthy_value` → [`is_truthy_value`] (mirrors `hermes_constants`).
//! * `_ok(rid, result)` / `_err(rid, code, msg)` / `_db_unavailable_error` →
//!   [`ok_response`] / [`err_response`] (mirrors `server.py::_ok` / `_err`).
//! * `session.resume` tail (900-1078): `owns_db`, `SessionDB(db_path=profile_home/state.db)`,
//!   `home_token`/`secret_token` (`set_hermes_home_override` + `set_secret_scope`),
//!   `sanitize_replay_history` → `raw_history`, `display_history` + `display_history_prefix`
//!   (`get_ancestor_display_prefix`), `history_version` + `session_info` → injected
//!   `Fn(&str)->Result<String,(i32,String)>` that owns `HERMES_HOME`/`_sessions` mutation;
//!   double-checked locking ` _session_resume_lock` + `_find_live_session_by_key` +
//!   `_reuse_live_response` (`*live`), `_init_session` + `_transfer_db_to_agent(owns_db)` +
//!   `active_session_lease` (`lease`) + `display_history_prefix` / `profile_home` stamping,
//!   `db.close()` abandon path (`contextlib.suppress`) → all kept inside the injected
//!   `resume_fn` closure; handler only validates `session_id` presence (`4006`) before
//!   delegating, same shape as slice 1 but now covering through the `payload` at
//!   `{"session_id":sid,"resumed":target,"message_count":..., "messages":..., "info":...}` at 1078.
//!   In `std`-only we keep the same `handle_session_resume` stub as slice 1 for routing tests;
//!   the full tail semantics are exercised by the closure and documented here for 1:1 traceability.
//! * `session.cwd.set` — `_sess_nowait` + `running` → `4009`, `cwd` required → `4016`,
//!   `_set_session_cwd` `ValueError` → `4017`, `_session_info` + `_emit("session.info")` →
//!   [`handle_session_cwd_set`] (validates nothing handler-side except delegating;
//!   `4016`/`4017`/`4009` are owned by the injected `cwd_set_fn` so ordering
//!   `session → running → cwd` is preserved).
//! * `session.workspace.move` — `session_key` required → `4007`, `cwd` required → `4016`,
//!   `translate_cwd_for_wsl_backend` + `abspath(expanduser(...))` + `isdir` → `4017`,
//!   live snapshot under `_sessions_lock` by `session_key`, `_git_branch_for_cwd` +
//!   `_git_common_repo_root_for_cwd`, `_profile_db` `5007` / `row_exists` `4007`, `update_session_cwd(...replace_git_meta=True)` → `5007`,
//!   live re-home `_set_session_cwd` → `4017` + `_emit` → [`handle_session_workspace_move`]
//!   validates `session_key` + `cwd` presence (`4007`/`4016`) handler-side then delegates
//!   `4017`/`5007`/`4007` to `workspace_move_fn`.
//! * `session.active_list` — `_sessions_lock` snapshot + `5036`, `not _finalized` filter +
//!   `_session_live_item` → [`handle_session_active_list`] (always `_ok`, errors via `Err`→`5036` in closure).
//! * `session.activate` — `_sess_nowait` + `_live_session_payload(touch=True, omit_messages, transport)` →
//!   [`handle_session_activate`] (delegates, `omit_messages` parsed via `is_truthy_value`).
//! * `session.delete` — `session_id` required `4006`, `list(_sessions.values())` `5036`,
//!   `active = {session_key}` + `4023` when target active, `_profile_home` + `get_hermes_home()/"sessions"`,
//!   `_profile_db` `5036`, `db.delete_session(target, sessions_dir)` `5036`/`4007` →
//!   [`handle_session_delete`] (validates `session_id` `4006` handler-side before delegating).
//! * `session.title` — `_sess_nowait` `5007`, `pending_title` fallback, `get_session_title`/`set_session_title`
//!   `4021`/`4022`/`5007`, `_ensure_session_db_row` + scoped `set_session_title` `pending` flag +
//!   `_emit_session_info_for_session` → [`handle_session_title`] (delegates; title
//!   required `4021` and `4022` owned by `title_fn`).
//! * `session.set_hidden` — `hidden = is_truthy_value(params.get("hidden", True))` (default True),
//!   live path `_session_db` `5007` + `set_session_hidden` else `pending_hidden`, durable
//!   fallback `resolve_session_id` + `set_session_hidden` `5007` → [`handle_session_set_hidden`]
//!   (parses `hidden` via `is_truthy_value` handler-side, delegates DB work).
//! * `message.react` — `_sess_nowait`, `newest_role`∈{user,assistant} else `4023`, `emoji` `4024`, `author` `4025`,
//!   `_session_db` `5007`, `latest_message_row_id` `4040`, `set_message_reaction` `5007`/`4040` →
//!   [`handle_message_react`] (delegates; the `4023`/`4024`/`4025`/`4040`/`5007` codes are owned by `react_fn`).
//! * `llm.oneshot` — `template`/`instructions`/`input` `4030`, `variables` dict, `task` default, `max_tokens` `1024`,
//!   `temperature` `0.3`, `_sessions.get` + `_main_runtime_from_agent`, `run_oneshot` `4031`/`4032`/`5030` →
//!   [`handle_llm_oneshot`] (validates `4030` handler-side for empty template+instructions+input, delegates `4031`/`4032`/`5030`).
//! * `handoff.request` — `_sess_nowait`, `running` `4009`, `platform` `4023`, `Platform(platform_name)` `4024`,
//!   `load_gateway_config` `5021`, `platform.enabled` `4025`, `get_home_channel` `4026`,
//!   `_ensure_session_db_row`, `_session_db` `5007`, `request_handoff` `4027` → [`handle_handoff_request`]
//!   (delegates; all codes owned by `handoff_request_fn`).
//! * `handoff.state` / `handoff.fail` — `_sess_nowait` `5007`, `get_handoff_state`/`fail_handoff` →
//!   [`handle_handoff_state`] / [`handle_handoff_fail`].
//! * `session.usage` — `_sess_nowait`, `_session_usage_snapshot` + `nous_credits_lines` → [`handle_session_usage`]
//!   (always `_ok`).
//! * `session.context_breakdown` — `_sess_nowait`, `agent None` → `[]` fallback, `compute_session_context_breakdown` `5000` →
//!   [`handle_session_context_breakdown`].
//! * `pet.info` (`@_profile_scoped`, `403`-free): `_pet_active_selection` + `_pet_sprite_payload` + `knownRevision` dedup
//!   `spritesheetUnchanged` → [`handle_pet_info`] (fail-open: always `_ok`, `Err`→`{"enabled":False}`).
//! * `pet.info.meta` (`@_profile_scoped`): same selection + `slug`/`displayName`/`scale`/`spritesheetRevision` →
//!   [`handle_pet_info_meta`] (fail-open).
//! * `pet.cells` (`@_profile_scoped`, truncated at 1800 inside kitty `if gmode=="kitty"` before unicode fallback):
//!   `pet_cfg.enabled` `is_truthy_value` → `enabled:False`, `resolve_active_pet` → `enabled:False`,
//!   `state`/`scale`/`cols` (`resolve_cols`), `graphics`+`render_mode`/`detect_terminal_graphics`+`kitty_image_id`+
//!   `PetRenderer(...kitty...).kitty_payload` → [`handle_pet_cells`] (fail-open; slice 2 covers through the kitty
//!   `return _ok(... "graphics":"kitty" ...)` at ~1865; unicode half-block fallback lives in the next slice).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] / [`build_registry`] / [`build_registry_default`]
//!   (deferred via `HandlerRegistry`).

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators for 900-1800
// ---------------------------------------------------------------------------

pub const METHOD_SESSION_CWD_SET: &str = "session.cwd.set";
pub const METHOD_SESSION_WORKSPACE_MOVE: &str = "session.workspace.move";
pub const METHOD_SESSION_ACTIVE_LIST: &str = "session.active_list";
pub const METHOD_SESSION_ACTIVATE: &str = "session.activate";
pub const METHOD_SESSION_DELETE: &str = "session.delete";
pub const METHOD_SESSION_TITLE: &str = "session.title";
pub const METHOD_SESSION_SET_HIDDEN: &str = "session.set_hidden";
pub const METHOD_MESSAGE_REACT: &str = "message.react";
pub const METHOD_LLM_ONESHOT: &str = "llm.oneshot";
pub const METHOD_HANDOFF_REQUEST: &str = "handoff.request";
pub const METHOD_HANDOFF_STATE: &str = "handoff.state";
pub const METHOD_HANDOFF_FAIL: &str = "handoff.fail";
pub const METHOD_SESSION_USAGE: &str = "session.usage";
pub const METHOD_SESSION_CONTEXT_BREAKDOWN: &str = "session.context_breakdown";
pub const METHOD_PET_INFO: &str = "pet.info";
pub const METHOD_PET_INFO_META: &str = "pet.info.meta";
pub const METHOD_PET_CELLS: &str = "pet.cells";

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_SESSION_ID_REQUIRED: i32 = 4006;
pub const ERR_SESSION_KEY_REQUIRED: i32 = 4007;
pub const ERR_SESSION_NOT_FOUND: i32 = 4007;
pub const ERR_CWD_REQUIRED: i32 = 4016;
pub const ERR_CWD_INVALID: i32 = 4017;
pub const ERR_TITLE_REQUIRED: i32 = 4021;
pub const ERR_TITLE_VALUE: i32 = 4022;
pub const ERR_CANNOT_DELETE_ACTIVE: i32 = 4023;
pub const ERR_REACT_TARGET_REQUIRED: i32 = 4023;
pub const ERR_PLATFORM_REQUIRED: i32 = 4023;
pub const ERR_REACT_EMOJI: i32 = 4024;
pub const ERR_UNKNOWN_PLATFORM: i32 = 4024;
pub const ERR_REACT_AUTHOR: i32 = 4025;
pub const ERR_PLATFORM_NOT_ENABLED: i32 = 4025;
pub const ERR_NO_HOME_CHANNEL: i32 = 4026;
pub const ERR_HANDOFF_IN_FLIGHT: i32 = 4027;
pub const ERR_ONESHOT_NEEDS_TEMPLATE: i32 = 4030;
pub const ERR_ONESHOT_KEY: i32 = 4031;
pub const ERR_ONESHOT_VALUE: i32 = 4032;
pub const ERR_REACT_NO_MESSAGE: i32 = 4040;
pub const ERR_REACT_NOT_FOUND: i32 = 4040;
pub const ERR_SESSION_BUSY: i32 = 4009;
pub const ERR_MISSING_SLUG: i32 = 4004;
pub const ERR_DB_UNAVAILABLE: i32 = 5007;
pub const ERR_DB_UNAVAILABLE_LIST: i32 = 5036;
pub const ERR_DB_UNAVAILABLE_DELETE: i32 = 5036;
pub const ERR_ENUM_ACTIVE_FAILED: i32 = 5036;
pub const ERR_GATEWAY_CONFIG: i32 = 5021;
pub const ERR_CONTEXT_FAILED: i32 = 5000;
pub const ERR_ONESHOT_FAILED: i32 = 5030;
pub const ERR_BILLING: i32 = 5031;

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
                    for _ in 0..3 { chars.next(); }
                    out.push('?');
                }
                _ => out.push(ch),
            }
            esc = false;
            continue;
        }
        if ch == '\\' { esc = true; continue; }
        if ch == '"' { return Some(out); }
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
    let mut rest = after[colon + 1..].trim_start();
    if rest.is_empty() { return None; }
    if rest.starts_with('[') || rest.starts_with('{') {
        let open = rest.chars().next().unwrap();
        let close = if open == '[' { ']' } else { '}' };
        let mut depth = 0usize;
        let mut in_str = false;
        let mut esc = false;
        let mut end_idx: Option<usize> = None;
        for (i, ch) in rest.char_indices() {
            if esc { esc = false; continue; }
            if ch == '\\' && in_str { esc = true; continue; }
            if ch == '"' && !esc { in_str = !in_str; continue; }
            if in_str { continue; }
            if ch == open { depth += 1; }
            else if ch == close {
                if depth > 0 { depth -= 1; if depth == 0 { end_idx = Some(i); break; } }
            }
        }
        if let Some(e) = end_idx { return Some(rest[..=e].to_string()); }
        return None;
    }
    if rest.starts_with('"') || rest.starts_with('\'') {
        let qc = rest.chars().next().unwrap();
        let mut esc = false;
        for (i, ch) in rest[1..].char_indices() {
            if esc { esc = false; continue; }
            if ch == '\\' { esc = true; continue; }
            if ch == qc { return Some(rest[..=1 + i].to_string()); }
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

/// Mirrors `is_truthy_value(v)` + default handling (params.get("hidden", True) → truthy default).
pub fn is_truthy_value(raw: Option<&str>) -> bool {
    match raw {
        None => false,
        Some(s) => {
            let t = s.trim().to_ascii_lowercase();
            if t.is_empty() || t == "0" || t == "false" || t == "no" || t == "off" || t == "n" || t == "f" { return false; }
            if t == "true" || t == "1" || t == "yes" || t == "on" || t == "y" || t == "t" { return true; }
            if let Ok(n) = t.parse::<i64>() { return n != 0; }
            if let Ok(f) = t.parse::<f64>() { return f != 0.0 && f.is_finite(); }
            false
        }
    }
}

pub fn is_truthy_value_with_default(raw: Option<&str>, default: bool) -> bool {
    match raw {
        None => default,
        Some(s) => {
            // When key absent Python uses default via params.get("hidden", True) → hidden=True.
            // Extract helpers handle absent as None; caller passes default accordingly.
            is_truthy_value(Some(s))
        }
    }
}

pub fn is_truthy_field(params_json: &str, field: &str) -> bool {
    let raw = extract_raw_value(params_json, field);
    is_truthy_value(raw.as_deref().map(|s| s.trim().trim_matches('"')))
}

pub fn is_truthy_field_with_default(params_json: &str, field: &str, default: bool) -> bool {
    let raw = extract_raw_value(params_json, field);
    match raw {
        None => default,
        Some(v) => is_truthy_value(Some(v.trim().trim_matches('"'))),
    }
}

// ---------------------------------------------------------------------------
// Param helpers for this slice
// ---------------------------------------------------------------------------

pub fn extract_cwd_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "cwd")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn extract_session_key_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "session_key")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn extract_session_id_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "session_id")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn parse_oneshot_requires_template(params_json: &str) -> bool {
    let template = extract_string_field(params_json, "template").unwrap_or_default().trim().to_string();
    let instructions = extract_string_field(params_json, "instructions").unwrap_or_default().trim().to_string();
    let input = extract_string_field(params_json, "input").unwrap_or_default().trim().to_string();
    // Also check raw presence for template variable fallback? Python checks `template` truthiness plus instructions/input stripped.
    // We mirror: empty template and empty instructions and empty input → missing
    template.is_empty() && instructions.is_empty() && input.is_empty()
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `session.cwd.set`.
///
/// Delegates `_sess_nowait` + `running` `4009` + `cwd` `4016` + `_set_session_cwd` `4017` + `_session_info` + `_emit`
/// to `cwd_set_fn`. Validating `cwd required` is owned by the closure so
/// `running → cwd` ordering (`busy` wins over `cwd required` when both apply) is preserved.
pub fn handle_session_cwd_set<F>(rid_json: &str, params_json: &str, cwd_set_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match cwd_set_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `session.workspace.move`.
///
/// Validates `session_key` required (`4007`) and `cwd` required (`4016`) handler-side
/// (both are pre-lock checks in Python), then delegates `4017`/`5007`/`4007` to `move_fn`.
pub fn handle_session_workspace_move<F>(rid_json: &str, params_json: &str, move_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let target = extract_string_field(params_json, "session_key").unwrap_or_default().trim().to_string();
    if target.is_empty() {
        return err_response(rid_json, ERR_SESSION_KEY_REQUIRED, "session_key required");
    }
    let raw_cwd = extract_string_field(params_json, "cwd").unwrap_or_default().trim().to_string();
    if raw_cwd.is_empty() {
        return err_response(rid_json, ERR_CWD_REQUIRED, "cwd required");
    }
    match move_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `session.active_list`.
///
/// Always returns `_ok` envelope; `5036` is produced by closure on lock failure.
pub fn handle_session_active_list<F>(rid_json: &str, params_json: &str, active_list_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match active_list_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `session.activate`.
///
/// Delegates `_sess_nowait` + `_live_session_payload` to `activate_fn`.
pub fn handle_session_activate<F>(rid_json: &str, params_json: &str, activate_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match activate_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `session.delete`.
///
/// Validates `session_id` required (`4006`) handler-side (pre-lock check), then delegates.
pub fn handle_session_delete<F>(rid_json: &str, params_json: &str, delete_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let target = extract_raw_value(params_json, "session_id")
        .map(|s| s.trim().trim_matches('"').to_string())
        .unwrap_or_default();
    if target.trim().is_empty() || target.trim() == "null" {
        return err_response(rid_json, ERR_SESSION_ID_REQUIRED, "session_id required");
    }
    match delete_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `session.title`.
///
/// Delegates `_sess_nowait` + `_session_db` `5007` + `pending_title` + `4021`/`4022` to `title_fn`.
pub fn handle_session_title<F>(rid_json: &str, params_json: &str, title_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match title_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `session.set_hidden`.
///
/// Parses `hidden` with default `True` (mirrors `params.get("hidden", True)`), then delegates
/// the live-vs-durable DB path to `set_hidden_fn`. The `hidden` parse is handler-side so
/// the closure receives the normalized bool.
pub fn handle_session_set_hidden<F>(rid_json: &str, params_json: &str, set_hidden_fn: F) -> String
where
    F: Fn(&str, bool) -> Result<String, (i32, String)>,
{
    let hidden = is_truthy_field_with_default(params_json, "hidden", true);
    match set_hidden_fn(params_json, hidden) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `message.react`.
///
/// Delegates all validation (`4023`/`4024`/`4025`/`4040`/`5007`) to `react_fn` so
/// `_sess_nowait` → `newest_role` ordering is preserved.
pub fn handle_message_react<F>(rid_json: &str, params_json: &str, react_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match react_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `llm.oneshot`.
///
/// Validates `4030` (requires template or instructions/input) handler-side (pre-session check),
/// then delegates `4031`/`4032`/`5030` to `oneshot_fn`.
pub fn handle_llm_oneshot<F>(rid_json: &str, params_json: &str, oneshot_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    if parse_oneshot_requires_template(params_json) {
        return err_response(rid_json, ERR_ONESHOT_NEEDS_TEMPLATE, "llm.oneshot requires a template or instructions/input");
    }
    match oneshot_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `handoff.request`.
///
/// Delegates `_sess_nowait` + `running` `4009` + `platform` `4023` + `Platform` `4024` + `load_gateway_config` `5021` +
/// `enabled` `4025` + `home_channel` `4026` + `request_handoff` `4027`/`5007` to `handoff_fn`.
pub fn handle_handoff_request<F>(rid_json: &str, params_json: &str, handoff_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match handoff_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `handoff.state`.
pub fn handle_handoff_state<F>(rid_json: &str, params_json: &str, state_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match state_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `handoff.fail`.
pub fn handle_handoff_fail<F>(rid_json: &str, params_json: &str, fail_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match fail_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `session.usage`.
///
/// Always returns `_ok` envelope (never `_err`) via the closure's `usage` payload.
pub fn handle_session_usage<F>(rid_json: &str, params_json: &str, usage_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match usage_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `session.context_breakdown`.
///
/// Delegates `5000` on `compute_session_context_breakdown` failure to `breakdown_fn`.
pub fn handle_session_context_breakdown<F>(rid_json: &str, params_json: &str, breakdown_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match breakdown_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

// --- pet handlers (profile-scoped, fail-open → always _ok) ---

/// Handle `pet.info` (profile-scoped, fail-open).
///
/// Mirrors `try: enabled, pet, scale = _pet_active_selection(); if not enabled → {"enabled":False}; payload = {"enabled":True, **_pet_sprite_payload}; knownRevision dedup; except: {"enabled":False}`.
/// The injected `pet_fn` owns the `_pet_active_selection` + `_pet_sprite_payload` reads; this wrapper maps `Err`→`{"enabled":false}` so the envelope stays `_ok`.
pub fn handle_pet_info<F>(rid_json: &str, params_json: &str, pet_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match pet_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, r#"{"enabled":false}"#),
    }
}

/// Handle `pet.info.meta` (profile-scoped, fail-open).
pub fn handle_pet_info_meta<F>(rid_json: &str, params_json: &str, meta_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match meta_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, r#"{"enabled":false}"#),
    }
}

/// Handle `pet.cells` (profile-scoped, fail-open, truncated at 1800 inside kitty branch).
///
/// Mirrors the slice-2 prefix: `pet_cfg.enabled` → `enabled:False`, `resolve_active_pet` → `enabled:False`,
/// `state`/`scale`/`cols` defaults, `graphics` + `render_mode` + `detect_terminal_graphics` + `kitty_image_id` +
/// `PetRenderer(..., mode="kitty").kitty_payload` kitty early-return.
/// Unicode half-block fallback (`mode="unicode"`, `cells(...)`, `frames`) continues in the next slice
/// and is modelled as the tail of the injected `cells_fn`.
pub fn handle_pet_cells<F>(rid_json: &str, params_json: &str, cells_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match cells_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, r#"{"enabled":false}"#),
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with the 17 slice-2 methods registered
/// using the provided deps (for tests / production injection).
///
/// Each closure is `'static` and mirrors the lazy imports inside Python
/// handler bodies. For the default stub (no backend) use [`build_registry_default`].
#[allow(clippy::too_many_arguments)]
pub fn build_registry<C, W, A, V, D, T, H, R, O, HR, ST, FA, U, CB, PI, PM, CE>(
    session_cwd_set: C,
    session_workspace_move: W,
    session_active_list: A,
    session_activate: V,
    session_delete: D,
    session_title: T,
    session_set_hidden: H,
    message_react: R,
    llm_oneshot: O,
    handoff_request: HR,
    handoff_state: ST,
    handoff_fail: FA,
    session_usage: U,
    session_context_breakdown: CB,
    pet_info: PI,
    pet_info_meta: PM,
    pet_cells: CE,
) -> HandlerRegistry
where
    C: Fn(String, String) -> String + Send + Sync + 'static,
    W: Fn(String, String) -> String + Send + Sync + 'static,
    A: Fn(String, String) -> String + Send + Sync + 'static,
    V: Fn(String, String) -> String + Send + Sync + 'static,
    D: Fn(String, String) -> String + Send + Sync + 'static,
    T: Fn(String, String) -> String + Send + Sync + 'static,
    H: Fn(String, String) -> String + Send + Sync + 'static,
    R: Fn(String, String) -> String + Send + Sync + 'static,
    O: Fn(String, String) -> String + Send + Sync + 'static,
    HR: Fn(String, String) -> String + Send + Sync + 'static,
    ST: Fn(String, String) -> String + Send + Sync + 'static,
    FA: Fn(String, String) -> String + Send + Sync + 'static,
    U: Fn(String, String) -> String + Send + Sync + 'static,
    CB: Fn(String, String) -> String + Send + Sync + 'static,
    PI: Fn(String, String) -> String + Send + Sync + 'static,
    PM: Fn(String, String) -> String + Send + Sync + 'static,
    CE: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        session_cwd_set,
        session_workspace_move,
        session_active_list,
        session_activate,
        session_delete,
        session_title,
        session_set_hidden,
        message_react,
        llm_oneshot,
        handoff_request,
        handoff_state,
        handoff_fail,
        session_usage,
        session_context_breakdown,
        pet_info,
        pet_info_meta,
        pet_cells,
    );
    reg
}

/// Build a registry with default stubs (every operation returns error / `enabled:false`).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_cwd_set(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_workspace_move(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_active_list(&rid_json, &params_json, |_| Err((ERR_ENUM_ACTIVE_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_activate(&rid_json, &params_json, |_| Err((ERR_SESSION_NOT_FOUND, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_delete(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE_DELETE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_title(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_set_hidden(&rid_json, &params_json, |_, _| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_message_react(&rid_json, &params_json, |_| Err((ERR_REACT_NOT_FOUND, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_llm_oneshot(&rid_json, &params_json, |_| Err((ERR_ONESHOT_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_handoff_request(&rid_json, &params_json, |_| Err((ERR_GATEWAY_CONFIG, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_handoff_state(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_handoff_fail(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_usage(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_context_breakdown(&rid_json, &params_json, |_| Err((ERR_CONTEXT_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_info(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_info_meta(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_cells(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
    )
}

/// Register all 17 slice-2 methods onto an existing registry.
#[allow(clippy::too_many_arguments)]
pub fn register_with<C, W, A, V, D, T, H, R, O, HR, ST, FA, U, CB, PI, PM, CE>(
    registry: &mut HandlerRegistry,
    session_cwd_set: C,
    session_workspace_move: W,
    session_active_list: A,
    session_activate: V,
    session_delete: D,
    session_title: T,
    session_set_hidden: H,
    message_react: R,
    llm_oneshot: O,
    handoff_request: HR,
    handoff_state: ST,
    handoff_fail: FA,
    session_usage: U,
    session_context_breakdown: CB,
    pet_info: PI,
    pet_info_meta: PM,
    pet_cells: CE,
) where
    C: Fn(String, String) -> String + Send + Sync + 'static,
    W: Fn(String, String) -> String + Send + Sync + 'static,
    A: Fn(String, String) -> String + Send + Sync + 'static,
    V: Fn(String, String) -> String + Send + Sync + 'static,
    D: Fn(String, String) -> String + Send + Sync + 'static,
    T: Fn(String, String) -> String + Send + Sync + 'static,
    H: Fn(String, String) -> String + Send + Sync + 'static,
    R: Fn(String, String) -> String + Send + Sync + 'static,
    O: Fn(String, String) -> String + Send + Sync + 'static,
    HR: Fn(String, String) -> String + Send + Sync + 'static,
    ST: Fn(String, String) -> String + Send + Sync + 'static,
    FA: Fn(String, String) -> String + Send + Sync + 'static,
    U: Fn(String, String) -> String + Send + Sync + 'static,
    CB: Fn(String, String) -> String + Send + Sync + 'static,
    PI: Fn(String, String) -> String + Send + Sync + 'static,
    PM: Fn(String, String) -> String + Send + Sync + 'static,
    CE: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method(METHOD_SESSION_CWD_SET, session_cwd_set);
    registry.method(METHOD_SESSION_WORKSPACE_MOVE, session_workspace_move);
    registry.method(METHOD_SESSION_ACTIVE_LIST, session_active_list);
    registry.method(METHOD_SESSION_ACTIVATE, session_activate);
    registry.method(METHOD_SESSION_DELETE, session_delete);
    registry.method(METHOD_SESSION_TITLE, session_title);
    registry.method(METHOD_SESSION_SET_HIDDEN, session_set_hidden);
    registry.method(METHOD_MESSAGE_REACT, message_react);
    registry.method(METHOD_LLM_ONESHOT, llm_oneshot);
    registry.method(METHOD_HANDOFF_REQUEST, handoff_request);
    registry.method(METHOD_HANDOFF_STATE, handoff_state);
    registry.method(METHOD_HANDOFF_FAIL, handoff_fail);
    registry.method(METHOD_SESSION_USAGE, session_usage);
    registry.method(METHOD_SESSION_CONTEXT_BREAKDOWN, session_context_breakdown);
    registry.method_profile_scoped(METHOD_PET_INFO, pet_info);
    registry.method_profile_scoped(METHOD_PET_INFO_META, pet_info_meta);
    registry.method_profile_scoped(METHOD_PET_CELLS, pet_cells);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_cwd_set(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_workspace_move(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_active_list(&rid_json, &params_json, |_| Err((ERR_ENUM_ACTIVE_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_activate(&rid_json, &params_json, |_| Err((ERR_SESSION_NOT_FOUND, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_delete(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE_DELETE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_title(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_set_hidden(&rid_json, &params_json, |_, _| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_message_react(&rid_json, &params_json, |_| Err((ERR_REACT_NOT_FOUND, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_llm_oneshot(&rid_json, &params_json, |_| Err((ERR_ONESHOT_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_handoff_request(&rid_json, &params_json, |_| Err((ERR_GATEWAY_CONFIG, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_handoff_state(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_handoff_fail(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_usage(&rid_json, &params_json, |_| Err((ERR_DB_UNAVAILABLE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_context_breakdown(&rid_json, &params_json, |_| Err((ERR_CONTEXT_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_info(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_info_meta(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_cells(&rid_json, &params_json, |_| Err("no backend".to_string()))
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

    fn rid1() -> String { encode_rid("1") }

    #[test]
    fn workspace_move_requires_keys() {
        let rid = rid1();
        let out = handle_session_workspace_move(&rid, r#"{}"#, |_| Ok(r#"{"cwd":"/tmp"}"#.to_string()));
        assert!(out.contains(r#""code":4007"#), "{}", out);
        assert!(out.contains("session_key required"));
        let out2 = handle_session_workspace_move(&rid, r#"{"session_key":"abc"}"#, |_| Ok(r#"{"cwd":"/tmp"}"#.to_string()));
        assert!(out2.contains(r#""code":4016"#), "{}", out2);
        let out3 = handle_session_workspace_move(&rid, r#"{"session_key":"abc","cwd":"/tmp/foo"}"#, |_| Ok(r#"{"cwd":"/tmp/foo","branch":"main","git_repo_root":"/tmp"}"#.to_string()));
        assert!(out3.contains(r#""cwd":"/tmp/foo""#), "{}", out3);
        let out4 = handle_session_workspace_move(&rid, r#"{"session_key":"abc","cwd":"/tmp/foo"}"#, |_| Err((4017, "working directory does not exist: /bad".into())));
        assert!(out4.contains(r#""code":4017"#));
    }

    #[test]
    fn delete_requires_id() {
        let rid = rid1();
        let out = handle_session_delete(&rid, r#"{}"#, |_| Ok(r#"{"deleted":"x"}"#.to_string()));
        assert!(out.contains(r#""code":4006"#), "{}", out);
        let out2 = handle_session_delete(&rid, r#"{"session_id":"abc"}"#, |_| Ok(r#"{"deleted":"abc"}"#.to_string()));
        assert!(out2.contains(r#""deleted":"abc""#));
        let out3 = handle_session_delete(&rid, r#"{"session_id":"abc"}"#, |_| Err((4023, "cannot delete an active session".into())));
        assert!(out3.contains(r#""code":4023"#));
    }

    #[test]
    fn llm_oneshot_requires_template() {
        let rid = rid1();
        let out = handle_llm_oneshot(&rid, r#"{}"#, |_| Ok(r#"{"text":"hi"}"#.to_string()));
        assert!(out.contains(r#""code":4030"#), "{}", out);
        let out2 = handle_llm_oneshot(&rid, r#"{"template":"commit"}"#, |_| Ok(r#"{"text":"feat: foo"}"#.to_string()));
        assert!(out2.contains(r#""text""#));
        let out3 = handle_llm_oneshot(&rid, r#"{"instructions":"summarize","input":"hello"}"#, |_| Ok(r#"{"text":"hi"}"#.to_string()));
        assert!(out3.contains(r#""text""#));
        let out4 = handle_llm_oneshot(&rid, r#"{"template":"t"}"#, |_| Err((5030, "one-shot generation failed: boom".into())));
        assert!(out4.contains(r#""code":5030"#));
    }

    #[test]
    fn cwd_set_delegates() {
        let rid = rid1();
        let out = handle_session_cwd_set(&rid, r#"{"cwd":"/tmp"}"#, |_| Ok(r#"{"cwd":"/tmp","branch":"main","project":{"name":"proj"},"lazy":true}"#.to_string()));
        assert!(out.contains(r#""cwd":"/tmp""#), "{}", out);
        let out2 = handle_session_cwd_set(&rid, r#"{"cwd":""}"#, |_| Err((4016, "cwd required".into())));
        assert!(out2.contains(r#""code":4016"#));
        let out3 = handle_session_cwd_set(&rid, r#"{"cwd":"/tmp"}"#, |_| Err((4009, "session busy".into())));
        assert!(out3.contains(r#""code":4009"#));
        let out4 = handle_session_cwd_set(&rid, r#"{"cwd":"/bad"}"#, |_| Err((4017, "working directory does not exist: /bad".into())));
        assert!(out4.contains(r#""code":4017"#));
    }

    #[test]
    fn message_react_delegates() {
        let rid = rid1();
        let out = handle_message_react(&rid, r#"{"row_id":1,"emoji":"👍"}"#, |_| Ok(r#"{"row_id":1,"reactions":{"👍":["user"]}}"#.to_string()));
        assert!(out.contains(r#""row_id":1"#), "{}", out);
        let out2 = handle_message_react(&rid, r#"{}"#, |_| Err((4023, "row_id or newest_role required".into())));
        assert!(out2.contains(r#""code":4023"#));
        let out3 = handle_message_react(&rid, r#"{"row_id":999}"#, |_| Err((4040, "message not found in this session".into())));
        assert!(out3.contains(r#""code":4040"#));
    }

    #[test]
    fn handoff_delegates() {
        let rid = rid1();
        let out = handle_handoff_request(&rid, r#"{"platform":"slack"}"#, |_| Ok(r#"{"queued":true,"session_key":"abc","platform":"slack","home_name":"Home"}"#.to_string()));
        assert!(out.contains(r#""queued":true"#), "{}", out);
        let out2 = handle_handoff_request(&rid, r#"{}"#, |_| Err((4023, "platform required".into())));
        assert!(out2.contains(r#""code":4023"#));
        let out3 = handle_handoff_request(&rid, r#"{"platform":"unknown"}"#, |_| Err((4024, "unknown platform 'unknown'".into())));
        assert!(out3.contains(r#""code":4024"#));
    }

    #[test]
    fn handoff_state_and_fail_ok() {
        let rid = rid1();
        let out = handle_handoff_state(&rid, r#"{"session_id":"a"}"#, |_| Ok(r#"{"state":"pending","platform":"slack","error":""}"#.to_string()));
        assert!(out.contains(r#""state":"pending""#), "{}", out);
        let out2 = handle_handoff_fail(&rid, r#"{"session_id":"a"}"#, |_| Ok(r#"{"failed":true,"state":"failed"}"#.to_string()));
        assert!(out2.contains(r#""failed":true"#));
        let out3 = handle_handoff_fail(&rid, r#"{"session_id":"a"}"#, |_| Ok(r#"{"failed":false,"state":"completed"}"#.to_string()));
        assert!(out3.contains(r#""failed":false"#));
    }

    #[test]
    fn usage_and_breakdown() {
        let rid = rid1();
        let out = handle_session_usage(&rid, r#"{"session_id":"a"}"#, |_| Ok(r#"{"calls":1,"input":10,"output":20,"total":30}"#.to_string()));
        assert!(out.contains(r#""total":30"#), "{}", out);
        let out2 = handle_session_context_breakdown(&rid, r#"{"session_id":"a"}"#, |_| Ok(r#"{"categories":[],"context_max":100000,"context_used":1234}"#.to_string()));
        assert!(out2.contains("context_max"), "{}", out2);
        let out3 = handle_session_context_breakdown(&rid, "{}", |_| Err((5000, "Could not compute context breakdown: boom".into())));
        assert!(out3.contains(r#""code":5000"#));
    }

    #[test]
    fn pet_info_fail_open() {
        let rid = rid1();
        let out = handle_pet_info(&rid, r#"{}"#, |_| Ok(r#"{"enabled":true,"slug":"cat","displayName":"Cat","scale":1.0,"spritesheetRevision":"abc"}"#.to_string()));
        assert!(out.contains(r#""enabled":true"#), "{}", out);
        let out2 = handle_pet_info(&rid, "{}", |_| Err("boom".into()));
        assert!(out2.contains(r#""enabled":false"#), "{}", out2);
        assert!(!out2.contains(r#""error""#), "pet.info fail-open should be _ok not _err: {}", out2);
        let out3 = handle_pet_info_meta(&rid, "{}", |_| Err("boom".into()));
        assert!(out3.contains(r#""enabled":false"#));
        let out4 = handle_pet_cells(&rid, r#"{"state":"idle","cols":20}"#, |_| Ok(r#"{"enabled":true,"slug":"cat","state":"idle","cols":20,"frames":[[[[255,255,255,255,0,0,0,255]]]]}"#.to_string()));
        assert!(out4.contains(r#""enabled":true"#));
        let out5 = handle_pet_cells(&rid, "{}", |_| Err("boom".into()));
        assert!(out5.contains(r#""enabled":false"#));
    }

    #[test]
    fn active_list_and_activate() {
        let rid = rid1();
        let out = handle_session_active_list(&rid, r#"{"current_session_id":"x"}"#, |_| Ok(r#"{"sessions":[]}"#.to_string()));
        assert!(out.contains(r#""sessions":[]"#), "{}", out);
        let out2 = handle_session_active_list(&rid, "{}", |_| Err((5036, "could not enumerate active sessions: boom".into())));
        assert!(out2.contains(r#""code":5036"#));
        let out3 = handle_session_activate(&rid, r#"{"session_id":"abc"}"#, |_| Ok(r#"{"session_id":"abc","resumed":"key","messages":[]}"#.to_string()));
        assert!(out3.contains("abc"));
        let out4 = handle_session_title(&rid, r#"{"session_id":"a","title":"New Title"}"#, |_| Ok(r#"{"pending":false,"title":"New Title"}"#.to_string()));
        assert!(out4.contains("New Title"));
        let out5 = handle_session_title(&rid, r#"{"session_id":"a"}"#, |_| Err((4021, "title required".into())));
        assert!(out5.contains(r#""code":4021"#));
    }

    #[test]
    fn set_hidden_parses_default_true() {
        let rid = rid1();
        assert!(is_truthy_field_with_default(r#"{}"#, "hidden", true));
        assert!(!is_truthy_field_with_default(r#"{"hidden":false}"#, "hidden", true));
        assert!(is_truthy_field_with_default(r#"{"hidden":true}"#, "hidden", true));
        assert!(!is_truthy_field_with_default(r#"{"hidden":0}"#, "hidden", true));
        let out = handle_session_set_hidden(&rid, r#"{"session_id":"a","hidden":true}"#, |_, hidden| {
            assert!(hidden);
            Ok(r#"{"hidden":true,"session_key":"key"}"#.to_string())
        });
        assert!(out.contains(r#""hidden":true"#), "{}", out);
        let out2 = handle_session_set_hidden(&rid, r#"{"session_id":"a"}"#, |_, hidden| {
            assert!(hidden); // default True
            Ok(r#"{"hidden":true,"session_key":"key"}"#.to_string())
        });
        assert!(out2.contains(r#""hidden":true"#));
    }

    #[test]
    fn registry_installs_seventeen() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 17);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["handoff.fail","handoff.request","handoff.state","llm.oneshot","message.react","pet.cells","pet.info","pet.info.meta","session.active_list","session.activate","session.context_breakdown","session.cwd.set","session.delete","session.set_hidden","session.title","session.usage","session.workspace.move"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 17);
        // pet.info fail-open should be ok enabled false even with no backend stub
        let out = map.get(METHOD_PET_INFO).unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains(r#""enabled":false"#), "{}", out);
        // workspace.move with missing session_key → 4007 even with no backend
        let out2 = map.get(METHOD_SESSION_WORKSPACE_MOVE).unwrap()("1".to_string(), "{}".to_string());
        assert!(out2.contains(r#""code":4007"#), "{}", out2);
        // delete missing id → 4006
        let out3 = map.get(METHOD_SESSION_DELETE).unwrap()("1".to_string(), "{}".to_string());
        assert!(out3.contains(r#""code":4006"#), "{}", out3);
        // llm missing template → 4030
        let out4 = map.get(METHOD_LLM_ONESHOT).unwrap()("1".to_string(), "{}".to_string());
        assert!(out4.contains(r#""code":4030"#), "{}", out4);
    }

    #[test]
    fn ok_err_envelope_shape() {
        let rid = encode_rid("42");
        let ok = ok_response(&rid, r#"{"cwd":"/tmp"}"#);
        assert!(ok.contains(r#""result""#));
        assert!(ok.contains("/tmp"));
        let err = err_response(&rid, 4007, "session_key required");
        assert!(err.contains(r#""code":4007"#));
        assert!(err.contains("session_key required"));
    }

    #[test]
    fn extract_helpers() {
        assert_eq!(extract_string_field(r#"{"session_key":"abc"}"#, "session_key").as_deref(), Some("abc"));
        assert_eq!(extract_string_field(r#"{"cwd":"/tmp/foo"}"#, "cwd").as_deref(), Some("/tmp/foo"));
        assert_eq!(extract_string_field(r#"{"hidden":true}"#, "hidden"), None);
        assert!(is_truthy_field(r#"{"hidden":true}"#, "hidden"));
        assert!(!is_truthy_field(r#"{"hidden":false}"#, "hidden"));
        assert_eq!(extract_raw_value(r#"{"session_id":"x"}"#, "session_id").unwrap(), r#""x""#);
    }
}
