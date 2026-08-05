use std::collections::HashMap;

use anyhow::{bail, Context, Result};

use crate::app::{MovePaneRequest, MoveTabRequest, PaneDestination, TabDestination};
use crate::herdr::{Herdr, Topology};
use crate::layout::LayoutNode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveSummary {
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Mover {
    herdr: Herdr,
}

impl Mover {
    pub fn new(herdr: Herdr) -> Self {
        Self { herdr }
    }

    pub fn move_pane(&self, request: &MovePaneRequest) -> Result<MoveSummary> {
        let source = self.herdr.pane(&request.source_pane_id)?;
        if source.tab_id != request.source_tab_id {
            bail!("the source pane changed tabs while Ferry was open");
        }
        let moved = match &request.destination {
            PaneDestination::ExistingTab { tab_id } => self.herdr.move_to_tab(
                &source.pane_id,
                tab_id,
                request.direction,
                None,
                None,
                true,
            )?,
            PaneDestination::NewTab {
                workspace_id,
                label,
            } => {
                self.herdr
                    .move_to_new_tab(&source.pane_id, workspace_id, label.as_deref(), true)?
            }
            PaneDestination::NewWorkspace { label } => {
                self.herdr
                    .move_to_new_workspace(&source.pane_id, label.as_deref(), None, true)?
            }
        };
        Ok(MoveSummary {
            message: format!("Pane moved to {}", moved.tab_id),
        })
    }

    pub fn move_tab(&self, request: &MoveTabRequest) -> Result<MoveSummary> {
        let snapshot = self.herdr.layout(&request.source_pane_id)?;
        if snapshot.tab_id != request.source_tab_id {
            bail!("the source tab changed while Ferry was open");
        }
        if snapshot.zoomed {
            bail!("unzoom the source tab before moving it");
        }
        let root = LayoutNode::from_snapshot(&snapshot)?;
        let total = root.leaves().len();
        let topology = self.herdr.topology()?;
        self.validate_tab_source(&topology, &root, &snapshot.tab_id)?;
        let tab_label = topology
            .tabs
            .iter()
            .find(|tab| tab.tab_id == snapshot.tab_id)
            .map(|tab| tab.label.trim())
            .filter(|label| !label.is_empty())
            .unwrap_or("moved")
            .to_string();

        let anchor = root.anchor().to_string();
        let first = match &request.destination {
            TabDestination::Workspace { workspace_id } => {
                if workspace_id == &snapshot.workspace_id {
                    bail!("a tab must move to a different workspace");
                }
                if !topology
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.workspace_id == *workspace_id)
                {
                    bail!("the destination workspace no longer exists");
                }
                self.herdr
                    .move_to_new_tab(&anchor, workspace_id, Some(&tab_label), false)?
            }
            TabDestination::NewWorkspace { label } => self.herdr.move_to_new_workspace(
                &anchor,
                label.as_deref().or(Some(&tab_label)),
                Some(&tab_label),
                false,
            )?,
        };

        let mut moved_count = 1;
        let mut id_map = HashMap::from([(anchor, first.pane_id.clone())]);
        if let Err(error) = self.place_layout(&root, &first.tab_id, &mut id_map, &mut moved_count) {
            bail!(
                "tab move stopped after {moved_count}/{total} panes; all moved panes are still live: {error:#}"
            );
        }
        self.herdr.focus_workspace(&first.workspace_id)?;
        self.herdr.focus_tab(&first.tab_id)?;
        Ok(MoveSummary {
            message: format!("Tab “{tab_label}” moved to {}", first.workspace_id),
        })
    }

    fn validate_tab_source(
        &self,
        topology: &Topology,
        root: &LayoutNode,
        source_tab_id: &str,
    ) -> Result<()> {
        let pane_tabs = topology
            .panes
            .iter()
            .map(|pane| (pane.pane_id.as_str(), pane.tab_id.as_str()))
            .collect::<HashMap<_, _>>();
        if root
            .leaves()
            .iter()
            .all(|pane_id| pane_tabs.get(pane_id).copied() == Some(source_tab_id))
        {
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
    fn source_validation_requires_every_layout_leaf_in_the_tab() {
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
    }
}
