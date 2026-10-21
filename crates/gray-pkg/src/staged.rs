use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct ApprovedEntry {
    name: String,
    entry: crate::index::Entry,
}

#[derive(Debug)]
pub struct PreparedPackage {
    approved: ApprovedEntry,
    home: PathBuf,
    stage: tempfile::TempDir,
    root: PathBuf,
}

impl ApprovedEntry {
    pub fn new(name: String, entry: crate::index::Entry) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "invalid marketplace plugin name"
        );
        anyhow::ensure!(
            !matches!(name.as_str(), "tmp" | "pi"),
            "reserved plugin name"
        );
        anyhow::ensure!(
            entry.ecosystem == "gray-native"
                && entry.source.type_ == "tarball"
                && matches!(entry.kind.as_str(), "" | "plugin"),
            "unsupported marketplace package"
        );
        anyhow::ensure!(
            entry.hash.primary().is_some_and(|h| !h.is_empty()),
            "marketplace package requires a digest"
        );
        Ok(Self { name, entry })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn entry(&self) -> &crate::index::Entry {
        &self.entry
    }
}

impl PreparedPackage {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn approved(&self) -> &ApprovedEntry {
        &self.approved
    }

    /// Publish files and consent under the registry guard. This boundary is not cancellable.
    pub fn commit(self, metadata: CommitMetadata) -> anyhow::Result<CommitOutcome> {
        self.commit_with(metadata, crate::ops::write_lock_in)
    }

    fn commit_with(
        self,
        metadata: CommitMetadata,
        write: impl FnOnce(&Path, &crate::ops::LockFile) -> anyhow::Result<()>,
    ) -> anyhow::Result<CommitOutcome> {
        metadata.validate()?;
        let plugins = self.home.join("plugins");
        let _guard = crate::ops::hold_registry_lock_in(&plugins)?
            .ok_or_else(|| anyhow::anyhow!("marketplace commit requires registry locking"))?;
        let mut lock = crate::ops::read_lock_in(&plugins)?.unwrap_or_default();
        if lock.plugins.contains_key(self.approved.name()) {
            return Ok(CommitOutcome::AlreadyInstalled(self.approved.name.clone()));
        }
        check_constraints_in(&lock, &self.approved)?;
        let destination = plugins.join(self.approved.name());
        match destination.symlink_metadata() {
            Ok(_) => anyhow::bail!(
                "unregistered plugin destination already exists: {}",
                destination.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let entry = self.approved.entry();
        lock.plugins.insert(
            self.approved.name.clone(),
            crate::ops::LockEntry {
                ecosystem: entry.ecosystem.clone(),
                version: entry.version.clone(),
                hash: entry.hash.primary().unwrap().to_owned(),
                source: entry.source.url.clone(),
                scope: if entry.scope.is_empty() {
                    "user".into()
                } else {
                    entry.scope.clone()
                },
                argv: metadata.argv,
                cli_argv: metadata.cli_argv,
                extra: metadata.extra,
                adapter_version: env!("CARGO_PKG_VERSION").into(),
                installed_at: crate::now_secs().to_string(),
                enabled: true,
                ..Default::default()
            },
        );
        std::fs::rename(&self.root, &destination)?;
        if let Err(error) = write(&plugins, &lock) {
            if let Err(rollback) = std::fs::rename(&destination, &self.root) {
                let recovery = self.stage.keep();
                anyhow::bail!(
                    "{error:#}; rollback failed: {rollback}; package recovery path: {}; staging retained at {}",
                    destination.display(),
                    recovery.display()
                );
            }
            return Err(error);
        }
        Ok(CommitOutcome::Installed(crate::ops::Report {
            name: self.approved.name.clone(),
            version: self.approved.entry.version.clone(),
            path: destination,
            deps: Vec::new(),
        }))
    }
}

#[derive(Default)]
pub struct CommitMetadata {
    pub argv: Vec<String>,
    pub cli_argv: Option<Vec<String>>,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl CommitMetadata {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.extra
                .get("capabilities_hash")
                .is_some_and(serde_json::Value::is_string)
                && self
                    .extra
                    .get("granted_capabilities")
                    .is_some_and(serde_json::Value::is_array),
            "marketplace commit requires explicit capability consent"
        );
        for key in self.extra.keys() {
            anyhow::ensure!(
                !matches!(
                    key.as_str(),
                    "ecosystem"
                        | "version"
                        | "hash"
                        | "source"
                        | "argv"
                        | "adapter_version"
                        | "installed_at"
                        | "scope"
                        | "enabled"
                        | "cli_argv"
                        | "global"
                ),
                "marketplace metadata uses reserved registry field: {key}"
            );
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum CommitOutcome {
    Installed(crate::ops::Report),
    AlreadyInstalled(String),
}

pub fn check_constraints(home: &Path, approved: &ApprovedEntry) -> anyhow::Result<()> {
    let lock = crate::ops::read_lock_in(&home.join("plugins"))?.unwrap_or_default();
    check_constraints_in(&lock, approved)
}

fn check_constraints_in(
    lock: &crate::ops::LockFile,
    approved: &ApprovedEntry,
) -> anyhow::Result<()> {
    let missing: Vec<_> = approved
        .entry
        .requires_plugins
        .iter()
        .filter(|name| !lock.plugins.contains_key(name.as_str()))
        .map(String::as_str)
        .collect();
    anyhow::ensure!(
        missing.is_empty(),
        "missing required plugins: {}",
        missing.join(", ")
    );
    let conflicts: Vec<_> = approved
        .entry
        .conflicts
        .iter()
        .filter(|name| lock.plugins.contains_key(name.as_str()))
        .map(String::as_str)
        .collect();
    anyhow::ensure!(
        conflicts.is_empty() || std::env::var_os("GRAY_PLUGIN_FORCE").is_some(),
        "conflicts with installed plugins: {}",
        conflicts.join(", ")
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallStage {
    Downloading,
    Unpacking,
    Scanning,
    ReadingManifest,
    Committing,
}

pub async fn prepare(
    home: &Path,
    approved: ApprovedEntry,
    client: &reqwest::Client,
    cancel: CancellationToken,
    progress: Option<tokio::sync::mpsc::UnboundedSender<InstallStage>>,
) -> anyhow::Result<PreparedPackage> {
    anyhow::ensure!(!cancel.is_cancelled(), "preparation cancelled");
    check_constraints(home, &approved)?;
    let plugins = home.join("plugins");
    std::fs::create_dir_all(&plugins)?;
    if let Some(tx) = &progress {
        let _ = tx.send(InstallStage::Downloading);
    }
    let tmp = plugins.join("tmp");
    let archive = tokio::select! {
        biased;
        _ = cancel.cancelled() => anyhow::bail!("preparation cancelled"),
        result = crate::fetch::download_owned(
            client, &approved.entry.source.url, Some(&approved.entry.hash),
            &tmp
        ) => result?,
    };
    let stage = tempfile::tempdir_in(&plugins)?;
    let home = home.to_path_buf();
    if let Some(tx) = &progress {
        let _ = tx.send(InstallStage::Unpacking);
    }
    // Keep both owned paths in the blocking job and await it even after cancellation.
    let prepared = tokio::task::spawn_blocking(move || {
        let root = stage.path().join("next");
        std::fs::create_dir(&root)?;
        crate::fetch::unpack_tar_gz(&archive, &root)?;
        Ok::<_, anyhow::Error>(PreparedPackage {
            approved,
            home,
            stage,
            root,
        })
    })
    .await??;
    anyhow::ensure!(!cancel.is_cancelled(), "preparation cancelled");
    Ok(prepared)
}

#[path = "staged_tests.rs"]
#[cfg(test)]
mod tests;
