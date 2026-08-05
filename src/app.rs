use std::cmp::Reverse;
use std::path::Path;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::fuzzy;
use crate::herdr::{PaneInfo, TabInfo, Topology, WorkspaceInfo};
use crate::layout::SplitDirection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveKind {
    Pane,
    Tab,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneDestination {
    ExistingTab {
        tab_id: String,
    },
    NewTab {
        workspace_id: String,
        label: Option<String>,
    },
    NewWorkspace {
        label: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabDestination {
    Workspace { workspace_id: String },
    NewWorkspace { label: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovePaneRequest {
    pub source_pane_id: String,
    pub source_tab_id: String,
    pub direction: SplitDirection,
    pub destination: PaneDestination,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveTabRequest {
    pub source_tab_id: String,
    pub source_pane_id: String,
    pub destination: TabDestination,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputOutcome {
    Continue,
    Cancel,
    MovePane(MovePaneRequest),
    MoveTab(MoveTabRequest),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowTone {
    Normal,
    Current,
    Create,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayRow {
    pub title: String,
    pub detail: String,
    pub tone: RowTone,
}

#[derive(Clone, Debug)]
enum Stage {
    Kind,
    PaneSource,
    TabSource,
    PaneDestination(PaneInfo),
    TabDestination(TabInfo),
}

#[derive(Clone, Debug)]
enum Choice {
    Kind(MoveKind),
    Pane(PaneInfo),
    Tab(TabInfo),
    PaneTab(TabInfo),
    PaneNewTab,
    PaneNewWorkspace,
    TabWorkspace(WorkspaceInfo),
    TabNewWorkspace,
}

#[derive(Clone, Debug)]
struct Candidate {
    choice: Choice,
    row: DisplayRow,
    search: String,
    pinned: bool,
}

#[derive(Clone, Debug)]
pub struct App {
    topology: Topology,
    invoked_pane_id: String,
    invoked_tab_id: String,
    stage: Stage,
    query: String,
    selected: usize,
    failure: Option<String>,
    working: Option<String>,
}

impl App {
    pub fn new(topology: Topology, invoked_pane_id: impl Into<String>) -> Result<Self> {
        let invoked_pane_id = invoked_pane_id.into();
        let invoked_tab_id = topology
            .panes
            .iter()
            .find(|pane| pane.pane_id == invoked_pane_id)
            .map(|pane| pane.tab_id.clone())
            .with_context(|| format!("source pane is no longer available: {invoked_pane_id}"))?;
        Ok(Self {
            topology,
            invoked_pane_id,
            invoked_tab_id,
            stage: Stage::Kind,
            query: String::new(),
            selected: 0,
            failure: None,
            working: None,
        })
    }

    pub fn handle_event(&mut self, event: Event) -> InputOutcome {
        match event {
            Event::Key(key) => self.handle_key(key),
            _ => InputOutcome::Continue,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> InputOutcome {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return InputOutcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            return InputOutcome::Cancel;
        }
        if self.working.is_some() {
            return InputOutcome::Continue;
        }
        if key.code == KeyCode::Esc {
            return self.back();
        }
        self.failure = None;

        if matches!(self.stage, Stage::Kind) {
            match key.code {
                KeyCode::Char('p' | 'P') => {
                    self.enter_source(MoveKind::Pane);
                    return InputOutcome::Continue;
                }
                KeyCode::Char('t' | 'T') => {
                    self.enter_source(MoveKind::Tab);
                    return InputOutcome::Continue;
                }
                _ => {}
            }
        }

        if matches!(self.stage, Stage::PaneDestination(_))
            && key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Char('d' | 'D'))
        {
            return self.activate(SplitDirection::Down);
        }

        match key.code {
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-8),
            KeyCode::PageDown => self.move_selection(8),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.visible_candidates().len().saturating_sub(1),
            KeyCode::Enter => return self.activate(SplitDirection::Right),
            KeyCode::Backspace if self.is_searchable() => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCode::Char(character)
                if self.is_searchable()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(character);
                self.selected = 0;
            }
            _ => {}
        }
        InputOutcome::Continue
    }

    pub fn rows(&self) -> Vec<DisplayRow> {
        self.visible_candidates()
            .into_iter()
            .map(|candidate| candidate.row)
            .collect()
    }

    pub fn selected(&self) -> Option<usize> {
        (!self.visible_candidates().is_empty()).then_some(self.selected)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn is_searchable(&self) -> bool {
        !matches!(self.stage, Stage::Kind)
    }

    pub fn heading(&self) -> &'static str {
        match self.stage {
            Stage::Kind => "What should cross?",
            Stage::PaneSource => "Choose a pane",
            Stage::TabSource => "Choose a tab",
            Stage::PaneDestination(_) => "Choose a destination tab",
            Stage::TabDestination(_) => "Choose a destination workspace",
        }
    }

    pub fn prompt(&self) -> &'static str {
        match self.stage {
            Stage::Kind => "",
            Stage::PaneSource => "search panes",
            Stage::TabSource => "search tabs",
            Stage::PaneDestination(_) => "search destinations or name a new one",
            Stage::TabDestination(_) => "search workspaces or name a new one",
        }
    }

    pub fn step(&self) -> usize {
        match self.stage {
            Stage::Kind => 1,
            Stage::PaneSource | Stage::TabSource => 2,
            Stage::PaneDestination(_) | Stage::TabDestination(_) => 3,
        }
    }

    pub fn trail(&self) -> &'static str {
        match self.stage {
            Stage::Kind => "move  ›  source  ›  destination",
            Stage::PaneSource => "pane  ›  source  ›  destination",
            Stage::TabSource => "tab  ›  source  ›  destination",
            Stage::PaneDestination(_) => "pane  ›  source  ›  destination",
            Stage::TabDestination(_) => "tab  ›  source  ›  destination",
        }
    }

    pub fn footer(&self) -> &'static str {
        match self.stage {
            Stage::Kind => "↑↓ navigate   enter choose   p/t shortcut   esc close",
            Stage::PaneSource | Stage::TabSource => {
                "type to filter   ↑↓ navigate   enter choose   esc back"
            }
            Stage::PaneDestination(_) => {
                "enter split right   alt+d split down   ↑↓ navigate   esc back"
            }
            Stage::TabDestination(_) => "enter move   ↑↓ navigate   esc back",
        }
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    pub fn working(&self) -> Option<&str> {
        self.working.as_deref()
    }

    pub fn set_failure(&mut self, failure: impl Into<String>) {
        self.working = None;
        self.failure = Some(failure.into());
    }

    pub fn set_working(&mut self, working: impl Into<String>) {
        self.failure = None;
        self.working = Some(working.into());
    }

    fn back(&mut self) -> InputOutcome {
        let previous = self.stage.clone();
        match previous {
            Stage::Kind => return InputOutcome::Cancel,
            Stage::PaneSource => {
                self.stage = Stage::Kind;
                self.selected = 0;
            }
            Stage::TabSource => {
                self.stage = Stage::Kind;
                self.selected = 1;
            }
            Stage::PaneDestination(source) => {
                self.stage = Stage::PaneSource;
                self.query.clear();
                self.selected = self
                    .candidates()
                    .iter()
                    .position(|candidate| {
                        matches!(&candidate.choice, Choice::Pane(pane) if pane.pane_id == source.pane_id)
                    })
                    .unwrap_or(0);
                return InputOutcome::Continue;
            }
            Stage::TabDestination(source) => {
                self.stage = Stage::TabSource;
                self.query.clear();
                self.selected = self
                    .candidates()
                    .iter()
                    .position(|candidate| {
                        matches!(&candidate.choice, Choice::Tab(tab) if tab.tab_id == source.tab_id)
                    })
                    .unwrap_or(0);
                return InputOutcome::Continue;
            }
        }
        self.query.clear();
        self.failure = None;
        InputOutcome::Continue
    }

    fn enter_source(&mut self, kind: MoveKind) {
        self.stage = match kind {
            MoveKind::Pane => Stage::PaneSource,
            MoveKind::Tab => Stage::TabSource,
        };
        self.query.clear();
        self.selected = 0;
        self.failure = None;
    }

    fn activate(&mut self, direction: SplitDirection) -> InputOutcome {
        let Some(candidate) = self.visible_candidates().get(self.selected).cloned() else {
            return InputOutcome::Continue;
        };
        match candidate.choice {
            Choice::Kind(kind) => {
                self.enter_source(kind);
                InputOutcome::Continue
            }
            Choice::Pane(pane) => {
                self.stage = Stage::PaneDestination(pane);
                self.query.clear();
                self.selected = 0;
                InputOutcome::Continue
            }
            Choice::Tab(tab) => {
                self.stage = Stage::TabDestination(tab);
                self.query.clear();
                self.selected = 0;
                InputOutcome::Continue
            }
            Choice::PaneTab(tab) => {
                let Stage::PaneDestination(source) = &self.stage else {
                    return InputOutcome::Continue;
                };
                InputOutcome::MovePane(MovePaneRequest {
                    source_pane_id: source.pane_id.clone(),
                    source_tab_id: source.tab_id.clone(),
                    direction,
                    destination: PaneDestination::ExistingTab { tab_id: tab.tab_id },
                })
            }
            Choice::PaneNewTab => {
                let Stage::PaneDestination(source) = &self.stage else {
                    return InputOutcome::Continue;
                };
                InputOutcome::MovePane(MovePaneRequest {
                    source_pane_id: source.pane_id.clone(),
                    source_tab_id: source.tab_id.clone(),
                    direction,
                    destination: PaneDestination::NewTab {
                        workspace_id: source.workspace_id.clone(),
                        label: self.query_name(),
                    },
                })
            }
            Choice::PaneNewWorkspace => {
                let Stage::PaneDestination(source) = &self.stage else {
                    return InputOutcome::Continue;
                };
                InputOutcome::MovePane(MovePaneRequest {
                    source_pane_id: source.pane_id.clone(),
                    source_tab_id: source.tab_id.clone(),
                    direction,
                    destination: PaneDestination::NewWorkspace {
                        label: self.query_name(),
                    },
                })
            }
            Choice::TabWorkspace(workspace) => self.tab_move_outcome(TabDestination::Workspace {
                workspace_id: workspace.workspace_id,
            }),
            Choice::TabNewWorkspace => self.tab_move_outcome(TabDestination::NewWorkspace {
                label: self.query_name(),
            }),
        }
    }

    fn tab_move_outcome(&mut self, destination: TabDestination) -> InputOutcome {
        let Stage::TabDestination(source) = &self.stage else {
            return InputOutcome::Continue;
        };
        let Some(source_pane) = self
            .topology
            .panes
            .iter()
            .find(|pane| pane.tab_id == source.tab_id && pane.focused)
            .or_else(|| {
                self.topology
                    .panes
                    .iter()
                    .find(|pane| pane.tab_id == source.tab_id)
            })
        else {
            self.set_failure("The selected tab no longer has a movable pane");
            return InputOutcome::Continue;
        };
        InputOutcome::MoveTab(MoveTabRequest {
            source_tab_id: source.tab_id.clone(),
            source_pane_id: source_pane.pane_id.clone(),
            destination,
        })
    }

    fn move_selection(&mut self, delta: isize) {
        let length = self.visible_candidates().len();
        if length == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(length as isize) as usize;
    }

    fn query_name(&self) -> Option<String> {
        let name = self.query.trim();
        (!name.is_empty()).then(|| name.to_string())
    }

    fn visible_candidates(&self) -> Vec<Candidate> {
        let candidates = self.candidates();
        if self.query.trim().is_empty() {
            return candidates;
        }
        let mut scored = candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| !candidate.pinned)
            .filter_map(|(index, candidate)| {
                fuzzy::score(&self.query, &candidate.search)
                    .map(|score| (Reverse(score), index, candidate.clone()))
            })
            .collect::<Vec<_>>();
        scored.sort_by_key(|(score, index, _)| (*score, *index));
        let mut visible = scored
            .into_iter()
            .map(|(_, _, candidate)| candidate)
            .collect::<Vec<_>>();
        visible.extend(candidates.into_iter().filter(|candidate| candidate.pinned));
        visible
    }

    fn candidates(&self) -> Vec<Candidate> {
        match &self.stage {
            Stage::Kind => self.kind_candidates(),
            Stage::PaneSource => self.pane_source_candidates(),
            Stage::TabSource => self.tab_source_candidates(),
            Stage::PaneDestination(source) => self.pane_destination_candidates(source),
            Stage::TabDestination(source) => self.tab_destination_candidates(source),
        }
    }

    fn kind_candidates(&self) -> Vec<Candidate> {
        let pane = self
            .topology
            .panes
            .iter()
            .find(|pane| pane.pane_id == self.invoked_pane_id);
        let tab = self
            .topology
            .tabs
            .iter()
            .find(|tab| tab.tab_id == self.invoked_tab_id);
        vec![
            Candidate {
                choice: Choice::Kind(MoveKind::Pane),
                row: DisplayRow {
                    title: "Move a pane".into(),
                    detail: pane
                        .map(|pane| format!("focused · {}", self.pane_label(pane)))
                        .unwrap_or_else(|| "choose any live pane".into()),
                    tone: RowTone::Normal,
                },
                search: String::new(),
                pinned: false,
            },
            Candidate {
                choice: Choice::Kind(MoveKind::Tab),
                row: DisplayRow {
                    title: "Move a whole tab".into(),
                    detail: tab
                        .map(|tab| {
                            format!(
                                "current · {} / {} · {} panes",
                                self.workspace_label(&tab.workspace_id),
                                self.tab_label(tab),
                                tab.pane_count
                            )
                        })
                        .unwrap_or_else(|| "preserve its live split layout".into()),
                    tone: RowTone::Normal,
                },
                search: String::new(),
                pinned: false,
            },
        ]
    }

    fn pane_source_candidates(&self) -> Vec<Candidate> {
        let invoked = self
            .topology
            .panes
            .iter()
            .find(|pane| pane.pane_id == self.invoked_pane_id);
        let invoked_workspace = invoked
            .map(|pane| pane.workspace_id.as_str())
            .unwrap_or_default();
        let mut panes = self.topology.panes.clone();
        panes.sort_by_key(|pane| {
            let scope = if pane.pane_id == self.invoked_pane_id {
                0
            } else if pane.tab_id == self.invoked_tab_id {
                1
            } else if pane.workspace_id == invoked_workspace {
                2
            } else {
                3
            };
            (
                scope,
                self.workspace_number(&pane.workspace_id),
                self.tab_number(&pane.tab_id),
                self.pane_label(pane),
            )
        });
        panes
            .into_iter()
            .map(|pane| {
                let current = pane.pane_id == self.invoked_pane_id;
                let title = self.pane_label(&pane);
                let detail = format!(
                    "{} / {}{}",
                    self.workspace_label(&pane.workspace_id),
                    self.tab_label_by_id(&pane.tab_id),
                    pane.agent
                        .as_ref()
                        .map(|agent| format!(" · {agent} {}", pane.agent_status))
                        .unwrap_or_default()
                );
                let search = format!(
                    "{title} {detail} {} {} {}",
                    pane.pane_id,
                    pane.cwd.as_deref().unwrap_or_default(),
                    pane.terminal_title_stripped.as_deref().unwrap_or_default()
                );
                Candidate {
                    choice: Choice::Pane(pane),
                    row: DisplayRow {
                        title,
                        detail,
                        tone: if current {
                            RowTone::Current
                        } else {
                            RowTone::Normal
                        },
                    },
                    search,
                    pinned: false,
                }
            })
            .collect()
    }

    fn tab_source_candidates(&self) -> Vec<Candidate> {
        let invoked_workspace = self
            .topology
            .tabs
            .iter()
            .find(|tab| tab.tab_id == self.invoked_tab_id)
            .map(|tab| tab.workspace_id.as_str())
            .unwrap_or_default();
        let mut tabs = self.topology.tabs.clone();
        tabs.sort_by_key(|tab| {
            let scope = if tab.tab_id == self.invoked_tab_id {
                0
            } else if tab.workspace_id == invoked_workspace {
                1
            } else {
                2
            };
            (scope, self.workspace_number(&tab.workspace_id), tab.number)
        });
        tabs.into_iter()
            .map(|tab| {
                let current = tab.tab_id == self.invoked_tab_id;
                let workspace = self.workspace_label(&tab.workspace_id);
                let title = format!("{workspace} / {}", self.tab_label(&tab));
                let detail = format!("{} panes · {}", tab.pane_count, tab.tab_id);
                Candidate {
                    choice: Choice::Tab(tab),
                    row: DisplayRow {
                        title: title.clone(),
                        detail: detail.clone(),
                        tone: if current {
                            RowTone::Current
                        } else {
                            RowTone::Normal
                        },
                    },
                    search: format!("{title} {detail}"),
                    pinned: false,
                }
            })
            .collect()
    }

    fn pane_destination_candidates(&self, source: &PaneInfo) -> Vec<Candidate> {
        let mut tabs = self
            .topology
            .tabs
            .iter()
            .filter(|tab| tab.tab_id != source.tab_id)
            .cloned()
            .collect::<Vec<_>>();
        tabs.sort_by_key(|tab| {
            (
                usize::from(tab.workspace_id != source.workspace_id),
                self.workspace_number(&tab.workspace_id),
                tab.number,
            )
        });
        let mut candidates = tabs
            .into_iter()
            .map(|tab| {
                let workspace = self.workspace_label(&tab.workspace_id);
                let title = format!("{workspace} / {}", self.tab_label(&tab));
                let detail = format!("{} panes · existing tab", tab.pane_count);
                Candidate {
                    choice: Choice::PaneTab(tab),
                    row: DisplayRow {
                        title: title.clone(),
                        detail: detail.clone(),
                        tone: RowTone::Normal,
                    },
                    search: format!("{title} {detail}"),
                    pinned: false,
                }
            })
            .collect::<Vec<_>>();
        let name = self.query_name();
        let preview = name
            .as_deref()
            .map(|name| format!("create “{name}”"))
            .unwrap_or_else(|| "type to name it".into());
        candidates.push(Candidate {
            choice: Choice::PaneNewTab,
            row: DisplayRow {
                title: format!("New tab in {}", self.workspace_label(&source.workspace_id)),
                detail: preview.clone(),
                tone: RowTone::Create,
            },
            search: String::new(),
            pinned: true,
        });
        candidates.push(Candidate {
            choice: Choice::PaneNewWorkspace,
            row: DisplayRow {
                title: "New workspace".into(),
                detail: preview,
                tone: RowTone::Create,
            },
            search: String::new(),
            pinned: true,
        });
        candidates
    }

    fn tab_destination_candidates(&self, source: &TabInfo) -> Vec<Candidate> {
        let mut workspaces = self
            .topology
            .workspaces
            .iter()
            .filter(|workspace| workspace.workspace_id != source.workspace_id)
            .cloned()
            .collect::<Vec<_>>();
        workspaces.sort_by_key(|workspace| workspace.number);
        let mut candidates = workspaces
            .into_iter()
            .map(|workspace| {
                let title = self.workspace_name(&workspace);
                let detail = format!(
                    "{} tabs · {} panes · {}",
                    workspace.tab_count, workspace.pane_count, workspace.workspace_id
                );
                Candidate {
                    choice: Choice::TabWorkspace(workspace),
                    row: DisplayRow {
                        title: title.clone(),
                        detail: detail.clone(),
                        tone: RowTone::Normal,
                    },
                    search: format!("{title} {detail}"),
                    pinned: false,
                }
            })
            .collect::<Vec<_>>();
        let detail = self
            .query_name()
            .as_deref()
            .map(|name| format!("create “{name}”"))
            .unwrap_or_else(|| "type to name it".into());
        candidates.push(Candidate {
            choice: Choice::TabNewWorkspace,
            row: DisplayRow {
                title: "New workspace".into(),
                detail,
                tone: RowTone::Create,
            },
            search: String::new(),
            pinned: true,
        });
        candidates
    }

    fn pane_label(&self, pane: &PaneInfo) -> String {
        pane.label
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or(pane.agent.as_deref())
            .or(pane.terminal_title_stripped.as_deref())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                pane.cwd
                    .as_deref()
                    .and_then(|cwd| Path::new(cwd).file_name())
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| pane.pane_id.clone())
    }

    fn workspace_name(&self, workspace: &WorkspaceInfo) -> String {
        if workspace.label.trim().is_empty() {
            workspace.workspace_id.clone()
        } else {
            workspace.label.clone()
        }
    }

    fn workspace_label(&self, workspace_id: &str) -> String {
        self.topology
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
            .map(|workspace| self.workspace_name(workspace))
            .unwrap_or_else(|| workspace_id.to_string())
    }

    fn tab_label(&self, tab: &TabInfo) -> String {
        if tab.label.trim().is_empty() {
            tab.tab_id.clone()
        } else {
            tab.label.clone()
        }
    }

    fn tab_label_by_id(&self, tab_id: &str) -> String {
        self.topology
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| self.tab_label(tab))
            .unwrap_or_else(|| tab_id.to_string())
    }

    fn workspace_number(&self, workspace_id: &str) -> usize {
        self.topology
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
            .map(|workspace| workspace.number)
            .unwrap_or(usize::MAX)
    }

    fn tab_number(&self, tab_id: &str) -> usize {
        self.topology
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| tab.number)
            .unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn topology() -> Topology {
        Topology {
            workspaces: vec![
                WorkspaceInfo {
                    workspace_id: "w1".into(),
                    label: "source".into(),
                    number: 1,
                    tab_count: 2,
                    pane_count: 3,
                    focused: true,
                },
                WorkspaceInfo {
                    workspace_id: "w2".into(),
                    label: "target".into(),
                    number: 2,
                    tab_count: 1,
                    pane_count: 1,
                    focused: false,
                },
            ],
            tabs: vec![
                TabInfo {
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    label: "main".into(),
                    number: 1,
                    pane_count: 2,
                    focused: true,
                },
                TabInfo {
                    tab_id: "w1:t2".into(),
                    workspace_id: "w1".into(),
                    label: "logs".into(),
                    number: 2,
                    pane_count: 1,
                    focused: false,
                },
                TabInfo {
                    tab_id: "w2:t1".into(),
                    workspace_id: "w2".into(),
                    label: "api".into(),
                    number: 1,
                    pane_count: 1,
                    focused: false,
                },
            ],
            panes: vec![
                PaneInfo {
                    pane_id: "w1:p1".into(),
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    label: Some("focused-pane".into()),
                    terminal_title_stripped: None,
                    cwd: Some("/tmp/source".into()),
                    agent: Some("codex".into()),
                    agent_status: "working".into(),
                    focused: true,
                },
                PaneInfo {
                    pane_id: "w1:p2".into(),
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    label: Some("other-pane".into()),
                    terminal_title_stripped: None,
                    cwd: None,
                    agent: None,
                    agent_status: "unknown".into(),
                    focused: false,
                },
                PaneInfo {
                    pane_id: "w1:p3".into(),
                    tab_id: "w1:t2".into(),
                    workspace_id: "w1".into(),
                    label: Some("logger".into()),
                    terminal_title_stripped: None,
                    cwd: None,
                    agent: None,
                    agent_status: "unknown".into(),
                    focused: false,
                },
                PaneInfo {
                    pane_id: "w2:p1".into(),
                    tab_id: "w2:t1".into(),
                    workspace_id: "w2".into(),
                    label: Some("api-server".into()),
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
    fn invoked_pane_and_tab_are_the_default_sources() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.rows()[0].title, "focused-pane");
        assert_eq!(app.rows()[0].tone, RowTone::Current);

        app.handle_key(key(KeyCode::Esc));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));

        assert!(app.rows()[0].title.contains("source / main"));
        assert_eq!(app.rows()[0].tone, RowTone::Current);
    }

    #[test]
    fn fuzzy_source_filter_searches_labels_and_workspace_context() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Enter));
        for character in "api".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }

        assert_eq!(app.rows().len(), 1);
        assert_eq!(app.rows()[0].title, "api-server");
    }

    #[test]
    fn pane_flow_supports_existing_tabs_and_alt_down() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Down));

        let outcome = app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT));

        assert_eq!(
            outcome,
            InputOutcome::MovePane(MovePaneRequest {
                source_pane_id: "w1:p1".into(),
                source_tab_id: "w1:t1".into(),
                direction: SplitDirection::Down,
                destination: PaneDestination::ExistingTab {
                    tab_id: "w2:t1".into()
                },
            })
        );
    }

    #[test]
    fn destination_query_names_a_new_workspace() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Enter));
        for character in "new-home".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        let last = app.rows().len() - 1;
        app.selected = last;

        let InputOutcome::MoveTab(request) = app.handle_key(key(KeyCode::Enter)) else {
            panic!("expected tab move");
        };
        assert_eq!(
            request.destination,
            TabDestination::NewWorkspace {
                label: Some("new-home".into())
            }
        );
    }

    #[test]
    fn escape_steps_back_before_closing() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.step(), 3);
        assert_eq!(app.handle_key(key(KeyCode::Esc)), InputOutcome::Continue);
        assert_eq!(app.step(), 2);
        assert_eq!(app.handle_key(key(KeyCode::Esc)), InputOutcome::Continue);
        assert_eq!(app.step(), 1);
        assert_eq!(app.handle_key(key(KeyCode::Esc)), InputOutcome::Cancel);
    }

    #[test]
    fn rejects_missing_invocation_panes() {
        assert!(App::new(topology(), "missing").is_err());
    }
}
