//! ScheduledPrompts — engine-local "send this prompt at this time" records
//! (personal-fork feature).
//!
//! Storage mirrors `ProjectActionsStore`: a versioned JSON file under the
//! profile store root (`scheduled-prompts.json`), mutex-guarded, atomically
//! rewritten. It is deliberately NOT a synced doc — the engine that owns the
//! file is the one that fires, headed or headless.
//!
//! Firing reproduces the composer send path: an optional `createChat` upsert
//! for a scheduled NEW session (idempotent on the pre-minted chat id), then a
//! durable `Run` command on the session doc. The command plane handles remote
//! hosts from there, so a schedule may target a chat/space on another device.
//!
//! Exactly-once: firing claims the row under an in-memory `firing` set and
//! persists the outcome after the attempt. A crash between the durable
//! command write and the outcome persist leaves the row Pending-and-past-due,
//! so the next boot re-fires it — the stored `message_id` dedupes the
//! transcript entry (the worst case is a double dispatch, never a lost one).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use chrono::Utc;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeron_doc::SessionCommandPayload;
use zeron_proto::{
    ScheduledChatCreate, ScheduledPrompt, ScheduledPromptDraft, ScheduledPromptStatus,
};

use crate::doc_host::DocHost;
use crate::workspace_host::WorkspaceHost;
use crate::{EngineError, new_id};

const STORE_FILE: &str = "scheduled-prompts.json";
const STORE_VERSION: u32 = 1;
/// Pending admission cap — generous; rows are tiny.
pub const MAX_SCHEDULED_PROMPTS: usize = 200;
/// Terminal (fired/failed) rows kept for the management list.
const MAX_TERMINAL_ROWS: usize = 100;
const MAX_PROMPT_BYTES: usize = 64 * 1024;
/// A run_at within this window of now counts as "due" (timer granularity).
const DUE_SKEW_MS: i64 = 250;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledPromptsFile {
    version: u32,
    #[serde(default)]
    prompts: Vec<ScheduledPrompt>,
}

struct Inner {
    path: PathBuf,
    state: Mutex<Vec<ScheduledPrompt>>,
    /// Publishes the rendered list (pending by run_at, terminal newest-first).
    items_tx: watch::Sender<Vec<ScheduledPrompt>>,
    /// Kicks the supervisor to recompute its wake time on every mutation.
    wake: Notify,
    cancel: CancellationToken,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Rows mid-fire — run-now and the due-drain must not double-claim.
    firing: Mutex<std::collections::HashSet<String>>,
    workspace: WorkspaceHost,
    doc_host: DocHost,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub struct ScheduledPrompts {
    inner: Arc<Inner>,
}

impl ScheduledPrompts {
    /// Open the store and start the fire supervisor. Requires a tokio runtime.
    pub fn open(
        profile_store_root: &Path,
        workspace: WorkspaceHost,
        doc_host: DocHost,
    ) -> Result<Self, EngineError> {
        std::fs::create_dir_all(profile_store_root)?;
        let path = profile_store_root.join(STORE_FILE);
        let prompts = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<ScheduledPromptsFile>(&bytes) {
                Ok(file) if file.version == STORE_VERSION => file.prompts,
                Ok(file) => {
                    tracing::warn!(
                        path = %path.display(),
                        version = file.version,
                        "unsupported scheduled-prompts store version; starting empty"
                    );
                    Vec::new()
                }
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "invalid scheduled-prompts store; starting empty"
                    );
                    Vec::new()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.into()),
        };
        let (items_tx, _) = watch::channel(Vec::new());
        let scheduler = Self {
            inner: Arc::new(Inner {
                path,
                state: Mutex::new(prompts),
                items_tx,
                wake: Notify::new(),
                cancel: CancellationToken::new(),
                supervisor: Mutex::new(None),
                firing: Mutex::new(std::collections::HashSet::new()),
                workspace,
                doc_host,
            }),
        };
        scheduler.publish();
        let task = tokio::spawn(supervisor(
            Arc::downgrade(&scheduler.inner),
            scheduler.inner.cancel.clone(),
        ));
        *lock(&scheduler.inner.supervisor) = Some(task);
        Ok(scheduler)
    }

    /// Stop the supervisor (fired rows are already durable; pending rows wait
    /// for the next boot). Idempotent.
    pub async fn shutdown(&self) {
        self.inner.cancel.cancel();
        let task = lock(&self.inner.supervisor).take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    /// The rendered list for `WatchScheduledPrompts`.
    pub fn watch(&self) -> watch::Receiver<Vec<ScheduledPrompt>> {
        self.inner.items_tx.subscribe()
    }

    pub fn list(&self) -> Vec<ScheduledPrompt> {
        render(lock(&self.inner.state).clone())
    }

    /// Admit a draft. The chat id / message id are minted (or adopted) here so
    /// every fire attempt — including a post-crash re-fire — dedupes on them.
    pub fn schedule(&self, draft: ScheduledPromptDraft) -> Result<ScheduledPrompt, EngineError> {
        let prompt = draft.request.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(EngineError::Other("Prompt is required".into()));
        }
        if prompt.len() > MAX_PROMPT_BYTES {
            return Err(EngineError::Other(format!(
                "Prompt must not exceed {MAX_PROMPT_BYTES} bytes"
            )));
        }
        if draft.run_at <= Utc::now() - chrono::Duration::milliseconds(DUE_SKEW_MS) {
            return Err(EngineError::Other(
                "Scheduled time is in the past — pick a future time".into(),
            ));
        }
        let chat_id = draft
            .chat_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(new_id);
        match &draft.create {
            Some(create) => {
                if create.space_id.is_none() && create.device_id.is_none() {
                    return Err(EngineError::Other(
                        "New scheduled sessions need a spaceId or a deviceId".into(),
                    ));
                }
            }
            // An existing-chat schedule must name a chat this workspace knows —
            // otherwise the fired command would materialize a dangling doc.
            None => match self.inner.workspace.chat(&chat_id) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(EngineError::Other("Chat not found".into()));
                }
                Err(err) => return Err(err),
            },
        }
        let item = ScheduledPrompt {
            id: new_id(),
            chat_id,
            message_id: new_id(),
            create: draft.create,
            request: zeron_proto::RunRequest {
                prompt,
                ..draft.request
            },
            run_at: draft.run_at,
            created_at: Utc::now(),
            status: ScheduledPromptStatus::Pending,
            fired_at: None,
            error: None,
        };
        {
            let mut state = lock(&self.inner.state);
            let mut next = state.clone();
            let pending = next
                .iter()
                .filter(|row| row.status == ScheduledPromptStatus::Pending)
                .count();
            if pending >= MAX_SCHEDULED_PROMPTS {
                return Err(EngineError::Other(format!(
                    "At most {MAX_SCHEDULED_PROMPTS} prompts can be scheduled"
                )));
            }
            next.push(item.clone());
            persist(&self.inner.path, &next)?;
            *state = next;
        }
        self.publish();
        self.inner.wake.notify_one();
        Ok(item)
    }

    /// Delete a row (cancelling a pending one). Returns false when absent.
    pub fn delete(&self, id: &str) -> Result<bool, EngineError> {
        let mut state = lock(&self.inner.state);
        let mut next = state.clone();
        let before = next.len();
        next.retain(|row| row.id != id);
        if next.len() == before {
            return Ok(false);
        }
        persist(&self.inner.path, &next)?;
        *state = next;
        drop(state);
        self.publish();
        Ok(true)
    }

    /// Fire a pending (or retry a failed) row now. Returns the updated row.
    pub async fn run_now(&self, id: &str) -> Result<ScheduledPrompt, EngineError> {
        let item = {
            let mut state = lock(&self.inner.state);
            let index = state
                .iter()
                .position(|row| row.id == id)
                .ok_or_else(|| EngineError::Other("Scheduled prompt not found".into()))?;
            if state[index].status == ScheduledPromptStatus::Fired {
                return Err(EngineError::Other("Scheduled prompt already fired".into()));
            }
            // Failed rows re-arm through the same claim path.
            state[index].status = ScheduledPromptStatus::Pending;
            state[index].run_at = Utc::now();
            state[index].error = None;
            let item = state[index].clone();
            persist(&self.inner.path, &state)?;
            item
        };
        self.publish();
        self.fire(item.clone()).await;
        Ok(self
            .list()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap_or(item))
    }

    /// The fire path shared by the due-drain and `run_now`: claim under the
    /// in-flight guard, attempt, persist the outcome.
    async fn fire(&self, item: ScheduledPrompt) {
        if !lock(&self.inner.firing).insert(item.id.clone()) {
            return;
        }
        let outcome = self.fire_inner(&item).await;
        lock(&self.inner.firing).remove(&item.id);
        {
            let mut state = lock(&self.inner.state);
            let mut next = state.clone();
            if let Some(row) = next.iter_mut().find(|row| row.id == item.id) {
                match &outcome {
                    Ok(()) => {
                        row.status = ScheduledPromptStatus::Fired;
                        row.fired_at = Some(Utc::now());
                        row.error = None;
                    }
                    Err(err) => {
                        row.status = ScheduledPromptStatus::Failed;
                        row.error = Some(err.to_string());
                    }
                }
            }
            prune_terminal(&mut next);
            let _ = persist(&self.inner.path, &next);
            *state = next;
        }
        if let Err(err) = &outcome {
            tracing::warn!(id = %item.id, chat = %item.chat_id, error = %err, "scheduled prompt failed");
        }
        self.publish();
    }

    async fn fire_inner(&self, item: &ScheduledPrompt) -> Result<(), EngineError> {
        if let Some(create) = &item.create {
            self.create_chat(&item.chat_id, create)?;
        }
        self.inner
            .doc_host
            .queue_command(
                &item.chat_id,
                SessionCommandPayload::Run {
                    request: item.request.clone(),
                    message_id: item.message_id.clone(),
                },
            )
            .map(|_| ())
    }

    fn create_chat(&self, chat_id: &str, create: &ScheduledChatCreate) -> Result<(), EngineError> {
        self.inner.workspace.create_chat(
            chat_id,
            create.space_id.as_deref(),
            create.device_id.as_deref(),
            create.config.clone(),
            create.cwd.clone(),
        )?;
        if let Some(branch) = create
            .branch
            .as_deref()
            .filter(|branch| !branch.trim().is_empty())
        {
            self.inner.workspace.set_chat_branch(chat_id, branch)?;
        }
        Ok(())
    }

    /// Claim every due Pending row and fire them sequentially.
    async fn fire_due(&self) {
        loop {
            let due = {
                let state = lock(&self.inner.state);
                let now = Utc::now() + chrono::Duration::milliseconds(DUE_SKEW_MS);
                state
                    .iter()
                    .filter(|row| row.status == ScheduledPromptStatus::Pending && row.run_at <= now)
                    .min_by_key(|row| row.run_at)
                    .cloned()
            };
            let Some(item) = due else { return };
            self.fire(item).await;
        }
    }

    fn publish(&self) {
        let rendered = render(lock(&self.inner.state).clone());
        let _ = self.inner.items_tx.send(rendered);
    }
}

/// Pending rows by soonest fire time, then terminal rows newest-first.
fn render(mut rows: Vec<ScheduledPrompt>) -> Vec<ScheduledPrompt> {
    rows.sort_by(|a, b| match (a.status, b.status) {
        (ScheduledPromptStatus::Pending, ScheduledPromptStatus::Pending) => a.run_at.cmp(&b.run_at),
        (ScheduledPromptStatus::Pending, _) => std::cmp::Ordering::Less,
        (_, ScheduledPromptStatus::Pending) => std::cmp::Ordering::Greater,
        _ => b.created_at.cmp(&a.created_at),
    });
    rows
}

/// Bound the file: drop the oldest terminal rows past the cap.
fn prune_terminal(rows: &mut Vec<ScheduledPrompt>) {
    let terminal = rows
        .iter()
        .filter(|row| row.status != ScheduledPromptStatus::Pending)
        .count();
    if terminal <= MAX_TERMINAL_ROWS {
        return;
    }
    let mut excess = terminal - MAX_TERMINAL_ROWS;
    rows.retain(|row| {
        if excess > 0 && row.status != ScheduledPromptStatus::Pending {
            excess -= 1;
            false
        } else {
            true
        }
    });
}

async fn supervisor(inner: Weak<Inner>, cancel: CancellationToken) {
    loop {
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let next = lock(&inner.state)
            .iter()
            .filter(|row| row.status == ScheduledPromptStatus::Pending)
            .map(|row| row.run_at)
            .min();
        match next {
            None => {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = inner.wake.notified() => {}
                }
            }
            Some(run_at) => {
                let delay = (run_at - Utc::now()).to_std().unwrap_or(Duration::ZERO);
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = inner.wake.notified() => {}
                    _ = tokio::time::sleep(delay) => {
                        let scheduler = ScheduledPrompts { inner: inner.clone() };
                        scheduler.fire_due().await;
                    }
                }
            }
        }
    }
}

fn persist(path: &Path, rows: &[ScheduledPrompt]) -> Result<(), EngineError> {
    let file = ScheduledPromptsFile {
        version: STORE_VERSION,
        prompts: rows.to_vec(),
    };
    let mut bytes = serde_json::to_vec_pretty(&file)
        .map_err(|err| EngineError::Other(format!("serialize scheduled prompts: {err}")))?;
    bytes.push(b'\n');
    let parent = path
        .parent()
        .ok_or_else(|| EngineError::Other("scheduled-prompts store has no parent".into()))?;
    std::fs::create_dir_all(parent)?;
    let temp_path = parent.join(format!(".{STORE_FILE}.tmp-{}", Uuid::new_v4()));
    let result = (|| -> Result<(), EngineError> {
        let mut temp = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        temp.write_all(&bytes)?;
        temp.sync_all()?;
        std::fs::rename(&temp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::RunRequest;

    fn request(prompt: &str) -> RunRequest {
        RunRequest {
            prompt: prompt.to_string(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: "~".into(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            resume: None,
            attachments: Vec::new(),
            worktree: None,
        }
    }

    #[test]
    fn render_orders_pending_soonest_then_terminal_newest() {
        let t0 = Utc::now();
        let row = |id: &str, run_at, status| ScheduledPrompt {
            id: id.into(),
            chat_id: "c".into(),
            message_id: "m".into(),
            create: None,
            request: request("p"),
            run_at,
            created_at: t0,
            status,
            fired_at: None,
            error: None,
        };
        let rows = vec![
            row("fired", t0, ScheduledPromptStatus::Fired),
            row(
                "later",
                t0 + chrono::Duration::hours(2),
                ScheduledPromptStatus::Pending,
            ),
            row(
                "sooner",
                t0 + chrono::Duration::hours(1),
                ScheduledPromptStatus::Pending,
            ),
        ];
        let ids: Vec<_> = render(rows).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["sooner", "later", "fired"]);
    }

    #[test]
    fn prune_terminal_drops_oldest_terminal_rows() {
        let t0 = Utc::now();
        let row = |id: &str, status| ScheduledPrompt {
            id: id.into(),
            chat_id: "c".into(),
            message_id: "m".into(),
            create: None,
            request: request("p"),
            run_at: t0,
            created_at: t0,
            status,
            fired_at: None,
            error: None,
        };
        let mut rows: Vec<_> = (0..MAX_TERMINAL_ROWS + 2)
            .map(|i| row(&format!("t{i}"), ScheduledPromptStatus::Fired))
            .collect();
        rows.push(row("pending", ScheduledPromptStatus::Pending));
        prune_terminal(&mut rows);
        assert_eq!(rows.len(), MAX_TERMINAL_ROWS + 1);
        assert!(rows.iter().any(|r| r.id == "pending"));
        assert!(!rows.iter().any(|r| r.id == "t0"));
        assert!(!rows.iter().any(|r| r.id == "t1"));
    }
}
