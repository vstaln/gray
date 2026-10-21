pub(crate) mod policy;
pub(crate) mod probe;

use std::path::Path;

use gray_pkg::staged::{
    ApprovedEntry, CommitMetadata, CommitOutcome, InstallStage, PreparedPackage,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub(crate) struct PreparedInstall {
    pub(crate) package: PreparedPackage,
    pub(crate) manifest: probe::PreparedManifest,
    pub(crate) scan: gray_plugin::scan::Report,
    progress: Option<UnboundedSender<InstallStage>>,
}

pub(crate) async fn prepare(
    home: &Path,
    approved: ApprovedEntry,
    cancel: CancellationToken,
    progress: Option<UnboundedSender<InstallStage>>,
) -> anyhow::Result<PreparedInstall> {
    let client = gray_pkg::fetch::client()?;
    let package =
        gray_pkg::staged::prepare(home, approved, &client, cancel.clone(), progress.clone())
            .await?;
    stage(&progress, InstallStage::Scanning);
    // Keep package ownership in this task and await the bounded scan before cancellation cleanup.
    let root = package.root().to_owned();
    let scan = tokio::task::spawn_blocking(move || gray_plugin::scan::scan_tree(&root)).await??;
    policy::require_installable_scan(&scan)?;
    anyhow::ensure!(!cancel.is_cancelled(), "preparation cancelled");
    stage(&progress, InstallStage::ReadingManifest);
    let manifest = probe::prepared(package.root(), cancel.clone()).await?;
    anyhow::ensure!(!cancel.is_cancelled(), "preparation cancelled");
    Ok(PreparedInstall {
        package,
        manifest,
        scan,
        progress,
    })
}

#[derive(Debug)]
pub struct InstallResult {
    pub name: String,
    pub installed: bool,
    pub warnings: Vec<String>,
}

pub(crate) async fn commit(
    prepared: PreparedInstall,
    consent: policy::ConsentRecord,
) -> anyhow::Result<InstallResult> {
    commit_with(prepared, consent, crate::plugin_cli::adopt_prepared).await
}

async fn commit_with(
    prepared: PreparedInstall,
    consent: policy::ConsentRecord,
    adopt: impl FnOnce(&Path, &str, &Path, &Value, bool) -> anyhow::Result<()> + Send + 'static,
) -> anyhow::Result<InstallResult> {
    let expected = policy::grant(&prepared.manifest.declared);
    anyhow::ensure!(
        consent == expected,
        "consent does not match prepared capabilities"
    );
    let home = prepared.package.home().to_owned();
    let name = prepared.package.approved().name().to_owned();
    let executable = home
        .join("plugins")
        .join(&name)
        .join(&prepared.manifest.executable_relative);
    let program = executable.to_string_lossy().into_owned();
    let argv = std::iter::once(program.clone())
        .chain(
            prepared.manifest.raw["sidecar_args"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string)),
        )
        .collect();
    let metadata = CommitMetadata {
        argv,
        cli_argv: prepared.manifest.cli.then(|| vec![program]),
        extra: serde_json::from_value(serde_json::json!({
            "granted_capabilities": consent.granted,
            "capabilities_hash": consent.hash,
            "runtime_role": crate::plugin_cli::provider_only_role_json(&prepared.manifest.raw)
        }))?,
    };
    stage(&prepared.progress, InstallStage::Committing);
    tokio::task::spawn_blocking(move || match prepared.package.commit(metadata)? {
        CommitOutcome::AlreadyInstalled(name) => Ok(InstallResult {
            name,
            installed: false,
            warnings: Vec::new(),
        }),
        CommitOutcome::Installed(_) => {
            let mut warnings = Vec::new();
            if let Err(error) = adopt(
                &home,
                &name,
                &executable,
                &prepared.manifest.raw,
                prepared.manifest.cli,
            ) {
                warnings.push(format!(
                    "installed, but manifest adoption failed: {error:#}"
                ));
            }
            Ok(InstallResult {
                name,
                installed: true,
                warnings,
            })
        }
    })
    .await?
}

fn stage(progress: &Option<UnboundedSender<InstallStage>>, value: InstallStage) {
    if let Some(tx) = progress {
        let _ = tx.send(value);
    }
}

#[cfg(test)]
mod tests;
