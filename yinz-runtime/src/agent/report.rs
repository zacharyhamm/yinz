//! Local hook reports and privacy-safe, monotonic diagnostics shared with the remote reporter.
use crate::errors::prelude::*;
use std::time::{Duration, Instant};
#[cfg(unix)]
use std::{
    io::Write,
    sync::{Arc, Mutex},
};
use yinz_types::agent::report::{AgentReport, AgentReportState};
use yinz_types::internal_ids::{ProcessId, SessionName};
use yinz_types::pane_id::PaneId;

pub const REPORT_DEADLINE: Duration = Duration::from_secs(1);
pub const SLOW_REPORT: Duration = Duration::from_millis(100);

pub const ROUTE_ENV: &[&str] = &[
    "YINZ_PANE_REF",
    "YINZ_PANE_ID",
    crate::envs::SESSION_NAME_ENV_KEY,
    super::pi::PANE_TOKEN_ENV,
    crate::envs::SOCKET_DIR_ENV_KEY,
];

/// The routing variables this process inherited from its pane.
pub fn route_env_from_process() -> std::collections::HashMap<String, String> {
    ROUTE_ENV
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).into(), value)))
        .collect()
}

/// Route only through the bound pane's environment, never the reporter's inherited pane.
pub fn forwarding_command(
    yinz: &std::path::Path,
    kind: &str,
    env: &std::collections::HashMap<String, String>,
) -> std::process::Command {
    let mut command = std::process::Command::new(yinz);
    for key in ROUTE_ENV {
        command.env_remove(key);
    }
    if env
        .get(super::pi::PANE_TOKEN_ENV)
        .is_some_and(|token| !token.is_empty())
    {
        command.args(["--pty-broker", "report", kind]);
    } else {
        command.args(["action", "agent-report", "--hook-json", "--kind", kind]);
    }
    command
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null());
    command
}

pub fn validate_report(report: &AgentReport) -> Result<()> {
    if uuid::Uuid::parse_str(&report.report_id).is_err() {
        return Err(anyhow!("invalid report id"));
    }
    if let Some(event) = &report.event
        && (report.pane_id != PaneId::Terminal(event.pane_id)
            || event.raw.len() > super::HOOK_BROADCAST_MAX_BYTES)
    {
        return Err(anyhow!("invalid report event target or size"));
    }
    Ok(())
}

pub fn report_diagnostics(report: &AgentReport, session_name: &SessionName) -> ReportTrace {
    let mut trace = ReportTrace::new(Instant::now());
    trace.report_id = report.report_id.clone();
    trace.process_id = report.process_id;
    trace.enabled = report.trace;
    trace.kind = report.kind.clone();
    trace.pane = format!("{:?}", report.pane_id);
    trace.session = session_name.to_string();
    if let AgentReportState::Set(agent) = &report.state {
        trace.kind = agent.kind.clone();
    }
    if let Some(event) = &report.event {
        trace.kind = event.kind.clone();
        trace.event = event.hook.clone();
        trace.payload_bytes = event.raw.len();
    }
    trace
}

/// Contains identifiers only: never put hook bodies, cwd, errors, or remote tokens here.
#[derive(Clone, Debug)]
pub struct ReportTrace {
    pub report_id: String,
    pub process_id: ProcessId,
    pub enabled: bool,
    pub kind: String,
    pub event: String,
    pub session: String,
    pub pane: String,
    pub payload_bytes: usize,
    pub started: Instant,
    stage_started: Instant,
    stage: &'static str,
    completed: Vec<(&'static str, Duration)>,
    finished: bool,
}

impl ReportTrace {
    pub fn new(started: Instant) -> Self {
        Self {
            report_id: uuid::Uuid::new_v4().to_string(),
            process_id: ProcessId::new(std::process::id()),
            enabled: std::env::var("YINZ_HOOK_TRACE").as_deref() == Ok("1"),
            kind: String::new(),
            event: String::new(),
            session: String::new(),
            pane: String::new(),
            payload_bytes: 0,
            started,
            stage_started: started,
            stage: "startup",
            completed: vec![],
            finished: false,
        }
    }

    /// Reserve the report's single final outcome. The reporter and the deadline observer race
    /// to finish; only the first claim may record totals.
    pub fn claim_finish(&mut self) -> bool {
        !std::mem::replace(&mut self.finished, true)
    }

    pub fn enter(&mut self, stage: &'static str) {
        self.record(self.stage, self.stage_started.elapsed(), "ok");
        self.completed
            .push((self.stage, self.stage_started.elapsed()));
        self.stage = stage;
        self.stage_started = Instant::now();
        if self.enabled {
            self.record(stage, Duration::ZERO, "enter");
        }
    }

    pub fn finish(&self, success: bool) {
        let outcome = if success { "ok" } else { "error" };
        self.record(self.stage, self.stage_started.elapsed(), outcome);
        if !success {
            for (stage, duration) in &self.completed {
                self.record(stage, *duration, "report_failed");
            }
        }
        self.record("total", self.started.elapsed(), outcome);
    }

    pub fn record(&self, stage: &str, duration: Duration, outcome: &str) {
        if !self.enabled && duration < SLOW_REPORT && (outcome == "ok" || outcome == "enter") {
            return;
        }
        // Bound and escape every caller-controlled label. No raw diagnostic error strings:
        // those can contain arguments, tokens, and JSON snippets.
        fn label(s: &str) -> String {
            s.chars()
                .take(128)
                .map(|c| {
                    if c.is_ascii_alphanumeric() || "_-.:".contains(c) {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        }
        let line = format!("hook report={} pid={} reporter_pid={} kind={} event={} session={} pane={} bytes={} stage={} duration_us={} outcome={}\n",
            label(&self.report_id), std::process::id(), self.process_id, label(&self.kind), label(&self.event),
            label(&self.session), label(&self.pane), self.payload_bytes, stage, duration.as_micros(), outcome);
        append_diagnostic(&line);
    }
}

#[cfg(not(unix))]
fn append_diagnostic(_line: &str) {}

#[cfg(unix)]
fn append_diagnostic(line: &str) {
    use std::os::unix::fs::OpenOptionsExt;
    if crate::shared::ensure_secure_dir(&crate::consts::YINZ_TMP_LOG_DIR).is_err() {
        return;
    }
    // O_NONBLOCK prevents opening a substituted FIFO from hanging. The watchdog also bounds
    // regular filesystem writes (which O_NONBLOCK cannot bound).
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(&*crate::consts::YINZ_TMP_LOG_FILE)
        && file.metadata().map(|m| m.is_file()).unwrap_or(false)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Run one routed hook report under the process deadline and record its outcome once.
///
/// Process-only, like [`reporting_watchdog`]: never call this in a server or an in-process test.
#[cfg(unix)]
pub fn traced_report(
    kind: &str,
    report: impl FnOnce(&Mutex<ReportTrace>) -> Result<()>,
) -> Result<()> {
    let trace = Arc::new(Mutex::new(ReportTrace::new(Instant::now())));
    if let Ok(mut trace) = trace.lock() {
        trace.kind = kind.to_owned();
    }
    reporting_watchdog(trace.clone()).context("arm the hook report deadline")?;
    let result = report(&trace);
    if let Ok(mut trace) = trace.lock()
        && trace.claim_finish()
    {
        trace.finish(result.is_ok());
    }
    result
}

/// Process-only guard: never arm this in a server or an in-process test. The hard-exit thread
/// does no logging and takes no locks, so even a stuck diagnostic write cannot hold the hook.
#[cfg(unix)]
pub fn reporting_watchdog(trace: Arc<Mutex<ReportTrace>>) -> Result<()> {
    let deadline = trace
        .lock()
        .map_err(|_| anyhow!("report trace poisoned"))?
        .started
        + REPORT_DEADLINE;
    std::thread::Builder::new()
        .name("hook-deadline".into())
        .spawn(move || {
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            // No destructors, logger flushing, or atexit handlers on the failure bound.
            unsafe { libc::_exit(1) }
        })
        .context("start hook deadline")?;
    // A separate observer records a stalled stage at the slow threshold, without delaying
    // termination if the log filesystem itself stalls.
    std::thread::Builder::new()
        .name("hook-slow".into())
        .spawn(move || {
            std::thread::sleep(
                (deadline - REPORT_DEADLINE + SLOW_REPORT)
                    .saturating_duration_since(Instant::now()),
            );
            let snapshot = trace.try_lock().ok().map(|t| t.clone());
            if let Some(t) = snapshot {
                t.record(t.stage, t.stage_started.elapsed(), "in_progress");
            }
            // Reserve the final 10ms for best-effort timeout diagnostics. The independent
            // hard-exit thread still terminates at one second if this write blocks.
            std::thread::sleep(
                (deadline - Duration::from_millis(10)).saturating_duration_since(Instant::now()),
            );
            let snapshot = trace
                .try_lock()
                .ok()
                .and_then(|mut t| t.claim_finish().then(|| t.clone()));
            if let Some(t) = snapshot {
                t.record(t.stage, t.stage_started.elapsed(), "timeout");
                t.finish(false);
            }
            unsafe { libc::_exit(1) }
        })
        .context("start hook observer")?;
    Ok(())
}

#[cfg(test)]
#[path = "../unit/agent_report_tests.rs"]
mod tests;
