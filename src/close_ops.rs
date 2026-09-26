//! Executes a confirmed snapshot, never a newly expanded selection. Herdr
//! removes empty tabs/workspaces as their last panes close; refusing group
//! closure stays a failure, not permission to broaden the target set.
use std::collections::BTreeSet;

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

use crate::close_plan::{CloseBackend, CloseKind, ClosePlan};
use crate::herdr::PaneInfo;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CloseReport {
    pub completed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub errors: Vec<String>,
    pub keeper: Option<PaneInfo>,
}

impl CloseReport {
    pub fn summary(&self) -> String {
        format!(
            "{} closed, {} already closed, {} failed{}",
            self.completed,
            self.skipped,
            self.failed,
            if self.keeper.is_some() {
                "; keeper created (see report)"
            } else {
                ""
            }
        )
    }
}

pub fn execute(backend: &impl CloseBackend, plan: &ClosePlan) -> CloseReport {
    let mut report = CloseReport::default();
    let mut completed = BTreeSet::new();
    if plan.kind == CloseKind::Clear {
        match prepare_keeper(backend, plan) {
            Ok(keeper) => report.keeper = keeper,
            Err(error) => {
                report.failed = plan.panes.len();
                report.errors.push(format!(
                    "Clear stopped before closing any old terminals: {error:#}"
                ));
                return report;
            }
        }
    }
    let mut panes = plan.panes.iter().collect::<Vec<_>>();
    // Tab first, then pane: the source terminal is always the final mutation,
    // although the server-owned worker does not depend on that terminal living.
    panes.sort_by_key(|p| {
        (
            Some(&p.pane.tab_id) == plan.caller_tab_id.as_ref(),
            p.pane.pane_id == plan.caller_pane_id,
        )
    });
    for (index, reviewed) in panes.iter().enumerate() {
        let topology = match plan.validate(backend, &completed, report.keeper.as_ref()) {
            Ok(topology) => topology,
            Err(error) => {
                report.failed += panes.len() - index;
                report.errors.push(format!("{error:#}"));
                break;
            }
        };
        // Validation has already distinguished moved/replaced identities from
        // absent terminals, so a disappeared target is a safe idempotent skip.
        if !topology
            .panes
            .iter()
            .any(|p| p.pane_id == reviewed.pane.pane_id)
        {
            report.skipped += 1;
            completed.insert(reviewed.pane.pane_id.clone());
            continue;
        }
        match backend.close_pane(&reviewed.pane.pane_id) {
            Ok(()) => report.completed += 1,
            Err(error) => {
                let gone = backend
                    .topology()
                    .ok()
                    .is_some_and(|t| matches!(plan.live_pane(reviewed, &t), Ok(None)));
                if gone {
                    report.skipped += 1;
                } else {
                    report.failed += 1;
                    report
                        .errors
                        .push(format!("{}: {error:#}", reviewed.pane.pane_id));
                }
            }
        }
        completed.insert(reviewed.pane.pane_id.clone());
    }
    report
}

/// Creation is the only permitted expansion: a fresh, verified shell in the
/// same workspace. Old tabs were frozen at review; new concurrent tabs never
/// enter the close set, and no failure path removes the keeper.
fn prepare_keeper(backend: &impl CloseBackend, plan: &ClosePlan) -> Result<Option<PaneInfo>> {
    let topology = plan.validate(backend, &BTreeSet::new(), None)?;
    if !plan
        .panes
        .iter()
        .any(|old| topology.panes.iter().any(|p| p.pane_id == old.pane.pane_id))
    {
        return Ok(None);
    }
    let workspace = plan.clear_workspace()?;
    let keeper = backend.create_keeper(
        &workspace.workspace_id,
        plan.keeper_cwd.as_deref().unwrap_or(""),
    )?;
    ensure!(
        keeper.workspace_id == workspace.workspace_id
            && keeper
                .terminal_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
            && !plan.tabs.iter().any(|t| t.tab_id == keeper.tab_id)
            && !plan
                .panes
                .iter()
                .any(|p| p.pane.pane_id == keeper.pane_id
                    || p.pane.terminal_id == keeper.terminal_id),
        "Herdr did not return a fresh keeper in the selected workspace"
    );
    // Return the created identity even if an external actor removes it next.
    // The execution loop verifies it before the first and every later close;
    // reporting must retain this identity if that validation fails.
    Ok(Some(keeper))
}
