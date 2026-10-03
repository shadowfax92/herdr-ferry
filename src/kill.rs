//! What a Ferry kill is: which kind of container it closes.
//!
//! The popup (`app`) decides *what* to kill; this module owns the vocabulary both the popup
//! and the detached executor share, so neither has to re-derive what a selection means.

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::herdr::{Topology, WorkspaceInfo};

/// The kind of Herdr container a kill closes. Every target in one kill has the same scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KillScope {
    Panes,
    Tabs,
    Workspaces,
}

impl KillScope {
    /// Singular noun for headings and reports, e.g. "pane".
    pub fn noun(self) -> &'static str {
        match self {
            Self::Panes => "pane",
            Self::Tabs => "tab",
            Self::Workspaces => "workspace",
        }
    }
}

/// Exactly what the user reviewed and confirmed: one scope and its targets in chosen order.
///
/// It is also the handoff contract: the popup serializes it for the detached executor
/// (`kill_ops`), which must close nothing outside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillPlan {
    pub scope: KillScope,
    pub targets: Vec<KillTarget>,
}

/// One pane, tab, or workspace to close, with the panes the review showed inside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillTarget {
    pub id: String,
    pub label: String,
    /// Every pane inside the target at review time. A tab or workspace that later holds a pane
    /// outside this list is no longer what the user confirmed and must not be closed.
    pub pane_ids: Vec<String>,
}

/// Where a confirmed target stands when the executor re-reads Herdr.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetState {
    /// Present, holding only panes the review showed: safe to close.
    Live,
    /// Already closed by someone else.
    Gone,
    /// Holds a pane the review never showed; closing it would kill unconfirmed work.
    Changed,
}

impl KillPlan {
    /// Resolves `(id, label)` choices, in the order the user picked them, against the topology
    /// they reviewed. Fails when a target is gone or the kill would cascade (see
    /// [`KillPlan::check_cascade`]).
    pub fn build(
        scope: KillScope,
        chosen: Vec<(String, String)>,
        topology: &Topology,
    ) -> Result<Self> {
        if chosen.is_empty() {
            bail!("select at least one {}", scope.noun());
        }
        let targets = chosen
            .into_iter()
            .map(|(id, label)| {
                let pane_ids = panes_in(scope, &id, topology).with_context(|| {
                    format!("the selected {} {id} no longer exists", scope.noun())
                })?;
                Ok(KillTarget {
                    id,
                    label,
                    pane_ids,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let plan = Self { scope, targets };
        plan.check_cascade(topology)?;
        Ok(plan)
    }

    /// Refuses kills that would make Herdr close worktree workspaces nobody selected.
    ///
    /// Closing a worktree root (the non-linked checkout) closes every workspace of its
    /// repository: `workspace close` refuses without a group flag, and closing the root's last
    /// tab or pane cascades silently when Herdr's `confirm_close` is off. The review must list
    /// everything that dies, so emptying a root while a linked workspace survives is refused.
    pub fn check_cascade(&self, topology: &Topology) -> Result<()> {
        let emptied = self.emptied_workspaces(&self.targets, topology);
        let Some((root, linked)) = cascade(&emptied, topology) else {
            return Ok(());
        };
        let pronoun = if linked.len() == 1 { "it" } else { "them" };
        bail!(
            "killing “{}” would also close its {}; select {pronoun} too",
            workspace_name(root),
            linked_worktrees(&linked)
        );
    }

    /// Why closing `target` now would take linked worktree workspaces down with it, if so.
    ///
    /// The executor asks right before each close inside a worktree root, against topology read
    /// after the closes before it. A linked workspace whose own target was skipped or failed is
    /// still open then, and closing the root's last tab or pane would cascade into it whenever
    /// Herdr's `confirm_close` is off, killing panes nobody confirmed.
    pub fn cascade_reason(&self, target: &KillTarget, topology: &Topology) -> Option<String> {
        let emptied = self.emptied_workspaces(std::slice::from_ref(target), topology);
        let (_, linked) = cascade(&emptied, topology)?;
        Some(format!("it would also close {}", linked_worktrees(&linked)))
    }

    /// Compares a target with fresh topology. Panes that left a tab or workspace since the
    /// review are fine (less dies); a pane that arrived is not.
    pub fn state_of(&self, target: &KillTarget, topology: &Topology) -> TargetState {
        let Some(live) = panes_in(self.scope, &target.id, topology) else {
            return TargetState::Gone;
        };
        if live.iter().all(|pane_id| target.pane_ids.contains(pane_id)) {
            TargetState::Live
        } else {
            TargetState::Changed
        }
    }

    /// Targets in closing order: the chosen order, except that targets inside a worktree root
    /// go last. Linked worktrees then close first, so each root closes on its own instead of
    /// tripping Herdr's group-close refusal.
    pub fn execution_order<'a>(&'a self, topology: &Topology) -> Vec<&'a KillTarget> {
        let mut targets = self.targets.iter().collect::<Vec<_>>();
        targets.sort_by_key(|target| self.in_worktree_root(target, topology));
        targets
    }

    /// Whether `target` lies inside a worktree root, whose closing can cascade into the
    /// workspaces linked to it.
    pub fn in_worktree_root(&self, target: &KillTarget, topology: &Topology) -> bool {
        let workspace_id = match self.scope {
            KillScope::Workspaces => Some(target.id.as_str()),
            KillScope::Tabs => topology
                .tabs
                .iter()
                .find(|tab| tab.tab_id == target.id)
                .map(|tab| tab.workspace_id.as_str()),
            KillScope::Panes => topology
                .panes
                .iter()
                .find(|pane| pane.pane_id == target.id)
                .map(|pane| pane.workspace_id.as_str()),
        };
        topology
            .workspaces
            .iter()
            .filter(|workspace| Some(workspace.workspace_id.as_str()) == workspace_id)
            .any(|workspace| {
                workspace
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| !worktree.is_linked_worktree)
            })
    }

    /// Workspaces that closing `targets` leaves without any pane. Herdr closes those as well.
    fn emptied_workspaces<'a>(
        &self,
        targets: &[KillTarget],
        topology: &'a Topology,
    ) -> HashSet<&'a str> {
        let ids = targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<HashSet<_>>();
        topology
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.as_str())
            .filter(|workspace_id| match self.scope {
                KillScope::Workspaces => ids.contains(workspace_id),
                KillScope::Tabs => all_selected(
                    topology
                        .tabs
                        .iter()
                        .filter(|tab| tab.workspace_id == *workspace_id)
                        .map(|tab| tab.tab_id.as_str())
                        .collect(),
                    &ids,
                ),
                KillScope::Panes => all_selected(
                    topology
                        .panes
                        .iter()
                        .filter(|pane| pane.workspace_id == *workspace_id)
                        .map(|pane| pane.pane_id.as_str())
                        .collect(),
                    &ids,
                ),
            })
            .collect()
    }
}

/// Pane IDs currently inside the pane, tab, or workspace `id`; `None` once it is gone.
pub(crate) fn panes_in(scope: KillScope, id: &str, topology: &Topology) -> Option<Vec<String>> {
    let exists = match scope {
        KillScope::Panes => topology.panes.iter().any(|pane| pane.pane_id == id),
        KillScope::Tabs => topology.tabs.iter().any(|tab| tab.tab_id == id),
        KillScope::Workspaces => topology
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == id),
    };
    if !exists {
        return None;
    }
    let panes = topology
        .panes
        .iter()
        .filter(|pane| match scope {
            KillScope::Panes => pane.pane_id == id,
            KillScope::Tabs => pane.tab_id == id,
            KillScope::Workspaces => pane.workspace_id == id,
        })
        .map(|pane| pane.pane_id.clone())
        .collect();
    Some(panes)
}

/// The first emptied worktree root that still has open linked workspaces, with those
/// workspaces: Herdr would close them together with the root.
fn cascade<'a>(
    emptied: &HashSet<&str>,
    topology: &'a Topology,
) -> Option<(&'a WorkspaceInfo, Vec<&'a WorkspaceInfo>)> {
    topology
        .workspaces
        .iter()
        .filter(|workspace| emptied.contains(workspace.workspace_id.as_str()))
        .find_map(|root| {
            let group = root
                .worktree
                .as_ref()
                .filter(|worktree| !worktree.is_linked_worktree)?;
            let linked = topology
                .workspaces
                .iter()
                .filter(|other| {
                    !emptied.contains(other.workspace_id.as_str())
                        && other
                            .worktree
                            .as_ref()
                            .is_some_and(|worktree| worktree.repo_key == group.repo_key)
                })
                .collect::<Vec<_>>();
            (!linked.is_empty()).then_some((root, linked))
        })
}

/// e.g. `linked worktree “feature”` or `linked worktrees “a”, “b”`.
fn linked_worktrees(linked: &[&WorkspaceInfo]) -> String {
    let names = linked
        .iter()
        .map(|workspace| format!("“{}”", workspace_name(workspace)))
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if linked.len() == 1 {
        "worktree"
    } else {
        "worktrees"
    };
    format!("linked {noun} {names}")
}

fn all_selected(members: Vec<&str>, selected: &HashSet<&str>) -> bool {
    !members.is_empty() && members.iter().all(|member| selected.contains(member))
}

fn workspace_name(workspace: &WorkspaceInfo) -> &str {
    if workspace.label.trim().is_empty() {
        &workspace.workspace_id
    } else {
        &workspace.label
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{PaneInfo, TabInfo, WorkspaceInfo, WorktreeInfo};

    fn workspace(id: &str, label: &str, worktree: Option<(&str, bool)>) -> WorkspaceInfo {
        WorkspaceInfo {
            workspace_id: id.into(),
            label: label.into(),
            number: 0,
            tab_count: 0,
            pane_count: 0,
            focused: false,
            worktree: worktree.map(|(repo_key, is_linked_worktree)| WorktreeInfo {
                repo_key: repo_key.into(),
                is_linked_worktree,
            }),
        }
    }

    fn tab(tab_id: &str, workspace_id: &str) -> TabInfo {
        TabInfo {
            tab_id: tab_id.into(),
            workspace_id: workspace_id.into(),
            label: String::new(),
            number: 0,
            pane_count: 0,
            focused: false,
        }
    }

    fn pane(pane_id: &str, tab_id: &str) -> PaneInfo {
        PaneInfo {
            pane_id: pane_id.into(),
            tab_id: tab_id.into(),
            workspace_id: tab_id.split(':').next().unwrap().into(),
            label: None,
            terminal_title_stripped: None,
            cwd: None,
            agent: None,
            agent_status: "unknown".into(),
            focused: false,
        }
    }

    /// `repo` is a worktree root with one linked worktree, `feature`; `plain` is no worktree.
    fn topology() -> Topology {
        Topology {
            workspaces: vec![
                workspace("w1", "repo", Some(("k", false))),
                workspace("w2", "feature", Some(("k", true))),
                workspace("w3", "plain", None),
            ],
            tabs: vec![
                tab("w1:t1", "w1"),
                tab("w1:t2", "w1"),
                tab("w2:t1", "w2"),
                tab("w3:t1", "w3"),
            ],
            panes: vec![
                pane("w1:p1", "w1:t1"),
                pane("w1:p2", "w1:t1"),
                pane("w1:p3", "w1:t2"),
                pane("w2:p1", "w2:t1"),
                pane("w3:p1", "w3:t1"),
            ],
        }
    }

    fn chosen(ids: &[&str]) -> Vec<(String, String)> {
        ids.iter()
            .map(|id| (id.to_string(), format!("label {id}")))
            .collect()
    }

    fn pane_ids(plan: &KillPlan) -> Vec<Vec<&str>> {
        plan.targets
            .iter()
            .map(|target| target.pane_ids.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn tests_that_plans_capture_the_panes_inside_each_target() {
        let tabs = KillPlan::build(KillScope::Tabs, chosen(&["w1:t1", "w3:t1"]), &topology());
        assert_eq!(
            pane_ids(&tabs.unwrap()),
            [vec!["w1:p1", "w1:p2"], vec!["w3:p1"]]
        );

        let workspaces = KillPlan::build(KillScope::Workspaces, chosen(&["w3"]), &topology());
        assert_eq!(pane_ids(&workspaces.unwrap()), [vec!["w3:p1"]]);

        let panes = KillPlan::build(KillScope::Panes, chosen(&["w1:p3"]), &topology()).unwrap();
        assert_eq!(
            panes,
            KillPlan {
                scope: KillScope::Panes,
                targets: vec![KillTarget {
                    id: "w1:p3".into(),
                    label: "label w1:p3".into(),
                    pane_ids: vec!["w1:p3".into()],
                }],
            }
        );
    }

    #[test]
    fn tests_that_targets_keep_the_order_they_were_chosen() {
        let plan = KillPlan::build(KillScope::Panes, chosen(&["w3:p1", "w1:p1"]), &topology());

        let ids = plan
            .unwrap()
            .targets
            .into_iter()
            .map(|target| target.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["w3:p1", "w1:p1"]);
    }

    #[test]
    fn tests_that_missing_or_empty_selections_are_rejected() {
        let missing = KillPlan::build(KillScope::Tabs, chosen(&["w9:t9"]), &topology());

        assert!(missing.unwrap_err().to_string().contains("w9:t9"));
        assert!(KillPlan::build(KillScope::Panes, Vec::new(), &topology()).is_err());
    }

    #[test]
    fn tests_that_emptying_a_worktree_root_with_live_linked_workspaces_is_refused() {
        for (scope, ids) in [
            (KillScope::Workspaces, vec!["w1"]),
            (KillScope::Tabs, vec!["w1:t1", "w1:t2"]),
            (KillScope::Panes, vec!["w1:p1", "w1:p2", "w1:p3"]),
        ] {
            let error = KillPlan::build(scope, chosen(&ids), &topology())
                .unwrap_err()
                .to_string();

            assert!(error.contains("“repo”"), "{error}");
            assert!(error.contains("“feature”"), "{error}");
        }
    }

    #[test]
    fn tests_that_worktree_groups_can_be_killed_whole_or_from_the_leaves() {
        for (scope, ids) in [
            (KillScope::Workspaces, vec!["w1", "w2"]),
            (KillScope::Workspaces, vec!["w2"]),
            (KillScope::Tabs, vec!["w1:t1", "w1:t2", "w2:t1"]),
            (KillScope::Panes, vec!["w1:p1"]),
        ] {
            assert!(KillPlan::build(scope, chosen(&ids), &topology()).is_ok());
        }
    }
}
