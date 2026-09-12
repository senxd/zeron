//! Live interrupt → resume against the four providers used in this change.
//! cargo test -p zeron-harness --test resume_interrupted_live -- --ignored --nocapture

use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{
    AcpHarness, CancellationToken, CodexHarness, CursorHarness, Harness, RunControls, SteerMessage,
};
use zeron_proto::{
    AgentEvent, DoneStatus, RESUME_INTERRUPTED_PROMPT, ReasoningLevel, RunRequest, SandboxLevel,
};

fn controls(interrupt: CancellationToken) -> RunControls {
    let (_steer, steering) = mpsc::channel::<SteerMessage>(1);
    RunControls {
        steering,
        interrupt,
        request_input: Box::new(|_| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Vec::new());
            rx
        }),
    }
}

fn request(prompt: &str, model: &str, reasoning: Option<ReasoningLevel>, cwd: &str) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: Some(model.into()),
        reasoning,
        model_options: Default::default(),
        cwd: cwd.into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        resume_interrupted: false,
    }
}

async fn interrupt_then_resume(
    label: &str,
    harness: &dyn Harness,
    model: &str,
    reasoning: Option<ReasoningLevel>,
) -> Result<String, String> {
    let cwd = tempfile::tempdir().map_err(|e| e.to_string())?;
    let cwd = cwd.path().display().to_string();
    let interrupt = CancellationToken::new();
    let first = request(
        "Count from 1 to 80, one number per line. Do not stop early.",
        model,
        reasoning,
        &cwd,
    );
    let mut stream = harness
        .run(first, controls(interrupt.clone()))
        .await
        .map_err(|e| format!("{label} start: {e}"))?;

    let mut session_id = None;
    let mut saw_text = false;
    let first_done = tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(event) = stream.next().await {
            let event = event.map_err(|e| format!("{label} first stream: {e}"))?;
            match event {
                AgentEvent::SessionStarted { session_id: id, .. } => {
                    if !id.is_empty() {
                        session_id = Some(id);
                    }
                }
                AgentEvent::TextDelta { .. } | AgentEvent::ReasoningDelta { .. } if !saw_text => {
                    saw_text = true;
                    interrupt.cancel();
                }
                AgentEvent::Done {
                    status,
                    session_id: done_id,
                    error,
                    ..
                } => {
                    if let Some(id) = done_id.filter(|s| !s.is_empty()) {
                        session_id = Some(id);
                    }
                    return Ok(status == DoneStatus::Interrupted
                        || matches!(status, DoneStatus::Completed if saw_text)
                        || error.is_some());
                }
                _ => {}
            }
        }
        Err(format!("{label} first run ended without Done"))
    })
    .await
    .map_err(|_| format!("{label} first run timed out"))??;
    if !first_done && !saw_text {
        return Err(format!("{label} produced no output before interrupt"));
    }
    let session_id =
        session_id.ok_or_else(|| format!("{label} did not report a harness session id"))?;

    let interrupt = CancellationToken::new();
    let mut second = request(RESUME_INTERRUPTED_PROMPT, model, reasoning, &cwd);
    second.resume = Some(session_id);
    second.resume_interrupted = true;
    let mut stream = harness
        .run(second, controls(interrupt))
        .await
        .map_err(|e| format!("{label} resume start: {e}"))?;
    let mut text = String::new();
    let resumed = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(event) = stream.next().await {
            let event = event.map_err(|e| format!("{label} resume stream: {e}"))?;
            match event {
                AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                AgentEvent::Done { status, error, .. } => {
                    if status != DoneStatus::Completed {
                        return Err(format!(
                            "{label} resume ended {status:?} error={error:?} text={text:?}"
                        ));
                    }
                    if text.trim().is_empty() {
                        return Err(format!("{label} resume completed with empty text"));
                    }
                    return Ok(text);
                }
                AgentEvent::Error { message } => {
                    return Err(format!("{label} resume error: {message}"));
                }
                _ => {}
            }
        }
        Err(format!("{label} resume ended without Done; text={text:?}"))
    })
    .await
    .map_err(|_| format!("{label} resume timed out"))?;
    resumed
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn grok_codex_cursor_pi_resume_after_interrupt() {
    let cases: Vec<(&str, Box<dyn Harness>, &str, Option<ReasoningLevel>)> = vec![
        (
            "grok",
            Box::new(AcpHarness::grok()),
            "grok-4.6",
            Some(ReasoningLevel::Low),
        ),
        (
            "codex",
            Box::new(CodexHarness::new()),
            "gpt-5.6-luna",
            Some(ReasoningLevel::Low),
        ),
        (
            "cursor",
            Box::new(CursorHarness::new()),
            "composer-2.5",
            None,
        ),
        (
            "pi",
            Box::new(AcpHarness::pi()),
            "glm-5.3-flash",
            Some(ReasoningLevel::Low),
        ),
    ];
    let mut failures = Vec::new();
    for (label, harness, model, reasoning) in cases {
        match interrupt_then_resume(label, harness.as_ref(), model, reasoning).await {
            Ok(text) => eprintln!(
                "{label} resume ok ({} chars): {}",
                text.len(),
                text.chars().take(120).collect::<String>()
            ),
            Err(err) => {
                eprintln!("{label} FAILED: {err}");
                failures.push(format!("{label}: {err}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "resume failed for: {}",
        failures.join(" | ")
    );
}
