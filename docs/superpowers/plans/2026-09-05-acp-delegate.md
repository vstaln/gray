# /acp delegate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `/acp <agent> <prompt>` in the gray REPL — drive external coding agents (codex, gemini) over the Agent Client Protocol and get their final text back. Launch-demo ready.

**Architecture:** New `gray-acp` crate wrapping the official `agent-client-protocol` SDK (Apache-2.0, crates.io 2.1.0 — dependency, not vendored). REPL owns the `/acp` command; adapters declared in `gray.yml`. Read-only permission default.

**Tech Stack:** Rust edition 2024, tokio (in tree), `agent-client-protocol = "2.1"` + `agent-client-protocol-tokio` for stdio transport.

**Spec:** ACP v1 flow — `initialize` → `session/new` → `session/prompt` → `session/update` stream → stop reason. Permission requests denied by default (read-only posture).

## Global Constraints

- No new deps except `agent-client-protocol` + `agent-client-protocol-tokio`. Everything else from the workspace.
- Adapters are subprocesses (never linked): `codex: npx -y @agentclientprotocol/codex-acp@latest`, `gemini: gemini --experimental-acp`. `NAME=value` env passthrough (keys, CODEX_PATH) per adapter.
- Default deny on agent permission requests; never block the REPL (timeout + cancel).
- `cargo test --workspace --quiet` + `cargo clippy --workspace --quiet` green, zero new warnings.
- Branch `feat/acp-delegate`, never `main`. No pushes/tags without approval.

## Shared contract (Tasks 1+2 — exact, do not renegotiate)

```rust
pub struct AcpConfig { pub adapters: HashMap<String, AcpAdapter> }
pub struct AcpAdapter { pub command: String, pub args: Vec<String>, pub env: HashMap<String, String> }
pub enum AcpPermission { ReadOnly } // default; deny all tool/permission requests from the worker
pub struct AcpClient; // connects on construction
impl AcpClient {
    pub async fn spawn(adapter: &AcpAdapter, cwd: &PathBuf) -> anyhow::Result<Self>;
    pub async fn prompt(&self, text: &str) -> anyhow::Result<String>; // final text or error
    pub async fn cancel(&self) -> anyhow::Result<()>;
}
```

`gray.yml` shape:

```yaml
acp:
  codex: { command: "npx", args: ["-y", "@agentclientprotocol/codex-acp@latest"] }
  gemini: { command: "gemini", args: ["--experimental-acp"] }
```

---

### Task 1: gray-acp crate — config, client, deny-default, tests

**Files:**
- Create: `crates/gray-acp/` (`Cargo.toml`, `src/lib.rs`, `src/config.rs`, `src/client.rs`)
- Create: `crates/gray-acp/testdata/fake_acp.sh` (minimal ACP server: answers initialize/session/new, echoes prompt as one update + end_turn; used by tests, POSIX sh + printf)
- Modify: workspace `Cargo.toml` members + `[workspace.dependencies]`

**Interfaces:**
- Consumes: shared contract above.
- Produces: `AcpClient` per contract; `acp:` gray.yml parsing; fake_acp.sh for tests.

- [ ] **Step 1: Config parse (failing test first)**

Test: parse the `acp:` YAML sample into `AcpConfig` (serde_yaml_ng is in the tree — reuse it). Unknown adapter → error naming available ones.

- [ ] **Step 2: Client spawn→prompt→text (failing test first)**

Against `testdata/fake_acp.sh`: spawn, initialize, session/new, prompt returns the echoed text. Permission request from the fake → denied (assert the fake observes a deny).

- [ ] **Step 3: Cancel + timeout (failing test first)**

`cancel()` mid-prompt aborts; a prompt exceeding the timeout errors instead of hanging the caller forever.

- [ ] **Step 4: Verify + commit**

Run: `cargo test -p gray-acp --quiet && cargo clippy -p gray-acp --quiet`
```bash
git add crates/gray-acp/ Cargo.toml Cargo.lock
git commit -m "feat(acp): gray-acp crate — spawn, prompt, deny-default permissions"
```

---

### Task 2: REPL /acp command wiring

**Files:**
- Modify: `crates/gray/src/repl/` (dispatch `/acp <agent> <prompt…>`: unknown agent lists configured ones; missing binary/auth errors print the adapter's stderr tail, not a stack trace)
- Modify: `crates/gray/src/main.rs` or setup (load `acp:` section from gray.yml into the REPL; reuse the existing gray.yml boot path)
- Test: repl tests with a stub `AcpClient` (or fake adapter): routes text, handles unknown agent, surfaces errors cleanly

**Interfaces:**
- Consumes: shared contract (Task 1 implements it in parallel — code against the contract, not Task 1's internals).
- Produces: `/acp codex <prompt>` prints worker text; `/help` lists `/acp`.

- [ ] **Step 1: Dispatch + resolution (failing test first)**

Tests: `/acp nosuchagent …` → lists available; adapter spawn failure → one-line error + hint (binary missing? npx needed?).

- [ ] **Step 2: Prompt flow + cancel (failing test first)**

Ctrl-C during `/acp` cancels the worker session (calls `cancel()`); normal path prints final text through the normal reply pipeline.

- [ ] **Step 3: /help + verify + commit**

Run: `cargo test -p gray --quiet && cargo clippy --workspace --quiet`
```bash
git add crates/gray/
git commit -m "feat(acp): /acp <agent> <prompt> REPL command"
```

---

### Task 3: Docs + attribution (after 1+2)

**Files:**
- Modify: `README.md` (short `/acp` section: what it is, adapter setup, env keys, read-only default, example session)
- Modify: `README.md` Acknowledgements (`agent-client-protocol`, Apache-2.0) + `gray.yml` example block

- [ ] **Step 1: Write + verify commands actually run**

Every command in the docs is executed once against the fake adapter or `--help` before committing.

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: /acp delegate command + Apache attribution"
```
