//! A reviewed snapshot is the authority for destructive work. Names only find
//! candidates; execution uses canonical pane/terminal identities and never
//! expands the confirmed descendant set when the live layout changes.
use std::collections::BTreeSet;

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::herdr::{PaneInfo, TabInfo, Topology, WorkspaceInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloseKind {
    Panes,
    Tabs,
    Workspaces,
    Clear,
}

impl CloseKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Panes => "Close panes",
            Self::Tabs => "Close tabs",
            Self::Workspaces => "Close workspaces",
            Self::Clear => "Clear workspace",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseRequest {
    pub kind: CloseKind,
    pub ids: Vec<String>,
}

/// Only stable process identity is compared. Cwd, title and agent activity can
/// change without replacing the reviewed foreground program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pane_id: String,
    pub shell_pid: Option<u32>,
    pub foreground_process_group_id: Option<u32>,
    #[serde(default)]
    pub foreground_processes: Vec<Process>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub name: String,
}

impl ProcessInfo {
    pub fn normalize(mut self) -> Self {
        self.foreground_processes.sort();
        self
    }

    pub fn description(&self) -> String {
        if self.foreground_processes.is_empty() {
            return format!("shell PID {:?}; foreground unavailable", self.shell_pid);
        }
        self.foreground_processes
            .iter()
            .map(|p| format!("{} ({})", p.name, p.pid))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The driver seam keeps snapshot/revalidation policy independent of CLI I/O.
/// Implementations must address this session and return canonical live IDs.
pub trait CloseBackend {
    fn topology(&self) -> Result<Topology>;
    fn process_info(&self, pane_id: &str) -> Result<ProcessInfo>;
    fn close_pane(&self, pane_id: &str) -> Result<()>;
    fn create_keeper(&self, workspace_id: &str, cwd: &str) -> Result<PaneInfo>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewedPane {
    pub pane: PaneInfo,
    pub process: ProcessInfo,
}

/// Immutable authorization passed across the popup/server-worker boundary.
/// `tabs` and `workspaces` retain review labels, but identity uses IDs only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClosePlan {
    pub kind: CloseKind,
    pub panes: Vec<ReviewedPane>,
    pub tabs: Vec<TabInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub caller_pane_id: String,
    pub caller_tab_id: Option<String>,
    pub keeper_cwd: Option<String>,
}

impl ClosePlan {
    pub fn capture(
        backend: &impl CloseBackend,
        request: CloseRequest,
        caller: &str,
        home: &str,
    ) -> Result<Self> {
        let topology = backend.topology()?;
        let ids: BTreeSet<_> = request.ids.iter().collect();
        ensure!(!ids.is_empty(), "Select at least one target");
        ensure!(
            request.kind != CloseKind::Clear || ids.len() == 1,
            "Clear requires exactly one workspace"
        );
        for id in &ids {
            let found = match request.kind {
                CloseKind::Panes => topology.panes.iter().any(|p| &p.pane_id == *id),
                CloseKind::Tabs => topology.tabs.iter().any(|t| &t.tab_id == *id),
                CloseKind::Workspaces | CloseKind::Clear => {
                    topology.workspaces.iter().any(|w| &w.workspace_id == *id)
                }
            };
            ensure!(
                found,
                "Selected target {id} is no longer available; select again"
            );
        }
        let selected: Vec<_> = topology
            .panes
            .iter()
            .filter(|p| match request.kind {
                CloseKind::Panes => ids.contains(&p.pane_id),
                CloseKind::Tabs => ids.contains(&p.tab_id),
                CloseKind::Workspaces | CloseKind::Clear => ids.contains(&p.workspace_id),
            })
            .cloned()
            .collect();
        ensure!(
            !selected.is_empty(),
            "Selected targets have no live terminals; refresh selection"
        );
        let tabs = topology
            .tabs
            .iter()
            .filter(|t| selected.iter().any(|p| p.tab_id == t.tab_id))
            .cloned()
            .collect::<Vec<_>>();
        let workspaces = topology
            .workspaces
            .iter()
            .filter(|w| selected.iter().any(|p| p.workspace_id == w.workspace_id))
            .cloned()
            .collect::<Vec<_>>();
        // The popup carries the source terminal identity across pane moves.
        // Its inherited pane ID can be stale by the time review is requested.
        let caller_pane = topology
            .panes
            .iter()
            .find(|p| p.pane_id == caller || p.terminal_id.as_deref() == Some(caller));
        let caller_tab_id = caller_pane.map(|p| p.tab_id.clone());
        let caller_pane_id = caller_pane
            .map(|p| p.pane_id.clone())
            .unwrap_or_else(|| caller.into());
        let keeper_cwd = (request.kind == CloseKind::Clear).then(|| {
            selected
                .first()
                .and_then(|p| p.cwd.clone())
                .filter(|cwd| !cwd.is_empty())
                .unwrap_or_else(|| home.into())
        });
        let panes = selected
            .into_iter()
            .map(|pane| {
                ensure!(
                    pane.terminal_id.as_deref().is_some_and(|id| !id.is_empty()),
                    "Herdr did not report a terminal identity for {}; cannot safely close it",
                    pane.pane_id
                );
                ensure!(
                    tabs.iter()
                        .any(|t| t.tab_id == pane.tab_id && t.workspace_id == pane.workspace_id),
                    "Topology changed while reading {}; refresh selection",
                    pane.pane_id
                );
                let process = backend.process_info(&pane.pane_id)?.normalize();
                ensure!(
                    process.pane_id == pane.pane_id,
                    "Pane identity changed during process inspection"
                );
                Ok(ReviewedPane { pane, process })
            })
            .collect::<Result<Vec<_>>>()?;
        let plan = Self {
            kind: request.kind,
            panes,
            tabs,
            workspaces,
            caller_pane_id,
            caller_tab_id,
            keeper_cwd,
        };
        plan.validate(backend, &BTreeSet::new(), None)?;
        Ok(plan)
    }

    /// Absence is idempotent only when the terminal is absent everywhere. A
    /// terminal moved under a new pane ID still exists and needs a new review.
    pub fn live_pane<'a>(
        &self,
        reviewed: &ReviewedPane,
        topology: &'a Topology,
    ) -> Result<Option<&'a PaneInfo>> {
        let old = &reviewed.pane;
        let canonical = topology.panes.iter().find(|p| p.pane_id == old.pane_id);
        let terminal = topology
            .panes
            .iter()
            .find(|p| p.terminal_id == old.terminal_id);
        match (canonical, terminal) {
            (None, None) => Ok(None),
            (Some(p), Some(t))
                if p.pane_id == t.pane_id
                    && p.terminal_id == old.terminal_id
                    && p.tab_id == old.tab_id
                    && p.workspace_id == old.workspace_id =>
            {
                Ok(Some(p))
            }
            _ => bail!(
                "Target {} changed identity or membership; review again",
                old.pane_id
            ),
        }
    }

    pub fn validate(
        &self,
        backend: &impl CloseBackend,
        completed: &BTreeSet<String>,
        keeper: Option<&PaneInfo>,
    ) -> Result<Topology> {
        let topology = backend.topology()?;
        if self.kind != CloseKind::Panes {
            for p in &topology.panes {
                if self.tabs.iter().any(|t| t.tab_id == p.tab_id) {
                    ensure!(
                        self.panes.iter().any(|old| old.pane.pane_id == p.pane_id
                            && old.pane.terminal_id == p.terminal_id),
                        "Selected tab {} changed membership; review again",
                        p.tab_id
                    );
                }
            }
        }
        if self.kind == CloseKind::Workspaces {
            for tab in &topology.tabs {
                if self
                    .workspaces
                    .iter()
                    .any(|w| w.workspace_id == tab.workspace_id)
                {
                    ensure!(
                        self.tabs.iter().any(|old| old.tab_id == tab.tab_id),
                        "Selected workspace {} changed membership; review again",
                        tab.workspace_id
                    );
                }
            }
        }
        // Herdr's implicit last-pane close can cascade to linked worktrees
        // when its confirm_close setting is off. Never delegate authorization
        // to that setting: reject a root's full closure while siblings exist.
        if self.kind != CloseKind::Clear {
            for workspace in &topology.workspaces {
                let Some(group) = &workspace.worktree else {
                    continue;
                };
                if group.is_linked_worktree {
                    continue;
                }
                let live = topology
                    .panes
                    .iter()
                    .filter(|p| p.workspace_id == workspace.workspace_id)
                    .collect::<Vec<_>>();
                let closes_root = !live.is_empty()
                    && live
                        .iter()
                        .all(|p| self.panes.iter().any(|old| old.pane.pane_id == p.pane_id));
                let has_siblings = topology.workspaces.iter().any(|w| {
                    w.workspace_id != workspace.workspace_id
                        && w.worktree
                            .as_ref()
                            .is_some_and(|member| member.repo_key == group.repo_key)
                });
                ensure!(!(closes_root && has_siblings), "Closing {} could close linked worktree workspaces. Close linked workspaces first, or use Clear; no group closure was authorized", workspace.workspace_id);
            }
        }
        if let Some(keeper) = keeper {
            ensure!(
                topology.panes.iter().any(|p| p.pane_id == keeper.pane_id
                    && p.terminal_id == keeper.terminal_id
                    && p.tab_id == keeper.tab_id
                    && p.workspace_id == keeper.workspace_id),
                "Keeper shell changed or disappeared; remaining old terminals were preserved"
            );
        }
        for reviewed in &self.panes {
            if completed.contains(&reviewed.pane.pane_id) {
                continue;
            }
            if let Some(pane) = self.live_pane(reviewed, &topology)? {
                let current = backend.process_info(&pane.pane_id)?.normalize();
                ensure!(
                    current == reviewed.process,
                    "Running program in {} changed; review again",
                    pane.pane_id
                );
            }
        }
        Ok(topology)
    }

    pub fn clear_workspace(&self) -> Result<&WorkspaceInfo> {
        ensure!(
            self.kind == CloseKind::Clear && self.workspaces.len() == 1,
            "Invalid Clear plan"
        );
        self.workspaces.first().context("Clear workspace missing")
    }
}
