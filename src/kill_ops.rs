//! How a confirmed kill runs: handed off by the popup, executed in a process of its own.
//!
//! ```text
//!  popup (dies with its tab)             detached executor (own session)
//!  ─────────────────────────             ─────────────────────────────────────────
//!  y on review ── spawn_executor ──▶     herdr-ferry execute-kill
//!  exits at once  (setsid; plan JSON     ├─ re-read topology, re-check cascades
//!                  in HERDR_FERRY_       ├─ close each target still as reviewed
//!                  KILL_PLAN)            └─ one notification with the outcome
//! ```
//!
//! Herdr closes a popup as soon as the tab that owns it disappears, and closing any pane
//! signals every process in that pane's terminal session (HUP, TERM, then KILL). A kill that
//! includes Ferry's own tab or workspace would stop an in-popup loop half way. The executor
//! calls `setsid` before exec, so no pane teardown reaches it and every confirmed close runs.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};

use crate::herdr::{CloseOutcome, Herdr};
use crate::kill::{KillPlan, KillScope, KillTarget, TargetState};

/// Carries the confirmed plan, as JSON, from the popup to the executor.
const PLAN_ENV: &str = "HERDR_FERRY_KILL_PLAN";

/// What happened to each confirmed target, by label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KillReport {
    pub scope: KillScope,
    pub killed: Vec<String>,
    pub gone: usize,
    pub changed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

impl KillReport {
    fn new(scope: KillScope) -> Self {
        Self {
            scope,
            killed: Vec::new(),
            gone: 0,
            changed: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// One line for a Herdr notification, e.g. `Killed 2 panes · 1 pane already gone`.
    pub fn message(&self) -> String {
        let noun = self.scope.noun();
        let mut parts = vec![match self.killed.as_slice() {
            [] => "Nothing killed".to_string(),
            [only] => format!("Killed {noun} “{only}”"),
            killed => format!("Killed {}", counted(killed.len(), noun)),
        }];
        if self.gone > 0 {
            parts.push(format!("{} already gone", counted(self.gone, noun)));
        }
        parts.extend(
            self.changed
                .iter()
                .map(|label| format!("skipped “{label}”: it changed after review")),
        );
        parts.extend(
            self.failed
                .iter()
                .map(|(label, error)| format!("“{label}” failed: {error}")),
        );
        parts.join(" · ")
    }
}

/// Closes every target that is still what the user reviewed.
///
/// The topology is read once: Herdr IDs are allocated, not positional, so closing one target
/// never renames another. A failure is recorded and the remaining targets still close. Killed
/// processes cannot come back, so there is no rollback.
pub fn execute(herdr: &Herdr, plan: &KillPlan) -> Result<KillReport> {
    let topology = herdr.topology()?;
    plan.check_cascade(&topology)?;
    let mut report = KillReport::new(plan.scope);
    for target in plan.execution_order(&topology) {
        match plan.state_of(target, &topology) {
            TargetState::Gone => report.gone += 1,
            TargetState::Changed => report.changed.push(target.label.clone()),
            TargetState::Live => match close(herdr, plan.scope, target) {
                Ok(CloseOutcome::Closed) => report.killed.push(target.label.clone()),
                Ok(CloseOutcome::Missing) => report.gone += 1,
                Err(error) => report
                    .failed
                    .push((target.label.clone(), format!("{error:#}"))),
            },
        }
    }
    Ok(report)
}

/// Starts `<ferry> execute-kill` outside the caller's terminal session.
///
/// The child gets the plan in its environment, the caller's Herdr binary, and no stdio. The
/// popup never waits for it; the returned handle exists so tests can.
pub fn spawn_executor(ferry: &Path, herdr: &Herdr, plan: &KillPlan) -> Result<Child> {
    let plan = serde_json::to_string(plan).context("failed to encode the kill plan")?;
    let mut command = Command::new(ferry);
    command
        .arg("execute-kill")
        .env(PLAN_ENV, plan)
        .env("HERDR_BIN_PATH", herdr.binary())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the hook runs between fork and exec and only calls setsid, which is
    // async-signal-safe and touches no state shared with the parent.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .with_context(|| format!("could not start {}", ferry.display()))
}

/// Entry point of `herdr-ferry execute-kill`: runs the handed-off plan and notifies the result.
pub fn run_from_environment() -> Result<()> {
    let herdr = Herdr::from_environment();
    let result = (|| {
        let plan = std::env::var(PLAN_ENV).with_context(|| format!("{PLAN_ENV} is not set"))?;
        let plan = serde_json::from_str::<KillPlan>(&plan).context("invalid kill plan")?;
        execute(&herdr, &plan)
    })();
    let message = match &result {
        Ok(report) => report.message(),
        Err(error) => format!("Kill failed: {error:#}"),
    };
    let _ = herdr.notify(&message);
    result.map(|_| ())
}

fn close(herdr: &Herdr, scope: KillScope, target: &KillTarget) -> Result<CloseOutcome> {
    match scope {
        KillScope::Panes => herdr.close_pane(&target.id),
        KillScope::Tabs => herdr.close_tab(&target.id),
        KillScope::Workspaces => herdr.close_workspace(&target.id),
    }
}

fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}
