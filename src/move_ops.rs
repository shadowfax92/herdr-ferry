use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Context, Result};

use crate::app::{MovePaneRequest, MoveTabRequest, PaneDestination, TabDestination};
use crate::herdr::{Herdr, MovedPane, Topology};
use crate::layout::LayoutNode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveSummary {
    pub message: String,
    pub moved_panes: usize,
    pub moved_tabs: usize,
    pub destination_workspace_id: String,
    pub destination_tab_id: String,
}

#[derive(Clone, Debug)]
pub struct Mover {
    herdr: Herdr,
}

#[derive(Clone, Debug)]
struct TabPlan {
    tab_id: String,
    label: String,
    root: LayoutNode,
    pane_count: usize,
}

impl Mover {
    pub fn new(herdr: Herdr) -> Self {
        Self { herdr }
    }

    pub fn move_pane(&self, request: &MovePaneRequest) -> Result<MoveSummary> {
        self.prepare_pane_move(request)?;
        let total = request.sources.len();
        let mut first_destination: Option<MovedPane> = None;
        let mut previous_pane_id = None;

        for (index, source) in request.sources.iter().enumerate() {
            let result =
                if let Some(destination) = &first_destination {
                    self.herdr.move_to_tab(
                        &source.pane_id,
                        &destination.tab_id,
                        request.direction,
                        previous_pane_id.as_deref(),
                        None,
                        false,
                    )
                } else {
                    match &request.destination {
                        PaneDestination::ExistingTab { tab_id } => self.herdr.move_to_tab(
                            &source.pane_id,
                            tab_id,
                            request.direction,
                            None,
                            None,
                            false,
                        ),
                        PaneDestination::NewTab {
                            workspace_id,
                            label,
                        } => self.herdr.move_to_new_tab(
                            &source.pane_id,
                            workspace_id,
                            label.as_deref(),
                            false,
                        ),
                        PaneDestination::NewWorkspace { label } => self
                            .herdr
                            .move_to_new_workspace(&source.pane_id, label.as_deref(), None, false),
                    }
                };
            let moved = match result {
                Ok(moved) => moved,
                Err(error) if index == 0 => {
                    return Err(error.context(format!(
                        "could not move pane 1/{total} ({})",
                        source.pane_id
                    )));
                }
                Err(error) => {
                    return Err(pane_batch_error(index, total, &source.pane_id, error));
                }
            };
            if let Some(destination) = &first_destination {
                if moved.tab_id != destination.tab_id
                    || moved.workspace_id != destination.workspace_id
                {
                    return Err(pane_batch_error(
                        index + 1,
                        total,
                        &source.pane_id,
                        anyhow!("Herdr returned an unexpected destination"),
                    ));
                }
            } else {
                first_destination = Some(moved.clone());
            }
            previous_pane_id = Some(moved.pane_id);
        }

        let destination = first_destination.context("pane selection was empty")?;
        self.herdr
            .focus_workspace(&destination.workspace_id)
            .with_context(|| {
                format!("{total} panes moved, but the destination workspace could not be focused")
            })?;
        self.herdr.focus_tab(&destination.tab_id).with_context(|| {
            format!("{total} panes moved, but the destination tab could not be focused")
        })?;
        Ok(MoveSummary {
            message: if total == 1 {
                format!("Pane moved to {}", destination.tab_id)
            } else {
                format!("{total} panes moved to {}", destination.tab_id)
            },
            moved_panes: total,
            moved_tabs: 0,
            destination_workspace_id: destination.workspace_id,
            destination_tab_id: destination.tab_id,
        })
    }

    pub fn move_tab(&self, request: &MoveTabRequest) -> Result<MoveSummary> {
        let plans = self.prepare_tab_move(request)?;
        let total_tabs = plans.len();
        let total_panes = plans.iter().map(|plan| plan.pane_count).sum::<usize>();
        let mut destination_workspace_id = match &request.destination {
            TabDestination::Workspace { workspace_id } => Some(workspace_id.clone()),
            TabDestination::NewWorkspace { .. } => None,
        };
        let mut first_destination_tab_id = None;
        let mut moved_panes = 0;

        for (completed_tabs, plan) in plans.iter().enumerate() {
            let anchor = plan.root.anchor().to_string();
            let first_result = if let Some(workspace_id) = &destination_workspace_id {
                self.herdr
                    .move_to_new_tab(&anchor, workspace_id, Some(&plan.label), false)
            } else {
                let TabDestination::NewWorkspace { label } = &request.destination else {
                    unreachable!();
                };
                self.herdr.move_to_new_workspace(
                    &anchor,
                    label.as_deref().or(Some(&plan.label)),
                    Some(&plan.label),
                    false,
                )
            };
            let first = match first_result {
                Ok(first) => first,
                Err(error) if moved_panes == 0 => {
                    return Err(error.context(format!(
                        "could not start moving tab 1/{total_tabs} ({})",
                        plan.tab_id
                    )));
                }
                Err(error) => {
                    return Err(tab_batch_error(
                        completed_tabs,
                        total_tabs,
                        moved_panes,
                        total_panes,
                        error,
                    ));
                }
            };

            if let Some(workspace_id) = &destination_workspace_id {
                if first.workspace_id != *workspace_id {
                    return Err(tab_batch_error(
                        completed_tabs,
                        total_tabs,
                        moved_panes + 1,
                        total_panes,
                        anyhow!("Herdr returned an unexpected destination workspace"),
                    ));
                }
            } else {
                destination_workspace_id = Some(first.workspace_id.clone());
            }
            first_destination_tab_id.get_or_insert_with(|| first.tab_id.clone());

            let mut moved_in_tab = 1;
            let mut id_map = HashMap::from([(anchor, first.pane_id.clone())]);
            if let Err(error) =
                self.place_layout(&plan.root, &first.tab_id, &mut id_map, &mut moved_in_tab)
            {
                return Err(tab_batch_error(
                    completed_tabs,
                    total_tabs,
                    moved_panes + moved_in_tab,
                    total_panes,
                    error,
                ));
            }
            moved_panes += moved_in_tab;
        }

        let destination_workspace_id =
            destination_workspace_id.context("tab selection was empty")?;
        let first_destination_tab_id =
            first_destination_tab_id.context("tab selection was empty")?;
        self.herdr
            .focus_workspace(&destination_workspace_id)
            .with_context(|| {
                format!(
                    "{total_tabs} tabs moved, but the destination workspace could not be focused"
                )
            })?;
        self.herdr
            .focus_tab(&first_destination_tab_id)
            .with_context(|| {
                format!("{total_tabs} tabs moved, but the first moved tab could not be focused")
            })?;
        Ok(MoveSummary {
            message: if total_tabs == 1 {
                format!(
                    "Tab “{}” moved to {destination_workspace_id}",
                    plans[0].label
                )
            } else {
                format!("{total_tabs} tabs moved to {destination_workspace_id}")
            },
            moved_panes,
            moved_tabs: total_tabs,
            destination_workspace_id,
            destination_tab_id: first_destination_tab_id,
        })
    }

    fn prepare_pane_move(&self, request: &MovePaneRequest) -> Result<()> {
        if request.sources.is_empty() {
            bail!("select at least one pane");
        }
        let topology = self.herdr.topology()?;
        let mut seen_panes = HashSet::new();
        let mut source_tabs = Vec::new();
        for source in &request.sources {
            if !seen_panes.insert(source.pane_id.as_str()) {
                bail!("pane {} was selected more than once", source.pane_id);
            }
            let pane = topology
                .panes
                .iter()
                .find(|pane| pane.pane_id == source.pane_id)
                .with_context(|| format!("selected pane no longer exists: {}", source.pane_id))?;
            if pane.tab_id != source.expected_tab_id {
                bail!("pane {} changed tabs while Ferry was open", source.pane_id);
            }
            if !source_tabs.contains(&pane.tab_id) {
                source_tabs.push(pane.tab_id.clone());
            }
        }

        match &request.destination {
            PaneDestination::ExistingTab { tab_id } => {
                if source_tabs.contains(tab_id) {
                    bail!("a pane cannot move into one of its source tabs");
                }
                if !topology.tabs.iter().any(|tab| tab.tab_id == *tab_id) {
                    bail!("the destination tab no longer exists");
                }
            }
            PaneDestination::NewTab { workspace_id, .. } => {
                if !topology
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.workspace_id == *workspace_id)
                {
                    bail!("the destination workspace no longer exists");
                }
            }
            PaneDestination::NewWorkspace { .. } => {}
        }

        for tab_id in &source_tabs {
            self.ensure_unzoomed(&topology, tab_id, "source")?;
        }
        if let PaneDestination::ExistingTab { tab_id } = &request.destination {
            self.ensure_unzoomed(&topology, tab_id, "destination")?;
        }
        Ok(())
    }

    fn prepare_tab_move(&self, request: &MoveTabRequest) -> Result<Vec<TabPlan>> {
        if request.tab_ids.is_empty() {
            bail!("select at least one tab");
        }
        let topology = self.herdr.topology()?;
        let mut seen_tabs = HashSet::new();
        let mut selected_tabs = Vec::new();
        for tab_id in &request.tab_ids {
            if !seen_tabs.insert(tab_id.as_str()) {
                bail!("tab {tab_id} was selected more than once");
            }
            let tab = topology
                .tabs
                .iter()
                .find(|tab| tab.tab_id == *tab_id)
                .with_context(|| format!("selected tab no longer exists: {tab_id}"))?;
            selected_tabs.push(tab);
        }

        if let TabDestination::Workspace { workspace_id } = &request.destination {
            if !topology
                .workspaces
                .iter()
                .any(|workspace| workspace.workspace_id == *workspace_id)
            {
                bail!("the destination workspace no longer exists");
            }
            if let Some(tab) = selected_tabs
                .iter()
                .find(|tab| tab.workspace_id == *workspace_id)
            {
                bail!("tab {} is already in the destination workspace", tab.tab_id);
            }
        }

        selected_tabs
            .into_iter()
            .map(|tab| {
                let source_pane_id = topology
                    .panes
                    .iter()
                    .find(|pane| pane.tab_id == tab.tab_id && pane.focused)
                    .or_else(|| topology.panes.iter().find(|pane| pane.tab_id == tab.tab_id))
                    .map(|pane| pane.pane_id.as_str())
                    .with_context(|| format!("tab {} no longer has a movable pane", tab.tab_id))?;
                let snapshot = self.herdr.layout(source_pane_id)?;
                if snapshot.tab_id != tab.tab_id {
                    bail!("tab {} changed while Ferry was open", tab.tab_id);
                }
                if snapshot.zoomed {
                    bail!("unzoom tab {} before moving it", tab.tab_id);
                }
                let root = LayoutNode::from_snapshot(&snapshot)?;
                self.validate_tab_source(&topology, &root, &tab.tab_id)?;
                Ok(TabPlan {
                    tab_id: tab.tab_id.clone(),
                    label: tab_label(&tab.label),
                    pane_count: root.leaves().len(),
                    root,
                })
            })
            .collect()
    }

    fn ensure_unzoomed(&self, topology: &Topology, tab_id: &str, role: &str) -> Result<()> {
        let pane_id = topology
            .panes
            .iter()
            .find(|pane| pane.tab_id == tab_id)
            .map(|pane| pane.pane_id.as_str())
            .with_context(|| format!("the {role} tab no longer has a pane"))?;
        let snapshot = self.herdr.layout(pane_id)?;
        if snapshot.tab_id != tab_id {
            bail!("the {role} tab changed while Ferry was open");
        }
        if snapshot.zoomed {
            bail!("unzoom the {role} tab before moving panes");
        }
        Ok(())
    }

    fn validate_tab_source(
        &self,
        topology: &Topology,
        root: &LayoutNode,
        source_tab_id: &str,
    ) -> Result<()> {
        let expected = topology
            .panes
            .iter()
            .filter(|pane| pane.tab_id == source_tab_id)
            .map(|pane| pane.pane_id.as_str())
            .collect::<HashSet<_>>();
        let leaves = root.leaves();
        let actual = leaves.iter().copied().collect::<HashSet<_>>();
        if leaves.len() == actual.len() && actual == expected {
            return Ok(());
        }
        bail!("the source tab layout changed while Ferry was open")
    }

    fn place_layout(
        &self,
        node: &LayoutNode,
        destination_tab_id: &str,
        id_map: &mut HashMap<String, String>,
        moved_count: &mut usize,
    ) -> Result<()> {
        let LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } = node
        else {
            return Ok(());
        };
        let target = id_map
            .get(node.anchor())
            .cloned()
            .with_context(|| format!("missing placed anchor {}", node.anchor()))?;
        let second_anchor = second.anchor().to_string();
        let moved = self.herdr.move_to_tab(
            &second_anchor,
            destination_tab_id,
            *direction,
            Some(&target),
            Some(*ratio),
            false,
        )?;
        id_map.insert(second_anchor, moved.pane_id);
        *moved_count += 1;
        self.place_layout(first, destination_tab_id, id_map, moved_count)?;
        self.place_layout(second, destination_tab_id, id_map, moved_count)
    }
}

fn tab_label(label: &str) -> String {
    let label = label.trim();
    if label.is_empty() {
        "moved".into()
    } else {
        label.into()
    }
}

fn pane_batch_error(
    moved: usize,
    total: usize,
    source_pane_id: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    anyhow!(
        "pane batch stopped after {moved}/{total} panes while moving {source_pane_id}; all moved panes are still live: {error:#}"
    )
}

fn tab_batch_error(
    completed_tabs: usize,
    total_tabs: usize,
    moved_panes: usize,
    total_panes: usize,
    error: anyhow::Error,
) -> anyhow::Error {
    anyhow!(
        "tab batch stopped after {completed_tabs}/{total_tabs} tabs and {moved_panes}/{total_panes} panes; all moved panes are still live: {error:#}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{PaneInfo, TabInfo, WorkspaceInfo};
    use crate::layout::{LayoutNode, SplitDirection};

    fn topology() -> Topology {
        Topology {
            workspaces: vec![WorkspaceInfo {
                workspace_id: "w1".into(),
                label: "one".into(),
                number: 1,
                tab_count: 1,
                pane_count: 2,
                focused: true,
            }],
            tabs: vec![TabInfo {
                tab_id: "w1:t1".into(),
                workspace_id: "w1".into(),
                label: "main".into(),
                number: 1,
                pane_count: 2,
                focused: true,
            }],
            panes: vec![
                PaneInfo {
                    pane_id: "w1:p1".into(),
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    label: None,
                    terminal_title_stripped: None,
                    cwd: None,
                    agent: None,
                    agent_status: "unknown".into(),
                    focused: true,
                },
                PaneInfo {
                    pane_id: "w1:p2".into(),
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    label: None,
                    terminal_title_stripped: None,
                    cwd: None,
                    agent: None,
                    agent_status: "unknown".into(),
                    focused: false,
                },
            ],
        }
    }

    #[test]
    fn source_validation_requires_exact_layout_membership() {
        let mover = Mover::new(Herdr::new("herdr"));
        let root = LayoutNode::Split {
            direction: SplitDirection::Right,
            ratio: 0.5,
            first: Box::new(LayoutNode::Pane {
                pane_id: "w1:p1".into(),
            }),
            second: Box::new(LayoutNode::Pane {
                pane_id: "w1:p2".into(),
            }),
        };

        assert!(mover
            .validate_tab_source(&topology(), &root, "w1:t1")
            .is_ok());
        assert!(mover
            .validate_tab_source(&topology(), &root, "w2:t1")
            .is_err());

        let mut topology = topology();
        topology.panes.push(PaneInfo {
            pane_id: "w1:p3".into(),
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            label: None,
            terminal_title_stripped: None,
            cwd: None,
            agent: None,
            agent_status: "unknown".into(),
            focused: false,
        });
        assert!(mover
            .validate_tab_source(&topology, &root, "w1:t1")
            .is_err());
    }
}
