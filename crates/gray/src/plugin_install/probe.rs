use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use gray_plugin::sidecar::{MANIFEST_TTL, MAX_FRAME, PLUGIN_MANIFEST, PLUGIN_SHUTDOWN};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct PreparedManifest {
    pub raw: Value,
    pub declared: Vec<String>,
    pub commands: Vec<String>,
    pub executable_relative: PathBuf,
    pub cli: bool,
}

#[derive(Debug, thiserror::Error)]
enum CaptureFailure {
    #[error("plugin output exceeds 64 KiB")]
    OutputLimit,
    #[error("plugin request timed out")]
    TimedOut,
    #[error("plugin exited {status}{output}")]
    Exited {
        status: std::process::ExitStatus,
        output: String,
    },
}

fn failure_summary(error: &anyhow::Error) -> String {
    match error.downcast_ref::<CaptureFailure>() {
        Some(CaptureFailure::Exited { status, .. }) => format!("plugin exited {status}"),
        Some(other) => other.to_string(),
        None if error.is::<serde_json::Error>() => "invalid plugin manifest JSON".into(),
        None => "manifest command failed".into(),
    }
}

pub async fn manifest(executable: &Path, cancel: CancellationToken) -> anyhow::Result<Value> {
    let mut command = Command::new(executable);
    command.arg("manifest");
    let bytes = capture(command, Duration::from_secs(10), cancel).await?;
    let raw: Value = serde_json::from_slice(&bytes).context("invalid plugin manifest JSON")?;
    anyhow::ensure!(raw["name"].is_string(), "plugin manifest has no name");
    Ok(raw)
}

pub(super) async fn prepared(
    root: &Path,
    cancel: CancellationToken,
) -> anyhow::Result<PreparedManifest> {
    anyhow::ensure!(!cancel.is_cancelled(), "preparation cancelled");
    let argv = gray_plugin::builder::resolve_argv(root)?;
    let executable = Path::new(argv.first().context("plugin has no executable")?);
    let relative = executable
        .strip_prefix(root)
        .context("plugin executable is outside staging")?
        .to_path_buf();
    anyhow::ensure!(
        relative
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
            && !relative.as_os_str().is_empty(),
        "invalid prepared executable path"
    );
    let (raw, cli) = match manifest(executable, cancel.clone()).await {
        Ok(raw) => (raw, true),
        Err(error) => {
            anyhow::ensure!(!cancel.is_cancelled(), "preparation cancelled");
            let summary = failure_summary(&error);
            let raw = wire_manifest(executable, cancel).await.with_context(|| {
                format!("CLI manifest probe failed ({summary}); wire manifest probe failed")
            })?;
            (raw, false)
        }
    };
    anyhow::ensure!(
        raw.is_object()
            && raw["name"]
                .as_str()
                .is_some_and(|name| !name.trim().is_empty()),
        "plugin manifest has missing/empty name"
    );
    let strings = |key: &str| {
        raw[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    };
    Ok(PreparedManifest {
        declared: strings("capabilities"),
        commands: strings("commands"),
        raw,
        executable_relative: relative,
        cli,
    })
}

pub(crate) async fn capture(
    mut command: Command,
    ttl: Duration,
    cancel: CancellationToken,
) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(!cancel.is_cancelled(), "plugin request cancelled");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(anyhow::anyhow!("plugin request cancelled")),
        result = tokio::time::timeout(ttl, async {
            let mut stdout = child.stdout.take().context("missing stdout")?.take(65537);
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await?;
            if bytes.len() > 65536 {
                return Err(CaptureFailure::OutputLimit.into());
            }
            let status = child.wait().await?;
            if !status.success() {
                let tail = String::from_utf8_lossy(&bytes);
                let tail = tail.trim_end();
                let tail = tail.get(tail.len().saturating_sub(500)..).unwrap_or(tail);
                let output = if tail.is_empty() {
                    String::new()
                } else {
                    format!(" — last output: {tail}")
                };
                return Err(CaptureFailure::Exited { status, output }.into());
            }
            Ok(bytes)
        }) => result.unwrap_or_else(|_| Err(CaptureFailure::TimedOut.into())),
    };
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}

pub(super) async fn wire_manifest(
    executable: &Path,
    cancel: CancellationToken,
) -> anyhow::Result<Value> {
    anyhow::ensure!(!cancel.is_cancelled(), "plugin request cancelled");
    // SidecarPlugin::spawn detaches its transport before the handshake completes,
    // so cancellation cannot reap that child. Keep ownership here instead.
    // sidecar.rs is the source of truth for these newline-delimited RPC envelopes.
    let mut child = Command::new(executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(anyhow::anyhow!("plugin request cancelled")),
        result = tokio::time::timeout(MANIFEST_TTL, async {
            let stdin = child.stdin.as_mut().context("missing stdin")?;
            let request = json!({"id": 1, "method": PLUGIN_MANIFEST});
            stdin.write_all(format!("{request}\n").as_bytes()).await?;
            let stdout = child.stdout.take().context("missing stdout")?;
            let mut reader = BufReader::new(stdout.take((MAX_FRAME + 1) as u64));
            let mut total = 0;
            loop {
                let mut frame = Vec::new();
                let n = reader.read_until(b'\n', &mut frame).await?;
                total += n;
                anyhow::ensure!(total <= MAX_FRAME, "plugin wire output exceeds 256 KiB");
                anyhow::ensure!(n != 0, "sidecar child closed stdout");
                let Ok(reply) = serde_json::from_slice::<Value>(&frame) else { continue };
                if reply["id"].as_u64() == Some(1) {
                    let raw = reply.get("result").context("plugin manifest response has no result")?;
                    let manifest = gray_plugin::Manifest::from_result(raw);
                    anyhow::ensure!(!manifest.name.trim().is_empty(), "sidecar manifest has missing/empty name");
                    let mut raw = raw.clone();
                    raw["name"] = manifest.name.trim().into();
                    return Ok(raw);
                }
                if reply["id"].is_string()
                    && reply["method"].as_str().is_some_and(|m| m.starts_with("host/"))
                {
                    let response = json!({"id": reply["id"], "result": {
                        "error": "no host capabilities granted during preparation"
                    }});
                    stdin.write_all(format!("{response}\n").as_bytes()).await?;
                }
            }
        }) => result.unwrap_or_else(|_| Err(anyhow::anyhow!("sidecar manifest request timed out"))),
    };
    let mut result = result;
    if let Ok(raw) = &result
        && raw["protocol"].is_string()
    {
        let shutdown = json!({"method": PLUGIN_SHUTDOWN, "params": {"reason": "session_end"}});
        tokio::select! {
            biased;
            _ = cancel.cancelled() => result = Err(anyhow::anyhow!("plugin request cancelled")),
            _ = tokio::time::timeout(Duration::from_secs(2), async {
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(format!("{shutdown}\n").as_bytes()).await;
                }
                let _ = child.wait().await;
            }) => {}
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}
