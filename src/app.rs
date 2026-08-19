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
    Workspace,
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
    pub sources: Vec<PaneSource>,
    pub direction: SplitDirection,
    pub destination: PaneDestination,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSource {
    pub pane_id: String,
    pub expected_tab_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveTabRequest {
    pub tab_ids: Vec<String>,
    pub destination: TabDestination,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeWorkspaceRequest {
    pub source_workspace_id: String,
    pub destination_workspace_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputOutcome {
    Continue,
    Cancel,
    MovePane(MovePaneRequest),
    MoveTab(MoveTabRequest),
    MergeWorkspace(MergeWorkspaceRequest),
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
    pub checked: bool,
}

#[derive(Clone, Debug)]
enum Stage {
    Kind,
    PaneSource,
    TabSource,
    WorkspaceSource,
    PaneDestination(Vec<PaneInfo>),
    TabDestination(Vec<TabInfo>),
    WorkspaceDestination(WorkspaceInfo),
}

#[derive(Clone, Debug)]
enum Choice {
    Kind(MoveKind),
    Pane(PaneInfo),
    Tab(TabInfo),
    PaneTab(TabInfo),
    PaneNewTab(WorkspaceInfo),
    PaneNewWorkspace,
    TabWorkspace(WorkspaceInfo),
    TabNewWorkspace,
    WorkspaceSource(WorkspaceInfo),
    WorkspaceDestination(WorkspaceInfo),
}

#[derive(Clone, Debug)]
struct Candidate {
    choice: Choice,
    row: DisplayRow,
    search: String,
    pinned: bool,
    source_key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct App {
    topology: Topology,
    invoked_pane_id: String,
    invoked_tab_id: String,
    invoked_workspace_id: String,
    stage: Stage,
    query: String,
    selected: usize,
    checked_sources: Vec<String>,
    failure: Option<String>,
    working: Option<String>,
}

impl App {
    pub fn new(topology: Topology, invoked_pane_id: impl Into<String>) -> Result<Self> {
        let invoked_pane_id = invoked_pane_id.into();
        let invoked = topology
            .panes
            .iter()
            .find(|pane| pane.pane_id == invoked_pane_id)
            .with_context(|| format!("source pane is no longer available: {invoked_pane_id}"))?;
        let invoked_tab_id = invoked.tab_id.clone();
        let invoked_workspace_id = invoked.workspace_id.clone();
        Ok(Self {
            topology,
            invoked_pane_id,
            invoked_tab_id,
            invoked_workspace_id,
            stage: Stage::Kind,
            query: String::new(),
            selected: 0,
            checked_sources: Vec::new(),
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
                KeyCode::Char('w' | 'W') => {
                    self.enter_source(MoveKind::Workspace);
                    return InputOutcome::Continue;
                }
                _ => {}
            }
        }

        if self.is_source_stage() {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('a' | 'A'))
            {
                self.toggle_all_visible();
                return InputOutcome::Continue;
            }
            match key.code {
                KeyCode::Char(' ') => {
                    self.toggle_current_source();
                    return InputOutcome::Continue;
                }
                KeyCode::Tab => {
                    self.toggle_source_and_move(1);
                    return InputOutcome::Continue;
                }
                KeyCode::BackTab => {
                    self.toggle_source_and_move(-1);
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

    pub fn heading(&self) -> String {
        match self.stage {
            Stage::Kind => "What should cross?".into(),
            Stage::PaneSource => source_heading("pane", self.checked_sources.len()),
            Stage::TabSource => source_heading("tab", self.checked_sources.len()),
            Stage::WorkspaceSource => "Choose a workspace to merge".into(),
            Stage::PaneDestination(ref sources) => {
                format!(
                    "Move {} {} to…",
                    sources.len(),
                    plural("pane", sources.len())
                )
            }
            Stage::TabDestination(ref sources) => {
                format!(
                    "Move {} {} to…",
                    sources.len(),
                    plural("tab", sources.len())
                )
            }
            Stage::WorkspaceDestination(ref source) => {
                format!("Merge {} into…", self.workspace_name(source))
            }
        }
    }

    pub fn prompt(&self) -> &'static str {
        match self.stage {
            Stage::Kind => "",
            Stage::PaneSource => "search panes",
            Stage::TabSource => "search tabs",
            Stage::WorkspaceSource => "search source workspaces",
            Stage::PaneDestination(_) => "search destinations or name a new one",
            Stage::TabDestination(_) => "search workspaces or name a new one",
            Stage::WorkspaceDestination(_) => "search destination workspaces",
        }
    }

    pub fn step(&self) -> usize {
        match self.stage {
            Stage::Kind => 1,
            Stage::PaneSource | Stage::TabSource | Stage::WorkspaceSource => 2,
            Stage::PaneDestination(_)
            | Stage::TabDestination(_)
            | Stage::WorkspaceDestination(_) => 3,
        }
    }

    pub fn trail(&self) -> &'static str {
        match self.stage {
            Stage::Kind => "move  ›  source  ›  destination",
            Stage::PaneSource => "pane  ›  source  ›  destination",
            Stage::TabSource => "tab  ›  source  ›  destination",
            Stage::WorkspaceSource => "workspace  ›  source  ›  destination",
            Stage::PaneDestination(_) => "pane  ›  source  ›  destination",
            Stage::TabDestination(_) => "tab  ›  source  ›  destination",
            Stage::WorkspaceDestination(_) => "workspace  ›  source  ›  destination",
        }
    }

    pub fn footer(&self) -> String {
        match self.stage {
            Stage::Kind => "↑↓ navigate   enter choose   p/t/w shortcut   esc close".into(),
            Stage::PaneSource | Stage::TabSource => {
                "space/tab select   ctrl+a all   enter continue   esc back".into()
            }
            Stage::WorkspaceSource => {
                "type to filter   ↑↓ navigate   enter choose   esc back".into()
            }
            Stage::PaneDestination(_) => {
                "enter split right   alt+d split down   ↑↓ navigate   esc back".into()
            }
            Stage::TabDestination(_) => "enter move   ↑↓ navigate   esc back".into(),
            Stage::WorkspaceDestination(_) => "enter merge   ↑↓ navigate   esc back".into(),
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
                self.checked_sources.clear();
            }
            Stage::TabSource => {
                self.stage = Stage::Kind;
                self.selected = 1;
                self.checked_sources.clear();
            }
            Stage::WorkspaceSource => {
                self.stage = Stage::Kind;
                self.selected = 2;
            }
            Stage::PaneDestination(sources) => {
                self.stage = Stage::PaneSource;
                self.query.clear();
                let first = sources.first().map(|source| source.pane_id.as_str());
                self.selected = self
                    .candidates()
                    .iter()
                    .position(|candidate| {
                        matches!(&candidate.choice, Choice::Pane(pane) if Some(pane.pane_id.as_str()) == first)
                    })
                    .unwrap_or(0);
                return InputOutcome::Continue;
            }
            Stage::TabDestination(sources) => {
                self.stage = Stage::TabSource;
                self.query.clear();
                let first = sources.first().map(|source| source.tab_id.as_str());
                self.selected = self
                    .candidates()
                    .iter()
                    .position(|candidate| {
                        matches!(&candidate.choice, Choice::Tab(tab) if Some(tab.tab_id.as_str()) == first)
                    })
                    .unwrap_or(0);
                return InputOutcome::Continue;
            }
            Stage::WorkspaceDestination(source) => {
                self.stage = Stage::WorkspaceSource;
                self.query.clear();
                self.selected = self
                    .candidates()
                    .iter()
                    .position(|candidate| {
                        matches!(
                            &candidate.choice,
                            Choice::WorkspaceSource(workspace)
                                if workspace.workspace_id == source.workspace_id
                        )
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
            MoveKind::Workspace => Stage::WorkspaceSource,
        };
        self.query.clear();
        self.selected = 0;
        self.checked_sources.clear();
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
                self.stage = Stage::PaneDestination(self.chosen_panes(pane));
                self.query.clear();
                self.selected = 0;
                InputOutcome::Continue
            }
            Choice::Tab(tab) => {
                self.stage = Stage::TabDestination(self.chosen_tabs(tab));
                self.query.clear();
                self.selected = 0;
                InputOutcome::Continue
            }
            Choice::WorkspaceSource(workspace) => {
                self.stage = Stage::WorkspaceDestination(workspace);
                self.query.clear();
                self.selected = 0;
                InputOutcome::Continue
            }
            Choice::PaneTab(tab) => {
                let Stage::PaneDestination(sources) = &self.stage else {
                    return InputOutcome::Continue;
                };
                InputOutcome::MovePane(MovePaneRequest {
                    sources: pane_sources(sources),
                    direction,
                    destination: PaneDestination::ExistingTab { tab_id: tab.tab_id },
                })
            }
            Choice::PaneNewTab(workspace) => {
                let Stage::PaneDestination(sources) = &self.stage else {
                    return InputOutcome::Continue;
                };
                InputOutcome::MovePane(MovePaneRequest {
                    sources: pane_sources(sources),
                    direction,
                    destination: PaneDestination::NewTab {
                        workspace_id: workspace.workspace_id,
                        label: self.query_name(),
                    },
                })
            }
            Choice::PaneNewWorkspace => {
                let Stage::PaneDestination(sources) = &self.stage else {
                    return InputOutcome::Continue;
                };
                InputOutcome::MovePane(MovePaneRequest {
                    sources: pane_sources(sources),
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
            Choice::WorkspaceDestination(destination) => {
                let Stage::WorkspaceDestination(source) = &self.stage else {
                    return InputOutcome::Continue;
                };
                InputOutcome::MergeWorkspace(MergeWorkspaceRequest {
                    source_workspace_id: source.workspace_id.clone(),
                    destination_workspace_id: destination.workspace_id,
                })
            }
        }
    }

    fn tab_move_outcome(&self, destination: TabDestination) -> InputOutcome {
        let Stage::TabDestination(sources) = &self.stage else {
            return InputOutcome::Continue;
        };
        InputOutcome::MoveTab(MoveTabRequest {
            tab_ids: sources.iter().map(|source| source.tab_id.clone()).collect(),
            destination,
        })
    }

    fn is_source_stage(&self) -> bool {
        matches!(self.stage, Stage::PaneSource | Stage::TabSource)
    }

    fn toggle_current_source(&mut self) {
        let Some(key) = self
            .visible_candidates()
            .get(self.selected)
            .and_then(|candidate| candidate.source_key.clone())
        else {
            return;
        };
        self.toggle_source(&key);
    }

    fn toggle_source_and_move(&mut self, delta: isize) {
        let visible = self.visible_candidates();
        if visible.is_empty() {
            return;
        }
        let Some(current_key) = visible
            .get(self.selected)
            .and_then(|candidate| candidate.source_key.as_deref())
        else {
            return;
        };
        let next = (self.selected as isize + delta).rem_euclid(visible.len() as isize) as usize;
        let next_key = visible[next].source_key.clone();
        self.toggle_source(current_key);
        if let Some(next_key) = next_key {
            self.selected = self
                .visible_candidates()
                .iter()
                .position(|candidate| candidate.source_key.as_deref() == Some(&next_key))
                .unwrap_or(0);
        }
    }

    fn toggle_source(&mut self, key: &str) {
        if let Some(index) = self
            .checked_sources
            .iter()
            .position(|checked| checked == key)
        {
            self.checked_sources.remove(index);
        } else {
            self.checked_sources.push(key.into());
        }
    }

    fn toggle_all_visible(&mut self) {
        let visible = self
            .visible_candidates()
            .into_iter()
            .filter_map(|candidate| candidate.source_key)
            .collect::<Vec<_>>();
        if visible.is_empty() {
            return;
        }
        if visible.iter().all(|key| self.checked_sources.contains(key)) {
            self.checked_sources
                .retain(|checked| !visible.contains(checked));
        } else {
            for key in visible {
                if !self.checked_sources.contains(&key) {
                    self.checked_sources.push(key);
                }
            }
        }
    }

    fn chosen_panes(&self, fallback: PaneInfo) -> Vec<PaneInfo> {
        if self.checked_sources.is_empty() {
            return vec![fallback];
        }
        self.checked_sources
            .iter()
            .filter_map(|pane_id| {
                self.topology
                    .panes
                    .iter()
                    .find(|pane| &pane.pane_id == pane_id)
                    .cloned()
            })
            .collect()
    }

    fn chosen_tabs(&self, fallback: TabInfo) -> Vec<TabInfo> {
        if self.checked_sources.is_empty() {
            return vec![fallback];
        }
        self.checked_sources
            .iter()
            .filter_map(|tab_id| {
                self.topology
                    .tabs
                    .iter()
                    .find(|tab| &tab.tab_id == tab_id)
                    .cloned()
            })
            .collect()
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
        let checked = self
            .checked_sources
            .iter()
            .filter_map(|key| {
                candidates
                    .iter()
                    .find(|candidate| candidate.source_key.as_deref() == Some(key))
                    .cloned()
            })
            .collect::<Vec<_>>();
        let mut scored = candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| !candidate.pinned && !candidate.row.checked)
            .filter_map(|(index, candidate)| {
                fuzzy::score(&self.query, &candidate.search)
                    .map(|score| (Reverse(score), index, candidate.clone()))
            })
            .collect::<Vec<_>>();
        scored.sort_by_key(|(score, index, _)| (*score, *index));
        let mut visible = checked;
        visible.extend(scored.into_iter().map(|(_, _, candidate)| candidate));
        visible.extend(candidates.into_iter().filter(|candidate| candidate.pinned));
        visible
    }

    fn candidates(&self) -> Vec<Candidate> {
        match &self.stage {
            Stage::Kind => self.kind_candidates(),
            Stage::PaneSource => self.pane_source_candidates(),
            Stage::TabSource => self.tab_source_candidates(),
            Stage::WorkspaceSource => self.workspace_source_candidates(),
            Stage::PaneDestination(source) => self.pane_destination_candidates(source),
            Stage::TabDestination(source) => self.tab_destination_candidates(source),
            Stage::WorkspaceDestination(source) => self.workspace_destination_candidates(source),
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
        let workspace = self
            .topology
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == self.invoked_workspace_id);
        vec![
            Candidate {
                choice: Choice::Kind(MoveKind::Pane),
                row: DisplayRow {
                    title: "Move a pane".into(),
                    detail: pane
                        .map(|pane| format!("focused · {}", self.pane_label(pane)))
                        .unwrap_or_else(|| "choose any live pane".into()),
                    tone: RowTone::Normal,
                    checked: false,
                },
                search: String::new(),
                pinned: false,
                source_key: None,
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
                    checked: false,
                },
                search: String::new(),
                pinned: false,
                source_key: None,
            },
            Candidate {
                choice: Choice::Kind(MoveKind::Workspace),
                row: DisplayRow {
                    title: "Merge a workspace".into(),
                    detail: workspace
                        .map(|workspace| {
                            format!(
                                "current · {} · {} tabs · {} panes",
                                self.workspace_name(workspace),
                                workspace.tab_count,
                                workspace.pane_count
                            )
                        })
                        .unwrap_or_else(|| "append all of its tabs to another workspace".into()),
                    tone: RowTone::Normal,
                    checked: false,
                },
                search: String::new(),
                pinned: false,
                source_key: None,
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
                let checked = self.checked_sources.contains(&pane.pane_id);
                let source_key = pane.pane_id.clone();
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
                        checked,
                    },
                    search,
                    pinned: false,
                    source_key: Some(source_key),
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
                let checked = self.checked_sources.contains(&tab.tab_id);
                let source_key = tab.tab_id.clone();
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
                        checked,
                    },
                    search: format!("{title} {detail}"),
                    pinned: false,
                    source_key: Some(source_key),
                }
            })
            .collect()
    }

    fn workspace_source_candidates(&self) -> Vec<Candidate> {
        let mut workspaces = self.topology.workspaces.clone();
        workspaces.sort_by_key(|workspace| {
            (
                usize::from(workspace.workspace_id != self.invoked_workspace_id),
                workspace.number,
            )
        });
        workspaces
            .into_iter()
            .map(|workspace| {
                let current = workspace.workspace_id == self.invoked_workspace_id;
                let title = self.workspace_name(&workspace);
                let detail = format!(
                    "{} tabs · {} panes · {}",
                    workspace.tab_count, workspace.pane_count, workspace.workspace_id
                );
                Candidate {
                    choice: Choice::WorkspaceSource(workspace),
                    row: DisplayRow {
                        title: title.clone(),
                        detail: detail.clone(),
                        tone: if current {
                            RowTone::Current
                        } else {
                            RowTone::Normal
                        },
                        checked: false,
                    },
                    search: format!("{title} {detail}"),
                    pinned: false,
                    source_key: None,
                }
            })
            .collect()
    }

    fn pane_destination_candidates(&self, sources: &[PaneInfo]) -> Vec<Candidate> {
        let source_tabs = sources
            .iter()
            .map(|source| source.tab_id.as_str())
            .collect::<Vec<_>>();
        let source_workspaces = sources
            .iter()
            .map(|source| source.workspace_id.as_str())
            .collect::<Vec<_>>();
        let mut tabs = self
            .topology
            .tabs
            .iter()
            .filter(|tab| !source_tabs.contains(&tab.tab_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        tabs.sort_by_key(|tab| {
            (
                usize::from(!source_workspaces.contains(&tab.workspace_id.as_str())),
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
                        checked: false,
                    },
                    search: format!("{title} {detail}"),
                    pinned: false,
                    source_key: None,
                }
            })
            .collect::<Vec<_>>();
        let name = self.query_name();
        let preview = name
            .as_deref()
            .map(|name| format!("create “{name}”"))
            .unwrap_or_else(|| "type to name it".into());
        let mut workspaces = self.topology.workspaces.clone();
        workspaces.sort_by_key(|workspace| {
            (
                usize::from(!source_workspaces.contains(&workspace.workspace_id.as_str())),
                workspace.number,
            )
        });
        candidates.extend(workspaces.into_iter().map(|workspace| Candidate {
            choice: Choice::PaneNewTab(workspace.clone()),
            row: DisplayRow {
                title: format!("New tab in {}", self.workspace_name(&workspace)),
                detail: preview.clone(),
                tone: RowTone::Create,
                checked: false,
            },
            search: String::new(),
            pinned: true,
            source_key: None,
        }));
        candidates.push(Candidate {
            choice: Choice::PaneNewWorkspace,
            row: DisplayRow {
                title: "New workspace".into(),
                detail: preview,
                tone: RowTone::Create,
                checked: false,
            },
            search: String::new(),
            pinned: true,
            source_key: None,
        });
        candidates
    }

    fn tab_destination_candidates(&self, sources: &[TabInfo]) -> Vec<Candidate> {
        let source_workspaces = sources
            .iter()
            .map(|source| source.workspace_id.as_str())
            .collect::<Vec<_>>();
        let mut workspaces = self
            .topology
            .workspaces
            .iter()
            .filter(|workspace| !source_workspaces.contains(&workspace.workspace_id.as_str()))
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
                        checked: false,
                    },
                    search: format!("{title} {detail}"),
                    pinned: false,
                    source_key: None,
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
                checked: false,
            },
            search: String::new(),
            pinned: true,
            source_key: None,
        });
        candidates
    }

    fn workspace_destination_candidates(&self, source: &WorkspaceInfo) -> Vec<Candidate> {
        let mut workspaces = self
            .topology
            .workspaces
            .iter()
            .filter(|workspace| workspace.workspace_id != source.workspace_id)
            .cloned()
            .collect::<Vec<_>>();
        workspaces.sort_by_key(|workspace| workspace.number);
        workspaces
            .into_iter()
            .map(|workspace| {
                let title = self.workspace_name(&workspace);
                let detail = format!(
                    "append after {} tabs · {} panes · {}",
                    workspace.tab_count, workspace.pane_count, workspace.workspace_id
                );
                Candidate {
                    choice: Choice::WorkspaceDestination(workspace),
                    row: DisplayRow {
                        title: title.clone(),
                        detail: detail.clone(),
                        tone: RowTone::Normal,
                        checked: false,
                    },
                    search: format!("{title} {detail}"),
                    pinned: false,
                    source_key: None,
                }
            })
            .collect()
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

fn source_heading(noun: &str, count: usize) -> String {
    if count == 0 {
        format!("Choose one or more {}s", noun)
    } else {
        format!("{count} {} selected", plural(noun, count))
    }
}

fn plural(noun: &str, count: usize) -> String {
    if count == 1 {
        noun.into()
    } else {
        format!("{noun}s")
    }
}

fn pane_sources(sources: &[PaneInfo]) -> Vec<PaneSource> {
    sources
        .iter()
        .map(|source| PaneSource {
            pane_id: source.pane_id.clone(),
            expected_tab_id: source.tab_id.clone(),
        })
        .collect()
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
    fn workspace_merge_defaults_to_the_current_workspace() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Char('w')));

        assert_eq!(app.rows()[0].title, "source");
        assert_eq!(app.rows()[0].tone, RowTone::Current);

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.heading(), "Merge source into…");
        assert_eq!(app.rows().len(), 1);
        assert_eq!(app.rows()[0].title, "target");

        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            InputOutcome::MergeWorkspace(MergeWorkspaceRequest {
                source_workspace_id: "w1".into(),
                destination_workspace_id: "w2".into(),
            })
        );
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
                sources: vec![PaneSource {
                    pane_id: "w1:p1".into(),
                    expected_tab_id: "w1:t1".into(),
                }],
                direction: SplitDirection::Down,
                destination: PaneDestination::ExistingTab {
                    tab_id: "w2:t1".into()
                },
            })
        );
    }

    #[test]
    fn pane_sources_support_ordered_multi_selection() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.heading(), "Move 2 panes to…");
        assert!(!app
            .rows()
            .iter()
            .any(|row| row.title.contains("source / main")));

        app.selected = app.rows().len() - 1;
        let InputOutcome::MovePane(request) = app.handle_key(key(KeyCode::Enter)) else {
            panic!("expected pane move");
        };
        assert_eq!(
            request.sources,
            vec![
                PaneSource {
                    pane_id: "w1:p1".into(),
                    expected_tab_id: "w1:t1".into(),
                },
                PaneSource {
                    pane_id: "w1:p2".into(),
                    expected_tab_id: "w1:t1".into(),
                },
            ]
        );
    }

    #[test]
    fn tab_sources_support_ordered_multi_selection() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Enter));

        let InputOutcome::MoveTab(request) = app.handle_key(key(KeyCode::Enter)) else {
            panic!("expected tab move");
        };
        assert_eq!(request.tab_ids, vec!["w1:t1", "w1:t2"]);
        assert_eq!(
            request.destination,
            TabDestination::Workspace {
                workspace_id: "w2".into()
            }
        );
    }

    #[test]
    fn selected_sources_remain_visible_while_filtering() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char(' ')));
        for character in "api".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }

        assert_eq!(app.rows().len(), 2);
        assert_eq!(app.rows()[0].title, "focused-pane");
        assert!(app.rows()[0].checked);
        assert_eq!(app.rows()[1].title, "api-server");
    }

    #[test]
    fn tab_advances_in_filtered_order_after_selection_reorders() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Char('e')));
        app.selected = 2;
        let rows = app.rows();
        let expected = rows[3].title.clone();

        app.handle_key(key(KeyCode::Tab));

        assert_eq!(app.rows()[app.selected].title, expected);
    }

    #[test]
    fn ctrl_a_toggles_all_visible_sources() {
        let mut app = App::new(topology(), "w1:p1").unwrap();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

        assert!(app.rows().iter().all(|row| row.checked));

        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(app.rows().iter().all(|row| !row.checked));
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
