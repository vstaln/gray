//! `gray plugin check <dir>` — sidecar conformance runner.
//!
//! Boots the plugin exactly like `gray.yml` would and drives the
//! failure modes the `gray-plugin` fixtures pin down:
//! empty-name manifests bail at spawn, hung calls time out instead of
//! hanging the host, concurrent calls route by id, and shutdown is
//! graceful for v1.1 sidecars. One line per check; nonzero exit on failure.

use std::path::Path;
use std::time::Duration;

use gray_core::agent::ToolContext;
use gray_core::agent::Tool as _;
use gray_core::event::Usage;
use gray_plugin::sidecar::SidecarPlugin;
use gray_plugin::{CoreEvent, Plugin};

struct Report {
    name: &'static str,
    pass: bool,
    detail: String,
}

impl Report {
    fn show(&self) {
        let mark = if self.pass { "PASS" } else { "FAIL" };
        println!("{mark} {} — {}", self.name, self.detail);
    }
}

/// Resolve the spawn argv for a plugin dir: the dir itself when
/// executable, else `plugin.sh`, else the single executable inside.
fn resolve_argv(dir: &Path) -> anyhow::Result<Vec<String>> {
    use std::os::unix::fs::PermissionsExt;
    let is_exec = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if is_exec(dir) {
        return Ok(vec![dir.to_string_lossy().into_owned()]);
    }
    if !dir.is_dir() {
        anyhow::bail!("{} is not a directory (or executable)", dir.display());
    }
    let script = dir.join("plugin.sh");
    if is_exec(&script) {
        return Ok(vec![script.to_string_lossy().into_owned()]);
    }
    let mut execs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if is_exec(&path) {
            execs.push(path);
        }
    }
    match execs.len() {
        1 => Ok(vec![execs[0].to_string_lossy().into_owned()]),
        0 => anyhow::bail!(
            "no executable in {} (expected plugin.sh or one executable)",
            dir.display()
        ),
        _ => anyhow::bail!(
            "ambiguous plugin dir {}: several executables, add plugin.sh",
            dir.display()
        ),
    }
}

/// Run every check; `Err` (with per-check lines already printed) when any fail.
pub async fn check_plugin_dir(dir: &str) -> anyhow::Result<()> {
    let argv = resolve_argv(Path::new(dir))?;
    let plugin = match tokio::time::timeout(Duration::from_secs(35), SidecarPlugin::spawn(argv)).await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            println!("FAIL spawn — {e:#}");
            anyhow::bail!("spawn failed");
        }
        Err(_) => {
            println!("FAIL spawn — manifest handshake timed out (30s+)");
            anyhow::bail!("spawn timed out");
        }
    };
    let m = plugin.manifest();
    let mut reports = vec![
        Report {
            name: "manifest",
            pass: !m.name.trim().is_empty() && !m.version.trim().is_empty(),
            detail: format!(
                "name={:?} version={:?} tools={} commands={:?} protocol={:?}",
                m.name,
                m.version,
                m.tools.len(),
                m.commands,
                m.protocol
            ),
        },
        Report {
            name: "capabilities",
            pass: true,
            detail: format!("{:?}", m.capabilities),
        },
    ];

    // tool/call round-trip on the first tool (skipped when there are none).
    if let Some(tool) = plugin.tools().first().cloned() {
        let name = tool.def().name.clone();
        let out = tokio::time::timeout(
            Duration::from_secs(10),
            tool.execute(&ToolContext::default(), serde_json::json!({})),
        )
        .await;
        reports.push(match out {
            Ok(o) => Report {
                name: "tool/call",
                pass: true,
                detail: format!("{name} → {} bytes, is_error={}", o.content.len(), o.is_error),
            },
            Err(_) => Report {
                name: "tool/call",
                pass: false,
                detail: format!("{name} hung past 10s (hang-fixture behavior)"),
            },
        });
        // Concurrent calls must both resolve (reorder-fixture behavior).
        let t2 = plugin.tools()[0].clone();
        let ctx = ToolContext::default();
        let both = tokio::time::timeout(
            Duration::from_secs(15),
            async { tokio::join!(tool.execute(&ctx, serde_json::json!({})), t2.execute(&ctx, serde_json::json!({}))) },
        )
        .await;
        reports.push(Report {
            name: "concurrency",
            pass: both.is_ok(),
            detail: if both.is_ok() {
                "2 concurrent tool/calls both resolved".to_string()
            } else {
                "concurrent tool/calls hung past 15s".to_string()
            },
        });
    }

    // event/notify is fire-and-forget: must return fast even for plugins
    // that never reply (hang fixture).
    let t = std::time::Instant::now();
    plugin
        .on_event(CoreEvent::TurnEnd {
            usage: Usage::default(),
        })
        .await;
    let dt = t.elapsed();
    reports.push(Report {
        name: "notify",
        pass: dt < Duration::from_secs(5),
        detail: format!("turn_end notify returned in {dt:?}"),
    });

    // Graceful teardown (v1.1: plugin/shutdown; pre-v1: SIGKILL path).
    let t = std::time::Instant::now();
    plugin.shutdown(Duration::from_secs(5)).await;
    let dt = t.elapsed();
    reports.push(Report {
        name: "shutdown",
        pass: dt < Duration::from_secs(10),
        detail: format!("teardown took {dt:?}"),
    });

    let mut failed = 0;
    for r in &reports {
        r.show();
        if !r.pass {
            failed += 1;
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} check(s) failed");
    }
    Ok(())
}
