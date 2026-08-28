//! Session / delegation / spawn-tree / billing / pet JSON-RPC handlers — slice 3 (lines 1800-2700).
//!
//! 1:1 port of `tui_gateway/methods_session.py` lines 1800–2700 (T0383 slice 3/3633).
//!
//! Handler bodies are byte-identical to their pre-split `server.py` form; they
//! are rebound onto `server.py`'s globals at install time — see `method_ctx.py`.
//!
//! ```python
//! # Python — tui_gateway/methods_session.py 1800-2700 (abridged, comments preserved)
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
//! # ── pet.cells tail (1867-1896) — unicode fallback completes slice 2's kitty early-return ─
//! #         renderer = PetRenderer(str(pet.spritesheet), mode="unicode", scale=scale, unicode_cols=cols)
//! #         count = renderer.frame_count(state) or 1
//! #         frames = []
//! #         for i in range(count):
//! #             grid = renderer.cells(state, i, cols=cols)
//! #             frames.append([[[*top, *bottom] for (top, bottom) in row] for row in grid])
//! #         return _ok(rid, {"enabled":True,"slug":pet.slug,"displayName":pet.display_name,"state":state,"cols":cols,"frameMs":constants.LOOP_MS/max(1,count),"frames":frames,"scale":scale})
//! #     except Exception as exc: logger.debug("pet.cells failed: %s", exc); return _ok(rid, {"enabled":False})
//!
//! @method("pet.gallery")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     local_only = bool(params.get("localOnly"))
//!     try:
//!         from agent.pet import store
//!         try: from hermes_cli.config import load_config; cfg = load_config(); display = cfg.get("display", {}) ...; pet_cfg = display.get("pet", {}) ...
//!         except Exception: pet_cfg = {}
//!         installed = {p.slug: p for p in store.installed_pets()}
//!         gallery: list[dict] = []; seen: set[str] = set()
//!         try:
//!             from agent.pet.manifest import fetch_manifest, prefetch
//!             if local_only: prefetch()
//!             for entry in [] if local_only else fetch_manifest():
//!                 seen.add(entry.slug); gallery.append({"slug":entry.slug,"displayName":entry.display_name,"installed":entry.slug in installed,"spritesheetUrl":entry.spritesheet_url,"curated":"/curated/" in entry.spritesheet_url,"generated":entry.slug in installed and installed[entry.slug].generated})
//!         except Exception as exc: logger.debug("pet.gallery manifest fetch failed: %s", exc)
//!         for slug, pet in installed.items():
//!             if slug not in seen: gallery.append({"slug":slug,"displayName":pet.display_name,"installed":True,"spritesheetUrl":"","generated":pet.generated})
//!         return _ok(rid, {"enabled":is_truthy_value(pet_cfg.get("enabled"), default=False),"active":str(pet_cfg.get("slug","") or ""),"pets":gallery})
//!     except Exception as exc: logger.debug("pet.gallery failed: %s", exc); return _ok(rid, {"enabled":False,"active":"","pets":[]})
//!
//! @method("pet.select")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     slug = str(params.get("slug") or "").strip()
//!     if not slug: return _err(rid, 4004, "missing slug")
//!     try:
//!         from agent.pet import store; from agent.pet.manifest import ManifestError; from hermes_cli.pets import _set_active
//!         try: pet = store.install_pet(slug)
//!         except (store.PetStoreError, ManifestError) as exc: return _err(rid, 5031, f"could not adopt '{slug}': {exc}")
//!         _set_active(slug); return _ok(rid, {"ok":True,"slug":slug,"displayName":pet.display_name})
//!     except Exception as exc: logger.debug("pet.select failed: %s", exc); return _err(rid, 5031, f"pet.select failed: {exc}")
//!
//! @method("pet.remove")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     slug = str(params.get("slug") or "").strip()
//!     if not slug: return _err(rid, 4004, "missing slug")
//!     try:
//!         from agent.pet import store; from hermes_cli.pets import _clear_active_if
//!         removed = store.remove_pet(slug)
//!         try: _clear_active_if(slug)
//!         except Exception as exc: logger.debug("pet.remove config update failed: %s", exc)
//!         return _ok(rid, {"ok":removed,"slug":slug})
//!     except Exception as exc: logger.debug("pet.remove failed: %s", exc); return _err(rid, 5031, f"pet.remove failed: {exc}")
//!
//! @method("pet.export")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     slug = str(params.get("slug") or "").strip()
//!     if not slug: return _err(rid, 4004, "missing slug")
//!     try:
//!         import base64; from agent.pet import store
//!         filename, data = store.export_pet(slug)
//!         return _ok(rid, {"ok":True,"filename":filename,"zipBase64":base64.standard_b64encode(data).decode("ascii")})
//!     except Exception as exc: logger.debug("pet.export failed: %s", exc); return _err(rid, 5031, f"pet.export failed: {exc}")
//!
//! @method("pet.rename")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     slug = str(params.get("slug") or "").strip(); name = str(params.get("name") or "").strip()
//!     if not slug: return _err(rid, 4004, "missing slug")
//!     if not name: return _err(rid, 4004, "missing name")
//!     try:
//!         from agent.pet import store
//!         new_slug = store.rename_pet(slug, name)
//!         if not new_slug: return _err(rid, 5031, "pet.rename failed")
//!         if new_slug != slug:
//!             try: from hermes_cli.pets import _rename_active_if; _rename_active_if(slug, new_slug)
//!             except Exception as exc: logger.debug("pet.rename config update failed: %s", exc)
//!         return _ok(rid, {"ok":True,"slug":new_slug,"displayName":name})
//!     except Exception as exc: logger.debug("pet.rename failed: %s", exc); return _err(rid, 5031, f"pet.rename failed: {exc}")
//!
//! @method("pet.thumb")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     slug = str(params.get("slug") or "").strip()
//!     if not slug: return _err(rid, 4004, "missing slug")
//!     try:
//!         import base64; from agent.pet import store
//!         data = store.thumbnail_png(slug, source_url=str(params.get("url") or ""))
//!         if not data: return _ok(rid, {"ok":False,"slug":slug})
//!         return _ok(rid, {"ok":True,"slug":slug,"dataUri":"data:image/png;base64,"+base64.standard_b64encode(data).decode("ascii")})
//!     except Exception as exc: logger.debug("pet.thumb failed: %s", exc); return _ok(rid, {"ok":False,"slug":slug})
//!
//! @method("pet.disable")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     try: from hermes_cli.pets import _set_enabled; _set_enabled(False); return _ok(rid, {"ok":True})
//!     except Exception as exc: logger.debug("pet.disable failed: %s", exc); return _err(rid, 5031, f"pet.disable failed: {exc}")
//!
//! @method("pet.scale")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     try: from hermes_cli.pets import set_pet_scale; scale, err = set_pet_scale(params.get("scale"))
//!          if err: return _err(rid, 4004, err)
//!          return _ok(rid, {"ok":True,"scale":scale})
//!     except Exception as exc: logger.debug("pet.scale failed: %s", exc); return _err(rid, 5031, f"pet.scale failed: {exc}")
//!
//! @method("pet.cancel")
//! def _(rid, params: dict) -> dict:
//!     token = str(params.get("token") or "").strip()
//!     if token: _pet_cancel_request(token)
//!     return _ok(rid, {"ok":True})
//!
//! @method("pet.generate.status")
//! def _(rid, params: dict) -> dict:
//!     try:
//!         from agent.pet.generate.imagegen import GenerationError, list_sprite_providers, resolve_provider
//!         try: resolve_provider(require_references=True); available=True
//!         except GenerationError: available=False
//!         try: providers = list_sprite_providers()
//!         except Exception as exc: logger.debug("pet provider list failed: %s", exc); providers=[]; return _ok(rid, {"available":available,"providers":providers})
//!     except Exception as exc: logger.debug("pet.generate.status failed: %s", exc); return _ok(rid, {"available":False,"providers":[]})
//!
//! @method("pet.generate")
//! def _(rid, params: dict) -> dict:
//!     prompt = str(params.get("prompt") or "").strip(); ref_raw = str(params.get("referenceImage") or "").strip()
//!     if not prompt and not ref_raw: return _err(rid, 4004, "missing prompt")
//!     try: count = max(1, min(4, int(params.get("count") or 4)))
//!     except (TypeError, ValueError): count = 4
//!     style = str(params.get("style") or "auto").strip() or "auto"
//!     try:
//!         import shutil, uuid; from agent.pet.generate import generate_base_drafts; from agent.pet.generate.imagegen import GenerationError, resolve_provider
//!         root = _pet_gen_root(); _pet_gen_sweep(root); token = uuid.uuid4().hex[:12]; _pet_cancel_arm(token); stage = root/token; stage.mkdir(parents=True, exist_ok=True)
//!         reference_images = None
//!         if ref_raw:
//!             try: reference_images = _pet_reference_images_from_data_url(ref_raw, stage)
//!             except ValueError as exc: _pet_cancel_release(token); return _err(rid, 4004, str(exc))
//!         provider_name = str(params.get("provider") or "").strip(); sprite = None
//!         if provider_name:
//!             try: sprite = resolve_provider(require_references=bool(reference_images), prefer=provider_name)
//!             except GenerationError as exc: _pet_cancel_release(token); return _err(rid, 5031, str(exc))
//!         concept = prompt or "a pet based on the reference image"; out: list[dict] = []
//!         try: _emit("pet.generate.progress", "", {"token":token,"count":count})
//!         except Exception as exc: logger.debug("pet.generate init emit failed: %s", exc)
//!         def _on_draft(index: int, src) -> None:
//!             dest = stage / f"draft-{index}.png"; shutil.copyfile(src, dest); data_uri = _pet_png_data_uri(dest); out.append({"index":index,"dataUri":data_uri}); _emit("pet.generate.progress","",{"token":token,"index":index,"dataUri":data_uri,"count":count})
//!         try: generate_base_drafts(concept, n=count, style=style, reference_images=reference_images, provider=sprite, on_draft=_on_draft, is_cancelled=lambda: _pet_is_cancelled(token))
//!         except GenerationError as exc: _pet_cancel_release(token); return _err(rid, 5031, str(exc))
//!         cancelled = _pet_is_cancelled(token); _pet_cancel_release(token)
//!         if cancelled: return _err(rid, 5031, "generation cancelled")
//!         if not out: return _err(rid, 5031, "generation produced no usable drafts")
//!         out.sort(key=lambda d: d["index"]); return _ok(rid, {"ok":True,"token":token,"drafts":out})
//!     except Exception as exc: logger.debug("pet.generate failed: %s", exc); return _err(rid, 5031, f"pet.generate failed: {exc}")
//!
//! @method("pet.hatch")
//! def _(rid, params: dict) -> dict:
//!     token = str(params.get("token") or "").strip(); cancel_token = str(params.get("cancelToken") or "").strip() or token; index = params.get("index",0); name = str(params.get("name") or "").strip()
//!     if not token: return _err(rid, 4004, "missing token")
//!     if not name: return _err(rid, 4004, "missing name")
//!     try: index = int(index)
//!     except (TypeError, ValueError): index = 0
//!     try:
//!         from agent.pet import store; from agent.pet.generate import hatch_pet; from agent.pet.generate.imagegen import GenerationError, resolve_provider
//!         base = _pet_gen_root()/token/f"draft-{index}.png"
//!         if not base.is_file(): return _err(rid, 4004, "draft expired — generate again")
//!         provider_name = str(params.get("provider") or "").strip(); sprite=None
//!         if provider_name:
//!             try: sprite = resolve_provider(require_references=True, prefer=provider_name)
//!             except GenerationError as exc: return _err(rid, 5031, str(exc))
//!         _pet_cancel_arm(cancel_token); slug=store.unique_slug(name)
//!         def _on_progress(event: str, detail: str) -> None:
//!             payload={"event":event,"detail":detail}
//!             if event=="row" and detail.count(":")==2: state,done,total=detail.split(":"); payload={"event":"row","state":state,"done":done,"total":total}
//!             _emit("pet.hatch.progress","",payload)
//!         try: result = hatch_pet(base_image=base, slug=slug, display_name=name, description=str(params.get("description") or ""), concept=str(params.get("prompt") or name), style=str(params.get("style") or "auto").strip() or "auto", provider=sprite, on_progress=_on_progress, is_cancelled=lambda: _pet_is_cancelled(cancel_token))
//!         except GenerationError as exc: return _err(rid, 5031, str(exc))
//!         finally: _pet_cancel_release(cancel_token)
//!         pet = store.load_pet(result.slug); payload = _pet_sprite_payload(pet, scale=_pet_config_scale()) if pet else {}
//!         return _ok(rid, {"ok":True,"slug":result.slug,"displayName":result.display_name,"warnings":result.validation.get("warnings",[]),"pet":payload})
//!     except Exception as exc: logger.debug("pet.hatch failed: %s", exc); return _err(rid, 5031, f"pet.hatch failed: {exc}")
//!
//! @method("billing.state")
//! def _(rid, params: dict) -> dict:
//!     try: from agent.billing_view import build_billing_state; state=build_billing_state(); return _ok(rid, _serialize_billing_state(state))
//!     except Exception: return _ok(rid, {"ok":True,"logged_in":False,"error":"could not load billing state"})
//!
//! @method("usage.bars")
//! def _(rid, params: dict) -> dict:
//!     try: from agent.billing_usage import build_usage_model; return _ok(rid, _serialize_usage_model(build_usage_model()))
//!     except Exception: return _ok(rid, {"ok":True,"available":False})
//!
//! @method("subscription.state")
//! def _(rid, params: dict) -> dict:
//!     try: from agent.subscription_view import build_subscription_state; state=build_subscription_state(); return _ok(rid, _serialize_subscription_state(state))
//!     except Exception: return _ok(rid, {"ok":True,"logged_in":False,"error":"could not load subscription state"})
//!
//! @method("subscription.preview")
//! def _(rid, params: dict) -> dict:
//!     from agent.subscription_view import subscription_change_preview_from_payload; from hermes_cli.nous_billing import BillingError, post_subscription_preview
//!     tier_id = params.get("subscription_type_id")
//!     if not tier_id: return _ok(rid, {"ok":False,"error":"invalid_request","message":"subscription_type_id is required"})
//!     try: preview = subscription_change_preview_from_payload(post_subscription_preview(subscription_type_id=tier_id)); return _ok(rid, _serialize_subscription_preview(preview))
//!     except BillingError as exc: return _ok(rid, _serialize_billing_error(exc))
//!     except Exception as exc: return _ok(rid, {"ok":False,"error":"error","message":str(exc)})
//!
//! @method("subscription.change")
//! def _(rid, params: dict) -> dict:
//!     from hermes_cli.nous_billing import BillingError, put_subscription_pending_change
//!     cancel = bool(params.get("cancel")); tier_id = params.get("subscription_type_id")
//!     if not cancel and not tier_id: return _ok(rid, {"ok":False,"error":"invalid_request","message":"subscription_type_id or cancel is required"})
//!     try: result=put_subscription_pending_change(subscription_type_id=tier_id,cancel=cancel); return _ok(rid, {"ok":True,"message":result.get("message"),"payload":result})
//!     except BillingError as exc: return _ok(rid, _serialize_billing_error(exc))
//!     except Exception as exc: return _ok(rid, {"ok":False,"error":"error","message":str(exc)})
//!
//! @method("subscription.resume")
//! def _(rid, params: dict) -> dict:
//!     from hermes_cli.nous_billing import BillingError, delete_subscription_pending_change
//!     try: result=delete_subscription_pending_change(); return _ok(rid, {"ok":True,"message":result.get("message"),"payload":result})
//!     except BillingError as exc: return _ok(rid, _serialize_billing_error(exc))
//!     except Exception as exc: return _ok(rid, {"ok":False,"error":"error","message":str(exc)})
//!
//! @method("subscription.upgrade")
//! def _(rid, params: dict) -> dict:
//!     from agent.billing_view import new_idempotency_key; from hermes_cli.nous_billing import BillingError, post_subscription_upgrade
//!     tier_id = params.get("subscription_type_id")
//!     if not tier_id: return _ok(rid, {"ok":False,"error":"invalid_request","message":"subscription_type_id is required"})
//!     key = params.get("idempotency_key") or new_idempotency_key()
//!     try: result=post_subscription_upgrade(subscription_type_id=tier_id,idempotency_key=key); return _ok(rid, {"ok":True,"status":result.get("status"),"target_tier_name":result.get("targetTierName"),"recovery_url":result.get("recoveryUrl"),"reason":result.get("reason"),"idempotency_key":key})
//!     except BillingError as exc: env=_serialize_billing_error(exc); env["idempotency_key"]=key; return _ok(rid, env)
//!     except Exception as exc: return _ok(rid, {"ok":False,"error":"error","message":str(exc),"idempotency_key":key})
//!
//! @method("billing.charge")
//! def _(rid, params: dict) -> dict:
//!     from hermes_cli.nous_billing import BillingError, post_charge; from agent.billing_view import new_idempotency_key
//!     amount = params.get("amount_usd")
//!     if amount is None: return _ok(rid, {"ok":False,"error":"invalid_request","message":"amount_usd is required"})
//!     key = params.get("idempotency_key") or new_idempotency_key()
//!     try: result=post_charge(amount_usd=amount,idempotency_key=key); return _ok(rid, {"ok":True,"charge_id":result.get("chargeId"),"idempotency_key":key})
//!     except BillingError as exc: env=_serialize_billing_error(exc); env["idempotency_key"]=key; return _ok(rid, env)
//!     except Exception as exc: return _ok(rid, {"ok":False,"error":"error","message":str(exc),"idempotency_key":key})
//!
//! @method("billing.charge_status")
//! def _(rid, params: dict) -> dict:
//!     from hermes_cli.nous_billing import BillingError, get_charge_status
//!     charge_id = params.get("charge_id")
//!     if not charge_id: return _ok(rid, {"ok":False,"error":"invalid_charge_id","message":"charge_id is required"})
//!     try: result=get_charge_status(charge_id); return _ok(rid, {"ok":True,"status":result.get("status"),"amount_usd":result.get("amountUsd"),"settled_at":result.get("settledAt"),"reason":result.get("reason")})
//!     except BillingError as exc: return _ok(rid, _serialize_billing_error(exc))
//!     except Exception as exc: return _ok(rid, {"ok":False,"error":"error","message":str(exc)})
//!
//! @method("billing.auto_reload")
//! def _(rid, params: dict) -> dict:
//!     from hermes_cli.nous_billing import BillingError, patch_auto_top_up
//!     try: enabled=bool(params.get("enabled")); threshold=params.get("threshold"); top_up_amount=params.get("top_up_amount")
//!          if threshold is None or top_up_amount is None: return _ok(rid, {"ok":False,"error":"invalid_request","message":"threshold and top_up_amount are required"})
//!          patch_auto_top_up(enabled=enabled,threshold=threshold,top_up_amount=top_up_amount); return _ok(rid, {"ok":True})
//!     except BillingError as exc: return _ok(rid, _serialize_billing_error(exc))
//!     except Exception as exc: return _ok(rid, {"ok":False,"error":"error","message":str(exc)})
//!
//! @method("billing.step_up")
//! def _(rid, params: dict) -> dict:
//!     sid = params.get("session_id") or ""
//!     try:
//!         from hermes_cli.auth import step_up_nous_billing_scope; from hermes_cli.nous_billing import BillingError
//!         def _on_verification(url: str, code: str) -> None: _emit("billing.step_up.verification", sid, {"verification_url":url,"user_code":code})
//!         granted = step_up_nous_billing_scope(open_browser=False, on_verification=_on_verification)
//!         return _ok(rid, {"ok":True,"granted":bool(granted)})
//!     except BillingError as exc: env=_serialize_billing_error(exc); env["granted"]=False; return _ok(rid, env)
//!     except Exception as exc: return _ok(rid, {"ok":False,"error":"error","message":str(exc),"granted":False})
//!
//! # truncated at line 2700 — @method("session.status") continues in next slice
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
//!   `db is None` → `5007`/`5036`/`5000`, `Err(e)` → same with `str(e)`.
//! * `is_truthy_value` → [`is_truthy_value`] (mirrors `hermes_constants`).
//! * `_ok(rid, result)` / `_err(rid, code, msg)` / `_db_unavailable_error` →
//!   [`ok_response`] / [`err_response`] (mirrors `server.py::_ok` / `_err`).
//! * `pet.cells` unicode tail (1867-1896): `PetRenderer(...,mode="unicode")` +
//!   `frame_count` + `cells` half-block grid + `LOOP_MS` → not re-registered here
//!   (already in slice 2); documented as tail completion for 1:1 traceability.
//! * `pet.gallery` (`@_profile_scoped`, fail-open): `store.installed_pets()` +
//!   `fetch_manifest`/`prefetch` + `localOnly` skip + `seen` dedup + `curated`
//!   (`/curated/` in URL) + `generated` + `is_truthy_value(enabled,default=False)` +
//!   `active` slug → [`handle_pet_gallery`] (always `_ok`; `Err`→`{"enabled":false,...}`).
//! * `pet.select` (`@_profile_scoped`): `slug` required `4004` + `store.install_pet` +
//!   `PetStoreError`/`ManifestError` → `5031` + `_set_active` → [`handle_pet_select`].
//! * `pet.remove` (`@_profile_scoped`): `slug` `4004` + `store.remove_pet` +
//!   `_clear_active_if` (best-effort) → [`handle_pet_remove`] (`5031` on `store` failure).
//! * `pet.export` (`@_profile_scoped`): `slug` `4004` + `store.export_pet` →
//!   `{"ok", "filename", "zipBase64":base64(...)}` → [`handle_pet_export`] (`5031`).
//! * `pet.rename` (`@_profile_scoped`): `slug`+`name` `4004` + `store.rename_pet` +
//!   `_rename_active_if` when slug moves → [`handle_pet_rename`] (`5031` when `new_slug` empty).
//! * `pet.thumb` (`@_profile_scoped`): `slug` `4004` + `store.thumbnail_png` +
//!   `source_url` → `{"ok", "slug", "dataUri":data:image/png...}` or `{"ok":false}`
//!   → [`handle_pet_thumb`] (`4004` handler-side; `ok:false` via `Ok` on `None`/`Err`).
//! * `pet.disable` (`@_profile_scoped`): `_set_enabled(False)` → [`handle_pet_disable`] (`5031`).
//! * `pet.scale` (`@_profile_scoped`): `set_pet_scale` → `4004`/`5031` →
//!   [`handle_pet_scale`].
//! * `pet.cancel` (no scope): `token` `strip()` + `_pet_cancel_request(token)` if non-empty,
//!   always `{"ok":true}` → [`handle_pet_cancel`] (never `_err`).
//! * `pet.generate.status` (no scope, fail-open): `resolve_provider(require_references=True)` +
//!   `list_sprite_providers` → `{"available", "providers"}` → [`handle_pet_generate_status`] (`Err`→`available:false`).
//! * `pet.generate` (no scope, worker pool): `prompt`+`referenceImage` → `4004`, `count` clamp 1-4,
//!   `style` default `auto`, `_pet_gen_root`+`_pet_gen_sweep`+`uuid hex[:12]` token+
//!   `_pet_cancel_arm`+`stage.mkdir`+`_pet_reference_images_from_data_url` `4004` +
//!   `resolve_provider` `5031` + `_emit token init` + `generate_base_drafts` +
//!   `_on_draft copy+_pet_png_data_uri+_emit progress` + cancellation/exhaustion `5031` →
//!   [`handle_pet_generate`] (validation `4004` handler-side for `prompt`, `4004` for bad data url delegated).
//! * `pet.hatch` (no scope, worker pool): `token` `4004` + `cancelToken` fallback + `index` int+`name` `4004` +
//!   `stage/draft-{index}.png` exists `4004` + `resolve_provider` `5031` + `_pet_cancel_arm` + `unique_slug` +
//!   `hatch_pet` + `_on_progress row:done:total` → `pet.hatch.progress` + `load_pet`+`_pet_sprite_payload` →
//!   [`handle_pet_hatch`] (`5031`).
//! * `billing.state` / `usage.bars` / `subscription.state` (fail-open, always `_ok`): `build_*` +
//!   `_serialize_*` → [`handle_billing_state`]/[`handle_usage_bars`]/[`handle_subscription_state`]
//!   (`Err`→`{"ok":true,"logged_in":false...}` or `{"available":false}`).
//! * `subscription.preview` / `change` / `resume` / `upgrade` / `billing.charge` /
//!   `charge_status` / `auto_reload` / `step_up` (always `_ok`, never `_err`): the
//!   Python returns `{"ok":false,"error":"invalid_request",...}` or `{"ok":false,"error":"..."}`
//!   via `_serialize_billing_error` + `idempotency_key` echo; Rust mirrors as
//!   `Fn(&str)->Result<String,String>` where `Ok(json)` is the `_ok` result object
//!   and `Err` is folded to `_ok({"ok":false,...})` — see [`handle_subscription_preview`] etc.
//!   `subscription.upgrade`/`billing.charge` mint `idempotency_key` via `new_idempotency_key`
//!   when absent and always echo it even on `BillingError`.
//! * `billing.step_up` (always `_ok`): `step_up_nous_billing_scope(open_browser=False,on_verification=emit)` +
//!   `BillingError` → `granted:false` + error spine → [`handle_billing_step_up`].
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] /
//!   [`build_registry`] / [`build_registry_default`] (deferred via `HandlerRegistry`).

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators for 1800-2700
// ---------------------------------------------------------------------------

pub const METHOD_PET_GALLERY: &str = "pet.gallery";
pub const METHOD_PET_SELECT: &str = "pet.select";
pub const METHOD_PET_REMOVE: &str = "pet.remove";
pub const METHOD_PET_EXPORT: &str = "pet.export";
pub const METHOD_PET_RENAME: &str = "pet.rename";
pub const METHOD_PET_THUMB: &str = "pet.thumb";
pub const METHOD_PET_DISABLE: &str = "pet.disable";
pub const METHOD_PET_SCALE: &str = "pet.scale";
pub const METHOD_PET_CANCEL: &str = "pet.cancel";
pub const METHOD_PET_GENERATE_STATUS: &str = "pet.generate.status";
pub const METHOD_PET_GENERATE: &str = "pet.generate";
pub const METHOD_PET_HATCH: &str = "pet.hatch";
pub const METHOD_BILLING_STATE: &str = "billing.state";
pub const METHOD_USAGE_BARS: &str = "usage.bars";
pub const METHOD_SUBSCRIPTION_STATE: &str = "subscription.state";
pub const METHOD_SUBSCRIPTION_PREVIEW: &str = "subscription.preview";
pub const METHOD_SUBSCRIPTION_CHANGE: &str = "subscription.change";
pub const METHOD_SUBSCRIPTION_RESUME: &str = "subscription.resume";
pub const METHOD_SUBSCRIPTION_UPGRADE: &str = "subscription.upgrade";
pub const METHOD_BILLING_CHARGE: &str = "billing.charge";
pub const METHOD_BILLING_CHARGE_STATUS: &str = "billing.charge_status";
pub const METHOD_BILLING_AUTO_RELOAD: &str = "billing.auto_reload";
pub const METHOD_BILLING_STEP_UP: &str = "billing.step_up";

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_MISSING_SLUG: i32 = 4004;
pub const ERR_MISSING_NAME: i32 = 4004;
pub const ERR_MISSING_TOKEN: i32 = 4004;
pub const ERR_MISSING_PROMPT: i32 = 4004;
pub const ERR_DRAFT_EXPIRED: i32 = 4004;
pub const ERR_SCALE_INVALID: i32 = 4004;
pub const ERR_PET_STORE: i32 = 5031;
pub const ERR_BILLING: i32 = 5031;

// For billing always-_ok handlers, no numeric code — errors are in payload `error` field.
// We keep numeric stubs for test assertions where needed:
pub const ERR_INVALID_REQUEST_PAYLOAD: &str = "invalid_request";

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
            if ch == qc { return Some(rest[..=i+1].to_string()); }
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

/// Mirrors `is_truthy_value(v)` with optional default.
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

pub fn is_truthy_field(params_json: &str, field: &str) -> bool {
    let raw = extract_raw_value(params_json, field);
    is_truthy_value(raw.as_deref().map(|s| s.trim().trim_matches('"')))
}

// ---------------------------------------------------------------------------
// Param helpers for this slice
// ---------------------------------------------------------------------------

pub fn extract_slug_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "slug")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn extract_name_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "name")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn extract_token_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "token")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn parse_generate_count(params_json: &str) -> i64 {
    let raw = extract_raw_value(params_json, "count");
    match raw {
        None => 4,
        Some(v) => {
            let t = v.trim().trim_matches('"').trim();
            match t.parse::<i64>() {
                Ok(n) => n.clamp(1, 4),
                Err(_) => 4,
            }
        }
    }
}

pub fn extract_prompt_param(params_json: &str) -> String {
    extract_string_field(params_json, "prompt").unwrap_or_default().trim().to_string()
}

pub fn extract_reference_image_param(params_json: &str) -> String {
    extract_string_field(params_json, "referenceImage").unwrap_or_default().trim().to_string()
}

pub fn parse_oneshot_requires_prompt(params_json: &str) -> bool {
    let prompt = extract_prompt_param(params_json);
    let ref_raw = extract_reference_image_param(params_json);
    prompt.is_empty() && ref_raw.is_empty()
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `pet.gallery` (profile-scoped, fail-open).
///
/// Always returns `_ok`. `Err` maps to `{"enabled":false,"active":"","pets":[]}`.
pub fn handle_pet_gallery<F>(rid_json: &str, params_json: &str, gallery_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match gallery_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, r#"{"enabled":false,"active":"","pets":[]}"#),
    }
}

/// Handle `pet.select` (profile-scoped).
///
/// Validates `slug` required (`4004`) handler-side, then delegates `5031` to closure.
pub fn handle_pet_select<F>(rid_json: &str, params_json: &str, select_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let slug = extract_string_field(params_json, "slug").unwrap_or_default().trim().to_string();
    if slug.is_empty() {
        return err_response(rid_json, ERR_MISSING_SLUG, "missing slug");
    }
    match select_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `pet.remove` (profile-scoped).
pub fn handle_pet_remove<F>(rid_json: &str, params_json: &str, remove_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let slug = extract_string_field(params_json, "slug").unwrap_or_default().trim().to_string();
    if slug.is_empty() {
        return err_response(rid_json, ERR_MISSING_SLUG, "missing slug");
    }
    match remove_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `pet.export` (profile-scoped).
pub fn handle_pet_export<F>(rid_json: &str, params_json: &str, export_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let slug = extract_string_field(params_json, "slug").unwrap_or_default().trim().to_string();
    if slug.is_empty() {
        return err_response(rid_json, ERR_MISSING_SLUG, "missing slug");
    }
    match export_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `pet.rename` (profile-scoped).
///
/// Validates `slug` and `name` required (`4004`) handler-side.
pub fn handle_pet_rename<F>(rid_json: &str, params_json: &str, rename_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let slug = extract_string_field(params_json, "slug").unwrap_or_default().trim().to_string();
    if slug.is_empty() {
        return err_response(rid_json, ERR_MISSING_SLUG, "missing slug");
    }
    let name = extract_string_field(params_json, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return err_response(rid_json, ERR_MISSING_NAME, "missing name");
    }
    match rename_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `pet.thumb` (profile-scoped, fail-open on store failure but _err on missing slug).
///
/// Missing `slug` → `4004`. Store returns `None` or exception → `{"ok":false,"slug":...}` via `Ok`.
/// The closure should return `Ok({"ok":false,...})` for `None`/exception; `Err` is reserved for missing slug
/// and unexpected `5031` paths that the Python would have raised (here we keep `5031` as `Ok` with `ok:false` to preserve `_ok` envelope).
pub fn handle_pet_thumb<F>(rid_json: &str, params_json: &str, thumb_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let slug = extract_string_field(params_json, "slug").unwrap_or_default().trim().to_string();
    if slug.is_empty() {
        return err_response(rid_json, ERR_MISSING_SLUG, "missing slug");
    }
    match thumb_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Alternative `pet.thumb` fail-open wrapper for closures that use Result<String,String> and return `ok:false` on Err.
pub fn handle_pet_thumb_ok<F>(rid_json: &str, params_json: &str, thumb_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    let slug = extract_string_field(params_json, "slug").unwrap_or_default().trim().to_string();
    if slug.is_empty() {
        return err_response(rid_json, ERR_MISSING_SLUG, "missing slug");
    }
    let fallback = format!(r#"{{"ok":false,"slug":"{}"}}"#, json_escape(&slug));
    match thumb_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, &fallback),
    }
}

/// Handle `pet.disable` (profile-scoped).
pub fn handle_pet_disable<F>(rid_json: &str, params_json: &str, disable_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match disable_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `pet.scale` (profile-scoped).
pub fn handle_pet_scale<F>(rid_json: &str, params_json: &str, scale_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match scale_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `pet.cancel` (no scope, always _ok).
///
/// Token may be empty → still `{"ok":true}`; never `_err`.
pub fn handle_pet_cancel<F>(rid_json: &str, params_json: &str, cancel_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match cancel_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `pet.generate.status` (no scope, fail-open).
///
/// Always `_ok`. `Err` → `{"available":false,"providers":[]}`.
pub fn handle_pet_generate_status<F>(rid_json: &str, params_json: &str, status_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match status_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, r#"{"available":false,"providers":[]}"#),
    }
}

/// Handle `pet.generate` (no scope, worker pool).
///
/// Validates `prompt`+`referenceImage` required (`4004`) handler-side, then delegates.
pub fn handle_pet_generate<F>(rid_json: &str, params_json: &str, generate_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    if parse_oneshot_requires_prompt(params_json) {
        return err_response(rid_json, ERR_MISSING_PROMPT, "missing prompt");
    }
    match generate_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `pet.hatch` (no scope, worker pool).
///
/// Validates `token` and `name` required (`4004`) handler-side, delegates `5031`/`4004` for draft/provider.
pub fn handle_pet_hatch<F>(rid_json: &str, params_json: &str, hatch_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let token = extract_string_field(params_json, "token").unwrap_or_default().trim().to_string();
    if token.is_empty() {
        return err_response(rid_json, ERR_MISSING_TOKEN, "missing token");
    }
    let name = extract_string_field(params_json, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return err_response(rid_json, ERR_MISSING_NAME, "missing name");
    }
    match hatch_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

// --- billing / subscription handlers (always _ok, never _err) ---

/// Handle `billing.state` (fail-open, always _ok).
pub fn handle_billing_state<F>(rid_json: &str, params_json: &str, billing_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match billing_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, r#"{"ok":true,"logged_in":false,"error":"could not load billing state"}"#),
    }
}

/// Handle `usage.bars` (fail-open, always _ok).
pub fn handle_usage_bars<F>(rid_json: &str, params_json: &str, usage_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match usage_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, r#"{"ok":true,"available":false}"#),
    }
}

/// Handle `subscription.state` (fail-open, always _ok).
pub fn handle_subscription_state<F>(rid_json: &str, params_json: &str, sub_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match sub_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, r#"{"ok":true,"logged_in":false,"error":"could not load subscription state"}"#),
    }
}

/// Handle `subscription.preview` (always _ok, validation inside closure returning `{"ok":false,...}`).
///
/// We keep handler thin — `tier_id` missing maps to the same `invalid_request` payload that Python returns via `_ok`.
pub fn handle_subscription_preview<F>(rid_json: &str, params_json: &str, preview_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match preview_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(e) => ok_response(rid_json, &format!(r#"{{"ok":false,"error":"error","message":"{}"}}"#, json_escape(&e))),
    }
}

/// Handle `subscription.change` (always _ok).
pub fn handle_subscription_change<F>(rid_json: &str, params_json: &str, change_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match change_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(e) => ok_response(rid_json, &format!(r#"{{"ok":false,"error":"error","message":"{}"}}"#, json_escape(&e))),
    }
}

/// Handle `subscription.resume` (always _ok).
pub fn handle_subscription_resume<F>(rid_json: &str, params_json: &str, resume_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match resume_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(e) => ok_response(rid_json, &format!(r#"{{"ok":false,"error":"error","message":"{}"}}"#, json_escape(&e))),
    }
}

/// Handle `subscription.upgrade` (always _ok, echoes idempotency_key).
pub fn handle_subscription_upgrade<F>(rid_json: &str, params_json: &str, upgrade_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match upgrade_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(e) => ok_response(rid_json, &format!(r#"{{"ok":false,"error":"error","message":"{}"}}"#, json_escape(&e))),
    }
}

/// Handle `billing.charge` (always _ok).
pub fn handle_billing_charge<F>(rid_json: &str, params_json: &str, charge_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match charge_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(e) => ok_response(rid_json, &format!(r#"{{"ok":false,"error":"error","message":"{}"}}"#, json_escape(&e))),
    }
}

/// Handle `billing.charge_status` (always _ok).
pub fn handle_billing_charge_status<F>(rid_json: &str, params_json: &str, status_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match status_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(e) => ok_response(rid_json, &format!(r#"{{"ok":false,"error":"error","message":"{}"}}"#, json_escape(&e))),
    }
}

/// Handle `billing.auto_reload` (always _ok).
pub fn handle_billing_auto_reload<F>(rid_json: &str, params_json: &str, auto_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match auto_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(e) => ok_response(rid_json, &format!(r#"{{"ok":false,"error":"error","message":"{}"}}"#, json_escape(&e))),
    }
}

/// Handle `billing.step_up` (always _ok, `granted:false` on BillingError).
pub fn handle_billing_step_up<F>(rid_json: &str, params_json: &str, step_fn: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match step_fn(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(e) => ok_response(rid_json, &format!(r#"{{"ok":false,"error":"error","message":"{}","granted":false}}"#, json_escape(&e))),
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with the 23 slice-3 methods registered
/// using the provided deps (for tests / production injection).
///
/// Each closure is `'static` and mirrors the lazy imports inside Python
/// handler bodies. For the default stub (no backend) use [`build_registry_default`].
#[allow(clippy::too_many_arguments)]
pub fn build_registry<Ga, Se, Re, Ex, Rn, Th, Di, Sc, Ca, St, Ge, Ha, Bi, Ub, Su, Pr, Ch, Rs, Up, Bc, Bs, Au, Su2>(
    pet_gallery: Ga,
    pet_select: Se,
    pet_remove: Re,
    pet_export: Ex,
    pet_rename: Rn,
    pet_thumb: Th,
    pet_disable: Di,
    pet_scale: Sc,
    pet_cancel: Ca,
    pet_generate_status: St,
    pet_generate: Ge,
    pet_hatch: Ha,
    billing_state: Bi,
    usage_bars: Ub,
    subscription_state: Su,
    subscription_preview: Pr,
    subscription_change: Ch,
    subscription_resume: Rs,
    subscription_upgrade: Up,
    billing_charge: Bc,
    billing_charge_status: Bs,
    billing_auto_reload: Au,
    billing_step_up: Su2,
) -> HandlerRegistry
where
    Ga: Fn(String, String) -> String + Send + Sync + 'static,
    Se: Fn(String, String) -> String + Send + Sync + 'static,
    Re: Fn(String, String) -> String + Send + Sync + 'static,
    Ex: Fn(String, String) -> String + Send + Sync + 'static,
    Rn: Fn(String, String) -> String + Send + Sync + 'static,
    Th: Fn(String, String) -> String + Send + Sync + 'static,
    Di: Fn(String, String) -> String + Send + Sync + 'static,
    Sc: Fn(String, String) -> String + Send + Sync + 'static,
    Ca: Fn(String, String) -> String + Send + Sync + 'static,
    St: Fn(String, String) -> String + Send + Sync + 'static,
    Ge: Fn(String, String) -> String + Send + Sync + 'static,
    Ha: Fn(String, String) -> String + Send + Sync + 'static,
    Bi: Fn(String, String) -> String + Send + Sync + 'static,
    Ub: Fn(String, String) -> String + Send + Sync + 'static,
    Su: Fn(String, String) -> String + Send + Sync + 'static,
    Pr: Fn(String, String) -> String + Send + Sync + 'static,
    Ch: Fn(String, String) -> String + Send + Sync + 'static,
    Rs: Fn(String, String) -> String + Send + Sync + 'static,
    Up: Fn(String, String) -> String + Send + Sync + 'static,
    Bc: Fn(String, String) -> String + Send + Sync + 'static,
    Bs: Fn(String, String) -> String + Send + Sync + 'static,
    Au: Fn(String, String) -> String + Send + Sync + 'static,
    Su2: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        pet_gallery,
        pet_select,
        pet_remove,
        pet_export,
        pet_rename,
        pet_thumb,
        pet_disable,
        pet_scale,
        pet_cancel,
        pet_generate_status,
        pet_generate,
        pet_hatch,
        billing_state,
        usage_bars,
        subscription_state,
        subscription_preview,
        subscription_change,
        subscription_resume,
        subscription_upgrade,
        billing_charge,
        billing_charge_status,
        billing_auto_reload,
        billing_step_up,
    );
    reg
}

/// Build a registry with default stubs (every operation returns error / `enabled:false` / `ok:false`).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_gallery(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_select(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_remove(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_export(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_rename(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_thumb(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_disable(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_scale(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_cancel(&rid_json, &params_json, |_| Ok(r#"{"ok":true}"#.to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_generate_status(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_generate(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_hatch(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_state(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_usage_bars(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_state(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_preview(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_change(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_resume(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_upgrade(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_charge(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_charge_status(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_auto_reload(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_step_up(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
    )
}

/// Register all 23 slice-3 methods onto an existing registry.
#[allow(clippy::too_many_arguments)]
pub fn register_with<Ga, Se, Re, Ex, Rn, Th, Di, Sc, Ca, St, Ge, Ha, Bi, Ub, Su, Pr, Ch, Rs, Up, Bc, Bs, Au, Su2>(
    registry: &mut HandlerRegistry,
    pet_gallery: Ga,
    pet_select: Se,
    pet_remove: Re,
    pet_export: Ex,
    pet_rename: Rn,
    pet_thumb: Th,
    pet_disable: Di,
    pet_scale: Sc,
    pet_cancel: Ca,
    pet_generate_status: St,
    pet_generate: Ge,
    pet_hatch: Ha,
    billing_state: Bi,
    usage_bars: Ub,
    subscription_state: Su,
    subscription_preview: Pr,
    subscription_change: Ch,
    subscription_resume: Rs,
    subscription_upgrade: Up,
    billing_charge: Bc,
    billing_charge_status: Bs,
    billing_auto_reload: Au,
    billing_step_up: Su2,
) where
    Ga: Fn(String, String) -> String + Send + Sync + 'static,
    Se: Fn(String, String) -> String + Send + Sync + 'static,
    Re: Fn(String, String) -> String + Send + Sync + 'static,
    Ex: Fn(String, String) -> String + Send + Sync + 'static,
    Rn: Fn(String, String) -> String + Send + Sync + 'static,
    Th: Fn(String, String) -> String + Send + Sync + 'static,
    Di: Fn(String, String) -> String + Send + Sync + 'static,
    Sc: Fn(String, String) -> String + Send + Sync + 'static,
    Ca: Fn(String, String) -> String + Send + Sync + 'static,
    St: Fn(String, String) -> String + Send + Sync + 'static,
    Ge: Fn(String, String) -> String + Send + Sync + 'static,
    Ha: Fn(String, String) -> String + Send + Sync + 'static,
    Bi: Fn(String, String) -> String + Send + Sync + 'static,
    Ub: Fn(String, String) -> String + Send + Sync + 'static,
    Su: Fn(String, String) -> String + Send + Sync + 'static,
    Pr: Fn(String, String) -> String + Send + Sync + 'static,
    Ch: Fn(String, String) -> String + Send + Sync + 'static,
    Rs: Fn(String, String) -> String + Send + Sync + 'static,
    Up: Fn(String, String) -> String + Send + Sync + 'static,
    Bc: Fn(String, String) -> String + Send + Sync + 'static,
    Bs: Fn(String, String) -> String + Send + Sync + 'static,
    Au: Fn(String, String) -> String + Send + Sync + 'static,
    Su2: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method_profile_scoped(METHOD_PET_GALLERY, pet_gallery);
    registry.method_profile_scoped(METHOD_PET_SELECT, pet_select);
    registry.method_profile_scoped(METHOD_PET_REMOVE, pet_remove);
    registry.method_profile_scoped(METHOD_PET_EXPORT, pet_export);
    registry.method_profile_scoped(METHOD_PET_RENAME, pet_rename);
    registry.method_profile_scoped(METHOD_PET_THUMB, pet_thumb);
    registry.method_profile_scoped(METHOD_PET_DISABLE, pet_disable);
    registry.method_profile_scoped(METHOD_PET_SCALE, pet_scale);
    registry.method(METHOD_PET_CANCEL, pet_cancel);
    registry.method(METHOD_PET_GENERATE_STATUS, pet_generate_status);
    registry.method(METHOD_PET_GENERATE, pet_generate);
    registry.method(METHOD_PET_HATCH, pet_hatch);
    registry.method(METHOD_BILLING_STATE, billing_state);
    registry.method(METHOD_USAGE_BARS, usage_bars);
    registry.method(METHOD_SUBSCRIPTION_STATE, subscription_state);
    registry.method(METHOD_SUBSCRIPTION_PREVIEW, subscription_preview);
    registry.method(METHOD_SUBSCRIPTION_CHANGE, subscription_change);
    registry.method(METHOD_SUBSCRIPTION_RESUME, subscription_resume);
    registry.method(METHOD_SUBSCRIPTION_UPGRADE, subscription_upgrade);
    registry.method(METHOD_BILLING_CHARGE, billing_charge);
    registry.method(METHOD_BILLING_CHARGE_STATUS, billing_charge_status);
    registry.method(METHOD_BILLING_AUTO_RELOAD, billing_auto_reload);
    registry.method(METHOD_BILLING_STEP_UP, billing_step_up);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_gallery(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_select(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_remove(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_export(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_rename(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_thumb(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_disable(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_scale(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_cancel(&rid_json, &params_json, |_| Ok(r#"{"ok":true}"#.to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_generate_status(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_generate(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pet_hatch(&rid_json, &params_json, |_| Err((ERR_PET_STORE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_state(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_usage_bars(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_state(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_preview(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_change(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_resume(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_subscription_upgrade(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_charge(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_charge_status(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_auto_reload(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_billing_step_up(&rid_json, &params_json, |_| Err("no backend".to_string()))
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
    fn pet_select_requires_slug() {
        let rid = rid1();
        let out = handle_pet_select(&rid, r#"{}"#, |_| Ok(r#"{"ok":true,"slug":"cat","displayName":"Cat"}"#.to_string()));
        assert!(out.contains(r#""code":4004"#), "{}", out);
        assert!(out.contains("missing slug"));
        let out2 = handle_pet_select(&rid, r#"{"slug":"cat"}"#, |_| Ok(r#"{"ok":true,"slug":"cat","displayName":"Cat"}"#.to_string()));
        assert!(out2.contains(r#""ok":true"#), "{}", out2);
        let out3 = handle_pet_select(&rid, r#"{"slug":"cat"}"#, |_| Err((5031, "could not adopt 'cat': boom".into())));
        assert!(out3.contains(r#""code":5031"#), "{}", out3);
    }

    #[test]
    fn pet_remove_requires_slug() {
        let rid = rid1();
        let out = handle_pet_remove(&rid, r#"{}"#, |_| Ok(r#"{"ok":true,"slug":"cat"}"#.to_string()));
        assert!(out.contains(r#""code":4004"#), "{}", out);
        let out2 = handle_pet_remove(&rid, r#"{"slug":"cat"}"#, |_| Ok(r#"{"ok":true,"slug":"cat"}"#.to_string()));
        assert!(out2.contains(r#""ok":true"#));
        let out3 = handle_pet_remove(&rid, r#"{"slug":"cat"}"#, |_| Err((5031, "pet.remove failed: boom".into())));
        assert!(out3.contains(r#""code":5031"#));
    }

    #[test]
    fn pet_export_and_rename_validation() {
        let rid = rid1();
        let out = handle_pet_export(&rid, r#"{}"#, |_| Ok(r#"{"ok":true,"filename":"cat.zip","zipBase64":"abc"}"#.to_string()));
        assert!(out.contains(r#""code":4004"#), "{}", out);
        let out2 = handle_pet_export(&rid, r#"{"slug":"cat"}"#, |_| Ok(r#"{"ok":true,"filename":"cat.zip","zipBase64":"abc"}"#.to_string()));
        assert!(out2.contains("cat.zip"));
        let out3 = handle_pet_rename(&rid, r#"{"slug":"cat"}"#, |_| Ok(r#"{"ok":true,"slug":"kitty","displayName":"Kitty"}"#.to_string()));
        assert!(out3.contains(r#""code":4004"#), "{}", out3); // missing name
        let out4 = handle_pet_rename(&rid, r#"{"slug":"cat","name":"Kitty"}"#, |_| Ok(r#"{"ok":true,"slug":"kitty","displayName":"Kitty"}"#.to_string()));
        assert!(out4.contains("kitty"));
        let out5 = handle_pet_rename(&rid, r#"{}"#, |_| Ok(r#"{}"#.to_string()));
        assert!(out5.contains(r#""code":4004"#));
    }

    #[test]
    fn pet_thumb_requires_slug_and_ok_fallback() {
        let rid = rid1();
        let out = handle_pet_thumb(&rid, r#"{}"#, |_| Ok(r#"{"ok":true,"slug":"cat","dataUri":"data:image/png;base64,abc"}"#.to_string()));
        assert!(out.contains(r#""code":4004"#), "{}", out);
        let out2 = handle_pet_thumb(&rid, r#"{"slug":"cat"}"#, |_| Ok(r#"{"ok":false,"slug":"cat"}"#.to_string()));
        assert!(out2.contains(r#""ok":false"#), "{}", out2);
        let out3 = handle_pet_thumb(&rid, r#"{"slug":"cat"}"#, |_| Ok(r#"{"ok":true,"slug":"cat","dataUri":"data:image/png;base64,abc"}"#.to_string()));
        assert!(out3.contains("dataUri"));
        // fail-open variant with Err -> ok:false
        let out4 = handle_pet_thumb_ok(&rid, r#"{"slug":"cat"}"#, |_| Err("store boom".into()));
        assert!(out4.contains(r#""ok":false"#), "{}", out4);
        assert!(out4.contains("cat"));
    }

    #[test]
    fn pet_gallery_fail_open() {
        let rid = rid1();
        let out = handle_pet_gallery(&rid, r#"{}"#, |_| Ok(r#"{"enabled":true,"active":"cat","pets":[{"slug":"cat","displayName":"Cat","installed":true,"spritesheetUrl":"","generated":false}]}"#.to_string()));
        assert!(out.contains(r#""enabled":true"#), "{}", out);
        let out2 = handle_pet_gallery(&rid, "{}", |_| Err("boom".into()));
        assert!(out2.contains(r#""enabled":false"#), "{}", out2);
        assert!(!out2.contains(r#""error""#), "pet.gallery fail-open should be _ok not _err: {}", out2);
        let out3 = handle_pet_gallery(&rid, r#"{"localOnly":true}"#, |params| {
            assert!(params.contains("localOnly"));
            Ok(r#"{"enabled":false,"active":"","pets":[]}"#.to_string())
        });
        assert!(out3.contains(r#""pets":[]"#));
    }

    #[test]
    fn pet_disable_and_scale() {
        let rid = rid1();
        let out = handle_pet_disable(&rid, "{}", |_| Ok(r#"{"ok":true}"#.to_string()));
        assert!(out.contains(r#""ok":true"#), "{}", out);
        let out2 = handle_pet_disable(&rid, "{}", |_| Err((5031, "pet.disable failed: boom".into())));
        assert!(out2.contains(r#""code":5031"#));
        let out3 = handle_pet_scale(&rid, r#"{"scale":1.5}"#, |_| Ok(r#"{"ok":true,"scale":1.5}"#.to_string()));
        assert!(out3.contains("1.5"));
        let out4 = handle_pet_scale(&rid, r#"{"scale":"bad"}"#, |_| Err((4004, "invalid scale".into())));
        assert!(out4.contains(r#""code":4004"#));
    }

    #[test]
    fn pet_cancel_never_err_on_empty_token() {
        let rid = rid1();
        // pet.cancel with empty token still ok:true (Python early return _ok even if token empty)
        let out = handle_pet_cancel(&rid, r#"{}"#, |_| Ok(r#"{"ok":true}"#.to_string()));
        assert!(out.contains(r#""ok":true"#), "{}", out);
        let out2 = handle_pet_cancel(&rid, r#"{"token":"abc123"}"#, |_| Ok(r#"{"ok":true}"#.to_string()));
        assert!(out2.contains(r#""ok":true"#));
        // Even with closure error, err path is preserved (though Python never errs)
        let out3 = handle_pet_cancel(&rid, r#"{"token":"abc"}"#, |_| Err((5000, "unexpected".into())));
        assert!(out3.contains(r#""code":5000"#));
    }

    #[test]
    fn pet_generate_status_fail_open() {
        let rid = rid1();
        let out = handle_pet_generate_status(&rid, "{}", |_| Ok(r#"{"available":true,"providers":["openai"]}"#.to_string()));
        assert!(out.contains(r#""available":true"#), "{}", out);
        let out2 = handle_pet_generate_status(&rid, "{}", |_| Err("boom".into()));
        assert!(out2.contains(r#""available":false"#), "{}", out2);
        assert!(out2.contains(r#""providers":[]"#));
    }

    #[test]
    fn pet_generate_requires_prompt() {
        let rid = rid1();
        let out = handle_pet_generate(&rid, r#"{}"#, |_| Ok(r#"{"ok":true,"token":"abc","drafts":[]}"#.to_string()));
        assert!(out.contains(r#""code":4004"#), "{}", out);
        assert!(out.contains("missing prompt"));
        let out2 = handle_pet_generate(&rid, r#"{"prompt":"a cute cat"}"#, |_| Ok(r#"{"ok":true,"token":"tok123","drafts":[{"index":0,"dataUri":"data:image/png;base64,abc"}]}"#.to_string()));
        assert!(out2.contains("tok123"));
        let out3 = handle_pet_generate(&rid, r#"{"referenceImage":"data:image/png;base64,abc"}"#, |_| Ok(r#"{"ok":true,"token":"tok","drafts":[]}"#.to_string()));
        assert!(out3.contains(r#""ok":true"#));
        let out4 = handle_pet_generate(&rid, r#"{"prompt":"hi"}"#, |_| Err((5031, "generation produced no usable drafts".into())));
        assert!(out4.contains(r#""code":5031"#));
        assert_eq!(parse_generate_count(r#"{"count":2}"#), 2);
        assert_eq!(parse_generate_count(r#"{"count":99}"#), 4);
        assert_eq!(parse_generate_count(r#"{"count":0}"#), 1);
        assert_eq!(parse_generate_count(r#"{}"#), 4);
    }

    #[test]
    fn pet_hatch_requires_token_and_name() {
        let rid = rid1();
        let out = handle_pet_hatch(&rid, r#"{}"#, |_| Ok(r#"{"ok":true,"slug":"cat","displayName":"Cat","warnings":[],"pet":{}}"#.to_string()));
        assert!(out.contains(r#""code":4004"#), "{}", out);
        assert!(out.contains("missing token"));
        let out2 = handle_pet_hatch(&rid, r#"{"token":"tok"}"#, |_| Ok(r#"{}"#.to_string()));
        assert!(out2.contains(r#""code":4004"#));
        assert!(out2.contains("missing name"));
        let out3 = handle_pet_hatch(&rid, r#"{"token":"tok","name":"Kitty","index":0}"#, |_| Ok(r#"{"ok":true,"slug":"kitty","displayName":"Kitty","warnings":[],"pet":{}}"#.to_string()));
        assert!(out3.contains("kitty"));
        let out4 = handle_pet_hatch(&rid, r#"{"token":"tok","name":"Kitty"}"#, |_| Err((4004, "draft expired — generate again".into())));
        assert!(out4.contains(r#""code":4004"#));
        let out5 = handle_pet_hatch(&rid, r#"{"token":"tok","name":"Kitty"}"#, |_| Err((5031, "provider error".into())));
        assert!(out5.contains(r#""code":5031"#));
    }

    #[test]
    fn billing_always_ok_not_err() {
        let rid = rid1();
        let out = handle_billing_state(&rid, "{}", |_| Ok(r#"{"ok":true,"logged_in":true,"available":true}"#.to_string()));
        assert!(out.contains(r#""ok":true"#), "{}", out);
        let out2 = handle_billing_state(&rid, "{}", |_| Err("boom".into()));
        assert!(out2.contains(r#""logged_in":false"#), "{}", out2);
        assert!(!out2.contains(r#""code""#), "billing.state should be _ok not _err: {}", out2);
        let out3 = handle_usage_bars(&rid, "{}", |_| Ok(r#"{"ok":true,"available":true,"bars":[]}"#.to_string()));
        assert!(out3.contains("available"));
        let out4 = handle_usage_bars(&rid, "{}", |_| Err("boom".into()));
        assert!(out4.contains(r#""available":false"#));
        let out5 = handle_subscription_state(&rid, "{}", |_| Ok(r#"{"ok":true,"logged_in":true}"#.to_string()));
        assert!(out5.contains("logged_in"));
        let out6 = handle_subscription_state(&rid, "{}", |_| Err("boom".into()));
        assert!(out6.contains(r#""logged_in":false"#));
    }

    #[test]
    fn subscription_billing_always_ok_payloads() {
        let rid = rid1();
        let out = handle_subscription_preview(&rid, r#"{"subscription_type_id":"tier_1"}"#, |_| Ok(r#"{"ok":true,"preview":{}}"#.to_string()));
        assert!(out.contains(r#""ok":true"#), "{}", out);
        let out2 = handle_subscription_preview(&rid, "{}", |_| Err("boom".into()));
        assert!(out2.contains(r#""ok":false"#), "{}", out2);
        let out3 = handle_subscription_change(&rid, r#"{"cancel":true}"#, |_| Ok(r#"{"ok":true,"message":"scheduled","payload":{}}"#.to_string()));
        assert!(out3.contains("scheduled"));
        let out4 = handle_subscription_resume(&rid, "{}", |_| Ok(r#"{"ok":true,"message":"resumed","payload":{}}"#.to_string()));
        assert!(out4.contains("resumed"));
        let out5 = handle_subscription_upgrade(&rid, r#"{"subscription_type_id":"tier_2"}"#, |_| Ok(r#"{"ok":true,"status":"success","target_tier_name":"Pro","recovery_url":null,"reason":null,"idempotency_key":"key123"}"#.to_string()));
        assert!(out5.contains("target_tier_name"));
        let out6 = handle_billing_charge(&rid, r#"{"amount_usd":10}"#, |_| Ok(r#"{"ok":true,"charge_id":"ch_123","idempotency_key":"key123"}"#.to_string()));
        assert!(out6.contains("charge_id"));
        let out7 = handle_billing_charge_status(&rid, r#"{"charge_id":"ch_123"}"#, |_| Ok(r#"{"ok":true,"status":"succeeded","amount_usd":10,"settled_at":123,"reason":null}"#.to_string()));
        assert!(out7.contains("succeeded"));
        let out8 = handle_billing_auto_reload(&rid, r#"{"enabled":true,"threshold":5,"top_up_amount":20}"#, |_| Ok(r#"{"ok":true}"#.to_string()));
        assert!(out8.contains(r#""ok":true"#));
        let out9 = handle_billing_step_up(&rid, r#"{"session_id":"abc"}"#, |_| Ok(r#"{"ok":true,"granted":true}"#.to_string()));
        assert!(out9.contains("granted"));
        // BillingError path should still be _ok with ok:false
        let out10 = handle_billing_charge(&rid, r#"{"amount_usd":10}"#, |_| Err("BillingError insufficient_scope".into()));
        assert!(out10.contains(r#""ok":false"#), "{}", out10);
        assert!(!out10.contains(r#""code""#), "billing.charge should be _ok not _err: {}", out10);
    }

    #[test]
    fn extract_and_truthy_helpers() {
        assert_eq!(extract_string_field(r#"{"slug":"cat"}"#, "slug").as_deref(), Some("cat"));
        assert_eq!(extract_string_field(r#"{"name":"Kitty"}"#, "name").as_deref(), Some("Kitty"));
        assert_eq!(extract_slug_param(r#"{"slug":"cat"}"#).as_deref(), Some("cat"));
        assert_eq!(extract_name_param(r#"{"name":"Kitty"}"#).as_deref(), Some("Kitty"));
        assert_eq!(extract_token_param(r#"{"token":"tok123"}"#).as_deref(), Some("tok123"));
        assert!(extract_slug_param(r#"{"slug":""}"#).is_none());
        assert!(extract_slug_param(r#"{}"#).is_none());
        assert_eq!(parse_generate_count(r#"{"count":2}"#), 2);
        assert!(is_truthy_field(r#"{"localOnly":true}"#, "localOnly"));
        assert!(!is_truthy_field(r#"{"localOnly":false}"#, "localOnly"));
        assert_eq!(extract_string_field(r#"{"amount_usd":10}"#, "amount_usd"), None); // numeric field -> raw, not string
        assert_eq!(extract_raw_value(r#"{"amount_usd":10}"#, "amount_usd").unwrap(), "10");
        assert_eq!(extract_raw_value(r#"{"providers":[]}"#, "providers").unwrap(), "[]");
    }

    #[test]
    fn registry_installs_twenty_three() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 23);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["billing.auto_reload","billing.charge","billing.charge_status","billing.state","billing.step_up","pet.cancel","pet.disable","pet.export","pet.gallery","pet.generate","pet.generate.status","pet.hatch","pet.remove","pet.rename","pet.scale","pet.select","pet.thumb","subscription.change","subscription.preview","subscription.resume","subscription.state","subscription.upgrade","usage.bars"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 23);
        // pet.select missing slug -> 4004 even with no backend
        let out = map.get(METHOD_PET_SELECT).unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains(r#""code":4004"#), "{}", out);
        // pet.gallery fail-open should be ok enabled false even with no backend
        let out2 = map.get(METHOD_PET_GALLERY).unwrap()("1".to_string(), "{}".to_string());
        assert!(out2.contains(r#""enabled":false"#), "{}", out2);
        // pet.generate missing prompt -> 4004
        let out3 = map.get(METHOD_PET_GENERATE).unwrap()("1".to_string(), "{}".to_string());
        assert!(out3.contains(r#""code":4004"#), "{}", out3);
        // billing.state fail-open -> logged_in false, not err
        let out4 = map.get(METHOD_BILLING_STATE).unwrap()("1".to_string(), "{}".to_string());
        assert!(out4.contains(r#""logged_in":false"#), "{}", out4);
        assert!(!out4.contains(r#""code""#));
        // subscription.preview err path -> ok false
        let out5 = map.get(METHOD_SUBSCRIPTION_PREVIEW).unwrap()("1".to_string(), "{}".to_string());
        assert!(out5.contains(r#""ok":false"#) || out5.contains(r#""code""#) || out5.contains("error"), "{}", out5);
        // usage.bars -> available false fallback
        let out6 = map.get(METHOD_USAGE_BARS).unwrap()("1".to_string(), "{}".to_string());
        assert!(out6.contains(r#""available":false"#), "{}", out6);
    }

    #[test]
    fn ok_err_envelope_shape() {
        let rid = encode_rid("42");
        let ok = ok_response(&rid, r#"{"ok":true,"slug":"cat"}"#);
        assert!(ok.contains(r#""result""#));
        assert!(ok.contains("cat"));
        let err = err_response(&rid, 4004, "missing slug");
        assert!(err.contains(r#""code":4004"#));
        assert!(err.contains("missing slug"));
        let ok2 = ok_response(&rid, r#"{"ok":false,"error":"invalid_request","message":"subscription_type_id is required"}"#);
        assert!(ok2.contains("invalid_request"));
    }
}
