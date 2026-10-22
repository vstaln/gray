use std::path::Path;
use std::time::Duration;

use gray_core::agent::ToolContext;
use gray_core::event::Usage;
use gray_plugin::builder::resolve_argv;
use gray_plugin::sidecar::SidecarPlugin;
use gray_plugin::{CoreEvent, Plugin};

fn outcome_text(o: &gray_core::agent::CommandOutcome) -> String {
    match o {
        gray_core::agent::CommandOutcome::Say(s) => format!("{} bytes", s.len()),
        gray_core::agent::CommandOutcome::Prompt(s) => format!("prompt {} bytes", s.len()),
        gray_core::agent::CommandOutcome::ModelPicker(row) => format!("model picker on {row}"),
        gray_core::agent::CommandOutcome::Panel(spec) => {
            format!("panel: {} ({} pages)", spec.title, spec.pages.len())
        }
        gray_core::agent::CommandOutcome::Picker(spec) => {
            format!("picker: {} ({} items)", spec.title, spec.items.len())
        }
        gray_core::agent::CommandOutcome::Terminal { argv, .. } => {
            format!("terminal: {}", argv.join(" "))
        }
    }
}

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

pub async fn check_plugin_dir(dir: &str) -> anyhow::Result<()> {
    let mut argv = resolve_argv(Path::new(dir))?;
    let argv0 = argv.first().cloned().unwrap_or_default();

    let probe = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(&argv0)
            .arg("manifest")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await;
    if let Ok(Ok(o)) = &probe
        && o.status.success()
        && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout)
    {
        argv.extend(
            v["sidecar_args"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string)),
        );
    }
    let plugin =
        match tokio::time::timeout(Duration::from_secs(35), SidecarPlugin::spawn(argv)).await {
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
    let mut reports = vec![Report {
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
    }];

    reports.push(match probe {
        Ok(Ok(o)) if o.status.success() => {
            match serde_json::from_slice::<serde_json::Value>(&o.stdout) {
                Ok(v) => {
                    let name_ok = v.get("name").and_then(|n| n.as_str()) == Some(m.name.as_str());
                    let proto_ok = v.get("protocol").is_some();
                    Report {
                        name: "argv-manifest",
                        pass: name_ok && proto_ok,
                        detail: format!(
                            "argv manifest {} wire (name={}, protocol present={})",
                            if name_ok && proto_ok {
                                "agrees with"
                            } else {
                                "disagrees with"
                            },
                            name_ok,
                            proto_ok
                        ),
                    }
                }
                Err(e) => Report {
                    name: "argv-manifest",
                    pass: false,
                    detail: format!("manifest arg printed non-JSON: {e}"),
                },
            }
        }
        Ok(Ok(o)) => Report {
            name: "argv-manifest",
            pass: false,
            detail: format!(
                "manifest arg exited {:?} — install probe would fail",
                o.status.code()
            ),
        },
        Ok(Err(e)) => Report {
            name: "argv-manifest",
            pass: false,
            detail: format!("manifest arg spawn failed: {e}"),
        },
        Err(_) => Report {
            name: "argv-manifest",
            pass: false,
            detail: "manifest arg timed out".to_string(),
        },
    });

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
                detail: format!(
                    "{name} → {} bytes, is_error={}",
                    o.content.len(),
                    o.is_error
                ),
            },
            Err(_) => Report {
                name: "tool/call",
                pass: false,
                detail: format!("{name} hung past 10s (hang-fixture behavior)"),
            },
        });

        let t2 = plugin.tools()[0].clone();
        let ctx = ToolContext::default();
        let both = tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(
                tool.execute(&ctx, serde_json::json!({})),
                t2.execute(&ctx, serde_json::json!({}))
            )
        })
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

    if m.hooks.iter().any(|h| h == "tool/before") {
        let t = std::time::Instant::now();
        let verdict = tokio::time::timeout(
            Duration::from_secs(15),
            plugin.tool_before("bash", &serde_json::json!({"command": "check-probe"})),
        )
        .await;
        reports.push(match verdict {
            Ok(gray_core::agent::ToolBefore::Allow) => Report {
                name: "tool/before",
                pass: true,
                detail: format!("allow resolved in {:?}", t.elapsed()),
            },
            Ok(gray_core::agent::ToolBefore::Deny(_)) => Report {
                name: "tool/before",
                pass: true,
                detail: format!("deny (fail-closed without host) in {:?}", t.elapsed()),
            },
            Ok(gray_core::agent::ToolBefore::Modify(_)) => Report {
                name: "tool/before",
                pass: true,
                detail: format!("modify resolved in {:?}", t.elapsed()),
            },
            Err(_) => Report {
                name: "tool/before",
                pass: false,
                detail: "tool/before hung past 15s (hang-fixture behavior)".to_string(),
            },
        });
    }

    for cmd in m.commands.iter() {
        let t = std::time::Instant::now();
        let outcome =
            tokio::time::timeout(Duration::from_secs(15), plugin.run_command(cmd, Vec::new()))
                .await;
        reports.push(match outcome {
            Ok(Some(o)) => Report {
                name: "command/run",
                pass: true,
                detail: format!("{cmd} → {} in {:?}", outcome_text(&o), t.elapsed()),
            },
            Ok(None) => Report {
                name: "command/run",
                pass: true,
                detail: format!("{cmd} declined (empty) in {:?}", t.elapsed()),
            },
            Err(_) => Report {
                name: "command/run",
                pass: false,
                detail: format!("{cmd} hung past 15s (hang-fixture behavior)"),
            },
        });
    }

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
