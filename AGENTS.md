# Instructions for AI Agents

## Build & Binary Installation
- After modifying code in `crates/gray` or other crates, always compile the release binary and copy/install it to the user's binary path:
  ```bash
  cargo build --release
  install target/release/gray ~/.local/bin/gray
  ```
- Always commit code changes.
