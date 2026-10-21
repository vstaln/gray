use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gray_pkg::index::Index;
use gray_pkg::staged::{ApprovedEntry, InstallStage};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::ConsentView;
use super::draw::safe_text;
use crate::plugin_install::{self, InstallResult};

#[async_trait::async_trait]
pub(crate) trait MarketBackend: Send + Sync + 'static {
    type Prepared: Send + 'static;
    async fn load(&self, cancel: CancellationToken) -> anyhow::Result<(Index, BTreeSet<String>)>;
    async fn prepare(
        &self,
        approved: ApprovedEntry,
        cancel: CancellationToken,
        progress: UnboundedSender<InstallStage>,
    ) -> anyhow::Result<(Self::Prepared, ConsentView)>;
    async fn commit(&self, prepared: Self::Prepared) -> anyhow::Result<InstallResult>;
    async fn installed(&self) -> anyhow::Result<Option<BTreeSet<String>>> {
        Ok(None)
    }
    fn record_failure(&self, _name: &str, _message: &str) {}
}

pub(crate) struct Backend {
    pub home: PathBuf,
}

fn installed_names(home: &Path) -> anyhow::Result<BTreeSet<String>> {
    let lock = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))?;
    anyhow::ensure!(lock.schema == 1, "unsupported plugin lock schema");
    Ok(lock.plugins.into_keys().collect())
}

#[async_trait::async_trait]
impl MarketBackend for Backend {
    type Prepared = plugin_install::PreparedInstall;

    async fn load(&self, cancel: CancellationToken) -> anyhow::Result<(Index, BTreeSet<String>)> {
        let client = gray_pkg::fetch::client()?;
        let catalog = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("catalog load cancelled"),
            result = gray_pkg::index::fetch_index_uncached(&client) => result?,
        };
        Ok((catalog, installed_names(&self.home)?))
    }

    async fn prepare(
        &self,
        approved: ApprovedEntry,
        cancel: CancellationToken,
        progress: UnboundedSender<InstallStage>,
    ) -> anyhow::Result<(Self::Prepared, ConsentView)> {
        let prepared =
            plugin_install::prepare(&self.home, approved, cancel, Some(progress)).await?;
        let view = ConsentView {
            capabilities: plugin_install::policy::capability_lines(&prepared.manifest.declared),
            commands: prepared.manifest.commands.clone(),
            scan_text: format!("{}\n{}", prepared.scan.summary(), prepared.scan.render()),
        };
        Ok((prepared, view))
    }

    async fn commit(&self, prepared: Self::Prepared) -> anyhow::Result<InstallResult> {
        let consent = plugin_install::policy::grant(&prepared.manifest.declared);
        plugin_install::commit(prepared, consent).await
    }

    async fn installed(&self) -> anyhow::Result<Option<BTreeSet<String>>> {
        installed_names(&self.home).map(Some)
    }

    fn record_failure(&self, name: &str, message: &str) {
        gray_pkg::errors::record("plugin", name, message.to_owned());
    }
}

pub enum MarketCommand {
    Load,
    Prepare(Box<ApprovedEntry>),
    Grant,
    Cancel,
    Close,
}

pub struct TaggedCommand {
    pub operation: u64,
    pub command: MarketCommand,
}

pub enum MarketEvent {
    Loaded(Index, BTreeSet<String>),
    Stage(InstallStage),
    Ready(ConsentView),
    Registry(BTreeSet<String>),
    Committed(InstallResult),
    Failed(String),
    Cancelled,
    Closed,
}

pub struct TaggedEvent {
    pub operation: u64,
    pub event: MarketEvent,
}

pub struct WorkerHandle {
    pub commands: UnboundedSender<TaggedCommand>,
    pub events: UnboundedReceiver<TaggedEvent>,
    pub task: JoinHandle<()>,
}

pub fn spawn_backend<B: MarketBackend>(backend: Arc<B>) -> WorkerHandle {
    let (commands, rx) = mpsc::unbounded_channel();
    let (tx, events) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(backend, rx, tx));
    WorkerHandle {
        commands,
        events,
        task,
    }
}

fn emit(events: &UnboundedSender<TaggedEvent>, operation: u64, event: MarketEvent) {
    let _ = events.send(TaggedEvent { operation, event });
}

struct Completion<T> {
    result: anyhow::Result<T>,
    cancelled: bool,
    next: Option<TaggedCommand>,
}

// Never drop or abort the child: cancelling preparation must await its cleanup.
async fn finish<T: Send + 'static>(
    mut task: JoinHandle<anyhow::Result<T>>,
    operation: u64,
    cancel: Option<CancellationToken>,
    commands: &mut UnboundedReceiver<TaggedCommand>,
    events: &UnboundedSender<TaggedEvent>,
    mut progress: UnboundedReceiver<InstallStage>,
) -> Completion<T> {
    let mut next = None;
    let mut cancelled = false;
    let mut closing = false;
    let mut progress_open = true;
    loop {
        tokio::select! {
            biased;
            command = commands.recv(), if !closing => {
                let command = command.unwrap_or(TaggedCommand { operation, command: MarketCommand::Close });
                match command.command {
                    MarketCommand::Close => {
                        closing = true;
                        next = Some(command);
                        if let Some(token) = &cancel { token.cancel(); cancelled = true; }
                    }
                    MarketCommand::Cancel if command.operation == operation => {
                        if let Some(token) = &cancel { token.cancel(); cancelled = true; }
                    }
                    MarketCommand::Load | MarketCommand::Prepare(_) if command.operation > operation && cancel.is_some() => {
                        if let Some(token) = &cancel { token.cancel(); }
                        cancelled = true;
                        if next.as_ref().is_none_or(|old: &TaggedCommand| old.operation < command.operation) {
                            next = Some(command);
                        }
                    }
                    _ => {}
                }
            }
            _ = events.closed(), if !closing => {
                closing = true;
                next = Some(TaggedCommand { operation, command: MarketCommand::Close });
                if let Some(token) = &cancel { token.cancel(); cancelled = true; }
            }
            stage = progress.recv(), if progress_open => match stage {
                Some(stage) if !cancelled => emit(events, operation, MarketEvent::Stage(stage)),
                Some(_) => {}
                None => progress_open = false,
            },
            result = &mut task => {
                return Completion {
                    // Do not expose panic payloads, which can include arbitrary plugin output.
                    result: result.unwrap_or_else(|_| Err(anyhow::anyhow!("marketplace worker failed"))),
                    cancelled,
                    next,
                };
            }
        }
    }
}

async fn run<B: MarketBackend>(
    backend: Arc<B>,
    mut commands: UnboundedReceiver<TaggedCommand>,
    events: UnboundedSender<TaggedEvent>,
) {
    let mut operation = 0;
    let mut started = false;
    let mut prepared: Option<B::Prepared> = None;
    let mut prepared_name = String::new();
    let mut pending: Option<TaggedCommand> = None;
    loop {
        let tagged = if let Some(command) = pending.take() {
            command
        } else {
            tokio::select! {
                command = commands.recv() => command.unwrap_or(TaggedCommand { operation, command: MarketCommand::Close }),
                _ = events.closed() => TaggedCommand { operation, command: MarketCommand::Close },
            }
        };
        let (stage_tx, stage_rx) = mpsc::unbounded_channel();
        match tagged.command {
            MarketCommand::Close => {
                drop(prepared.take());
                emit(&events, operation, MarketEvent::Closed);
                return;
            }
            MarketCommand::Load | MarketCommand::Prepare(_)
                if started && tagged.operation <= operation => {}
            MarketCommand::Load => {
                operation = tagged.operation;
                started = true;
                drop(prepared.take());
                let cancel = CancellationToken::new();
                let token = cancel.clone();
                let backend = backend.clone();
                drop(stage_tx);
                let done = finish(
                    tokio::spawn(async move { backend.load(token).await }),
                    operation,
                    Some(cancel),
                    &mut commands,
                    &events,
                    stage_rx,
                )
                .await;
                pending = done.next;
                if done.cancelled {
                    emit(&events, operation, MarketEvent::Cancelled);
                    continue;
                }
                match done.result {
                    Ok((index, installed)) => {
                        emit(&events, operation, MarketEvent::Loaded(index, installed))
                    }
                    Err(error) => emit(
                        &events,
                        operation,
                        MarketEvent::Failed(safe_text(&format!("{error:#}"))),
                    ),
                }
            }
            MarketCommand::Prepare(approved) => {
                operation = tagged.operation;
                started = true;
                drop(prepared.take());
                prepared_name = approved.name().into();
                let cancel = CancellationToken::new();
                let token = cancel.clone();
                let prepare_backend = backend.clone();
                let done = finish(
                    tokio::spawn(async move {
                        prepare_backend.prepare(*approved, token, stage_tx).await
                    }),
                    operation,
                    Some(cancel),
                    &mut commands,
                    &events,
                    stage_rx,
                )
                .await;
                pending = done.next;
                if done.cancelled {
                    drop(done.result);
                    emit(&events, operation, MarketEvent::Cancelled);
                    continue;
                }
                match done.result {
                    Ok((package, view)) => {
                        prepared = Some(package);
                        emit(&events, operation, MarketEvent::Ready(view));
                    }
                    Err(error) => {
                        let message = safe_text(&format!("{error:#}"));
                        backend.record_failure(&prepared_name, &message);
                        emit(&events, operation, MarketEvent::Failed(message));
                    }
                }
            }
            MarketCommand::Grant if tagged.operation == operation && prepared.is_some() => {
                let Some(package) = prepared.take() else {
                    continue;
                };
                let commit_backend = backend.clone();
                emit(
                    &events,
                    operation,
                    MarketEvent::Stage(InstallStage::Committing),
                );
                drop(stage_tx);
                let done = finish(
                    tokio::spawn(async move {
                        let mut result = commit_backend.commit(package).await?;
                        let registry = match commit_backend.installed().await {
                            Ok(registry) => registry,
                            Err(error) => {
                                result
                                    .warnings
                                    .push(format!("installed registry refresh failed: {error:#}"));
                                None
                            }
                        };
                        Ok((result, registry))
                    }),
                    operation,
                    None,
                    &mut commands,
                    &events,
                    stage_rx,
                )
                .await;
                pending = done.next;
                match done.result {
                    Ok((mut result, registry)) => {
                        if let Some(installed) = registry {
                            emit(&events, operation, MarketEvent::Registry(installed));
                        }
                        result.warnings = result
                            .warnings
                            .iter()
                            .map(|warning| safe_text(warning))
                            .collect();
                        emit(&events, operation, MarketEvent::Committed(result));
                    }
                    Err(error) => {
                        let message = safe_text(&format!("{error:#}"));
                        backend.record_failure(&prepared_name, &message);
                        emit(&events, operation, MarketEvent::Failed(message));
                    }
                }
            }
            MarketCommand::Cancel if tagged.operation == operation => {
                drop(prepared.take());
                emit(&events, operation, MarketEvent::Cancelled);
            }
            _ => {}
        }
    }
}
