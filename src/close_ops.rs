//! Executes a confirmed snapshot, never a newly expanded selection. Herdr
//! removes empty tabs/workspaces as their last panes close; refusing group
//! closure stays a failure, not permission to broaden the target set.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::close_plan::{CloseBackend, ClosePlan};
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
                "; keeper shell preserved"
            } else {
                ""
            }
        )
    }
}

pub fn execute(backend: &impl CloseBackend, plan: &ClosePlan) -> CloseReport {
    let mut report = CloseReport::default();
    let mut completed = BTreeSet::new();
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
