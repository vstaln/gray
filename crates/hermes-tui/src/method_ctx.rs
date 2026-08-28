//! Seam for the `server.py` `@method` handler split (mechanical move).
//!
//! 1:1 port of `tui_gateway/method_ctx.py` (53 lines).
//!
//! Python's `server.py` (~130 JSON-RPC handlers) closes over module globals
//! (`_sessions`, `_ok`, `_err`, config helpers, …). To move handlers out of
//! the 19K-line module without rewriting bodies, each `methods_*` module
//! defines handlers under a local [`HandlerRegistry`] and `server.py` calls
//! [`HandlerRegistry::install`] at the end of its import — once every global
//! the handlers close over exists. `install()` rebinds each handler's
//! `__globals__` to `server.py`'s namespace with `types.FunctionType`, so
//! bodies stay byte-identical and `global X` inside handlers keeps mutating
//! `server.py` state.
//!
//! No import cycle: `methods_*` modules never import server at module level
//! — server imports them and passes itself to `register()`.
//!
//! # Rust mapping
//!
//! * `types.FunctionType` rebinding (`fn.__code__, g, __defaults__,
//!   `__closure__`, `__kwdefaults__`, `__doc__`, `__dict__`) is a Python-only
//!   dynamic-globals trick. In Rust closures capture lexically, so rebinding
//!   is a **no-op** — `install` simply moves pending handlers into the
//!   server's method map.
//! * `fn._hermes_profile_scoped = True` cannot be attached to a Rust `fn`;
//!   instead the flag is stored per pending entry (`profile_scoped: bool`)
//!   and checked at `install` time. Call [`HandlerRegistry::method`] for
//!   plain handlers and [`HandlerRegistry::method_profile_scoped`] for
//!   profile-scoped ones (mirrors `@method` + `@_profile_scoped`).
//! * `server._methods: dict[str, callable]` → `HashMap<String, Handler>`
//! * `server._profile_scoped(handler)` → `wrap_profile_scoped: Fn(Handler) -> Handler`
//!   passed to [`HandlerRegistry::install`].
//!
//! ```python
//! # Python — tui_gateway/method_ctx.py
//! class HandlerRegistry:
//!     def __init__(self): self._pending = []
//!     def method(self, name): ...
//!     def profile_scoped(self, fn): fn._hermes_profile_scoped = True; return fn
//!     def install(self, server):
//!         g = vars(server)
//!         for name, fn in self._pending:
//!             real = types.FunctionType(fn.__code__, g, ...)
//!             if getattr(fn, "_hermes_profile_scoped", False):
//!                 real = server._profile_scoped(real)
//!             server._methods[name] = real
//! ```

use std::collections::HashMap;

/// JSON-RPC handler: `(request_id, params_json) -> response_json`.
///
/// Mirrors Python's `def _(rid, params: dict) -> dict:` but uses `String`
/// for both sides to stay `std`-only. A real server can parse `params_json`
/// as `serde_json::Value` inside the closure.
pub type Handler = Box<dyn Fn(String, String) -> String + Send + Sync + 'static>;

struct Pending {
    name: String,
    handler: Handler,
    profile_scoped: bool,
}

/// Deferred `@method` registrar used by the `methods_*` split modules.
///
/// Mirrors `tui_gateway/method_ctx.py::HandlerRegistry`.
pub struct HandlerRegistry {
    pending: Vec<Pending>,
}

impl Default for HandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HandlerRegistry {
    /// Create an empty registry. Mirrors `HandlerRegistry.__init__`.
    pub fn new() -> Self {
        Self { pending: Vec::new() }
    }

    /// Number of deferred handlers.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// True if no handlers are pending.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Iterator over pending method names (in registration order).
    pub fn pending_names(&self) -> impl Iterator<Item = &str> {
        self.pending.iter().map(|p| p.name.as_str())
    }

    /// Drop-in for `server.py`'s `@method` decorator (defers registration).
    ///
    /// Mirrors `HandlerRegistry.method(name)` — appends `(name, fn)` to
    /// `_pending` and returns `fn` (here the insert is the return).
    pub fn method<F>(&mut self, name: impl Into<String>, handler: F)
    where
        F: Fn(String, String) -> String + Send + Sync + 'static,
    {
        self.pending.push(Pending {
            name: name.into(),
            handler: Box::new(handler),
            profile_scoped: false,
        });
    }

    /// Register a handler that must be wrapped with `server._profile_scoped`
    /// at install time. Mirrors `@method` + `@_profile_scoped` stacking.
    ///
    /// Python stacks as:
    /// ```python
    /// @method("foo")
    /// @_profile_scoped
    /// def _(rid, params): ...
    /// ```
    /// where `_profile_scoped` sets `_hermes_profile_scoped = True` and
    /// `install` checks the flag. Rust cannot attach attributes to `fn`, so
    /// the flag lives in `Pending.profile_scoped` instead.
    pub fn method_profile_scoped<F>(&mut self, name: impl Into<String>, handler: F)
    where
        F: Fn(String, String) -> String + Send + Sync + 'static,
    {
        self.pending.push(Pending {
            name: name.into(),
            handler: Box::new(handler),
            profile_scoped: true,
        });
    }

    /// Drop-in for `server.py`'s `@_profile_scoped` (applied at install).
    ///
    /// Python: `fn._hermes_profile_scoped = True; return fn`.
    /// In Rust this helper is **not** used as a decorator — prefer
    /// [`Self::method_profile_scoped`]. It is provided for 1:1 completeness
    /// and simply returns the handler unchanged; the caller must register it
    /// via `method_profile_scoped` to actually mark it. The standalone marker
    /// exists so code mechanically translated from Python still type-checks
    /// if it does `let h = reg.profile_scoped(h); reg.method("foo", h)`.
    pub fn profile_scoped<F>(&self, handler: F) -> F
    where
        F: Fn(String, String) -> String + Send + Sync + 'static,
    {
        // flag is not attached to `F` itself; caller should use
        // `method_profile_scoped` if profile-scoping is needed.
        handler
    }

    /// Rebind pending handlers onto `server`'s globals and register them.
    ///
    /// Mirrors `HandlerRegistry.install(server)`:
    /// ```python
    /// g = vars(server)
    /// for name, fn in self._pending:
    ///     real = types.FunctionType(fn.__code__, g, fn.__name__, fn.__defaults__, fn.__closure__)
    ///     real.__kwdefaults__ = fn.__kwdefaults__
    ///     real.__doc__ = fn.__doc__
    ///     real.__dict__.update(fn.__dict__)
    ///     if getattr(fn, "_hermes_profile_scoped", False):
    ///         real = server._profile_scoped(real)
    ///     server._methods[name] = real
    /// ```
    /// Rust: the `FunctionType` rebinding and `__kwdefaults__/__doc__/__dict__`
    /// copies are no-ops. Only the `profile_scoped` wrapping and map insert
    /// remain. `wrap_profile_scoped` mirrors `server._profile_scoped`.
    pub fn install<F>(self, methods: &mut HashMap<String, Handler>, wrap_profile_scoped: F)
    where
        F: Fn(Handler) -> Handler,
    {
        for p in self.pending {
            let handler = if p.profile_scoped {
                wrap_profile_scoped(p.handler)
            } else {
                p.handler
            };
            methods.insert(p.name, handler);
        }
    }

    /// Convenience `install` where profile-scoped wrapping is identity.
    ///
    /// Mirrors installing onto a server that has no `_profile_scoped` (or
    /// where the test does not need home-override semantics).
    pub fn install_into(self, methods: &mut HashMap<String, Handler>) {
        self.install(methods, |h| h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_and_install() {
        let mut reg = HandlerRegistry::new();
        reg.method("a", |_rid, _params| "ok-a".to_string());
        reg.method_profile_scoped("b", |_rid, _params| "ok-b".to_string());
        assert_eq!(reg.len(), 2);
        let mut map: HashMap<String, Handler> = HashMap::new();
        let mut wrapped = Vec::new();
        reg.install(&mut map, |h| {
            // simulate server._profile_scoped wrapping
            let inner = h("x".to_string(), "{}".to_string());
            assert_eq!(inner, "ok-b");
            wrapped.push("b".to_string());
            Box::new(move |rid, params| format!("wrapped:{}", h(rid, params)))
        });
        assert_eq!(map.len(), 2);
        assert!(wrapped.contains(&"b".to_string()));
        let a = map.get("a").unwrap()("1".to_string(), "{}".to_string());
        assert_eq!(a, "ok-a");
        let b = map.get("b").unwrap()("1".to_string(), "{}".to_string());
        assert_eq!(b, "wrapped:ok-b");
    }

    #[test]
    fn empty_registry_install_is_noop() {
        let reg = HandlerRegistry::new();
        assert!(reg.is_empty());
        let mut map: HashMap<String, Handler> = HashMap::new();
        reg.install_into(&mut map);
        assert!(map.is_empty());
    }
}
