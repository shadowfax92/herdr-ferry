//! Pure picker state: selection emits a review request, and only an exact typed
//! confirmation emits a plan. The event loop owns I/O; navigating or cancelling
//! this state machine cannot enqueue destructive work.
use std::cmp::Reverse;
use std::collections::BTreeSet;

use anyhow::{ensure, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{DisplayRow, RowTone};
use crate::close_plan::{CloseKind, ClosePlan, CloseRequest};
use crate::fuzzy;
use crate::herdr::Topology;

const KINDS: [CloseKind; 4] = [
    CloseKind::Panes,
    CloseKind::Tabs,
    CloseKind::Workspaces,
    CloseKind::Clear,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Close,
    ClearFt,
}

#[derive(Debug)]
pub enum CloseOutcome {
    Continue,
    Cancel,
    Refresh,
    Review(CloseRequest),
    Confirm(Box<ClosePlan>),
}

#[derive(Debug)]
enum Stage {
    Kind,
    Select(CloseKind),
    Review(Box<ClosePlan>),
}

/// Owns local UI choices only. Clear's preset limits candidates to exact `ft`
/// labels; duplicate labels remain distinct IDs and are never multi-selected.
pub struct CloseApp {
    topology: Topology,
    entry: Entry,
    stage: Stage,
    query: String,
    selected: usize,
    checked: BTreeSet<String>,
    confirmation: String,
    scroll: u16,
    failure: Option<String>,
}

impl CloseApp {
    pub fn new(topology: Topology, entry: Entry) -> Result<Self> {
        if entry == Entry::ClearFt {
            ensure!(topology.workspaces.iter().any(|w| w.label == "ft"), "No workspace labelled ft in this Herdr session. Open Close/Clear and choose another workspace, or create ft first.");
        }
        Ok(Self {
            topology,
            entry,
            stage: if entry == Entry::ClearFt {
                Stage::Select(CloseKind::Clear)
            } else {
                Stage::Kind
            },
            query: String::new(),
            selected: 0,
            checked: BTreeSet::new(),
            confirmation: String::new(),
            scroll: 0,
            failure: None,
        })
    }

    pub fn initial_request(&self) -> Option<CloseRequest> {
        if self.entry != Entry::ClearFt {
            return None;
        }
        let rows = self.candidates();
        (rows.len() == 1).then(|| CloseRequest {
            kind: CloseKind::Clear,
            ids: vec![rows[0].0.clone()],
        })
    }

    pub fn set_review(&mut self, plan: ClosePlan) {
        self.stage = Stage::Review(Box::new(plan));
        self.confirmation.clear();
        self.scroll = 0;
        self.failure = None;
    }

    pub fn refresh(&mut self, topology: Topology) {
        self.topology = topology;
        self.checked.clear();
        self.selected = 0;
        self.failure = None;
    }

    pub fn set_failure(&mut self, error: impl Into<String>) {
        self.failure = Some(error.into());
    }
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    pub fn review(&self) -> Option<&ClosePlan> {
        if let Stage::Review(plan) = &self.stage {
            Some(plan)
        } else {
            None
        }
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn confirmation(&self) -> &str {
        &self.confirmation
    }
    pub fn selected(&self) -> usize {
        self.selected
    }
    pub fn scroll(&self) -> u16 {
        self.scroll
    }
    pub fn is_kind(&self) -> bool {
        matches!(self.stage, Stage::Kind)
    }

    pub fn heading(&self) -> String {
        match &self.stage {
            Stage::Kind => "Close or clear?".into(),
            Stage::Select(kind) if self.entry == Entry::ClearFt => {
                format!("{}: choose one ft workspace by ID", kind.title())
            }
            Stage::Select(kind) => format!("{} · {} selected", kind.title(), self.checked.len()),
            Stage::Review(plan) => format!(
                "Review {} · {} workspaces / {} tabs / {} panes",
                plan.kind.title().to_lowercase(),
                plan.workspaces.len(),
                plan.tabs.len(),
                plan.panes.len()
            ),
        }
    }

    pub fn confirm_word(&self) -> &'static str {
        if self.review().is_some_and(|p| p.kind == CloseKind::Clear) {
            "clear"
        } else {
            "close"
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CloseOutcome {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return CloseOutcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            return CloseOutcome::Cancel;
        }
        if key.code == KeyCode::Esc {
            match &self.stage {
                Stage::Kind => return CloseOutcome::Cancel,
                Stage::Select(_) if self.entry == Entry::ClearFt => return CloseOutcome::Cancel,
                Stage::Select(_) => {
                    self.stage = Stage::Kind;
                    self.checked.clear();
                    self.selected = 0;
                    self.query.clear();
                }
                Stage::Review(plan) => {
                    self.stage = Stage::Select(plan.kind);
                    self.confirmation.clear();
                }
            }
            self.failure = None;
            return CloseOutcome::Continue;
        }
        self.failure = None;
        if matches!(self.stage, Stage::Review(_)) {
            match key.code {
                KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(8),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(8),
                KeyCode::Home => self.scroll = 0,
                KeyCode::End => self.scroll = u16::MAX,
                KeyCode::Backspace => {
                    self.confirmation.pop();
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.confirmation.push(c)
                }
                KeyCode::Enter if self.confirmation == self.confirm_word() => {
                    if let Stage::Review(plan) = &self.stage {
                        return CloseOutcome::Confirm(plan.clone());
                    }
                }
                KeyCode::Enter => {
                    self.failure = Some(format!(
                        "Type {} then Enter to confirm",
                        self.confirm_word()
                    ))
                }
                _ => {}
            }
            return CloseOutcome::Continue;
        }
        if self.is_kind() {
            let kind = match key.code {
                KeyCode::Char('p') => Some(CloseKind::Panes),
                KeyCode::Char('t') => Some(CloseKind::Tabs),
                KeyCode::Char('w') => Some(CloseKind::Workspaces),
                KeyCode::Char('c') => Some(CloseKind::Clear),
                KeyCode::Enter => Some(KINDS[self.selected]),
                _ => None,
            };
            if let Some(kind) = kind {
                self.stage = Stage::Select(kind);
                self.selected = 0;
                return CloseOutcome::Continue;
            }
        } else {
            if key.code == KeyCode::F(5) {
                return CloseOutcome::Refresh;
            }
            if let Stage::Select(kind) = self.stage {
                if key.code == KeyCode::Enter {
                    let candidates = self.candidates();
                    let ids = if self.checked.is_empty() || kind == CloseKind::Clear {
                        candidates
                            .get(self.selected)
                            .map(|c| vec![c.0.clone()])
                            .unwrap_or_default()
                    } else {
                        self.checked.iter().cloned().collect()
                    };
                    if !ids.is_empty() {
                        return CloseOutcome::Review(CloseRequest { kind, ids });
                    }
                    return CloseOutcome::Continue;
                }
                if kind != CloseKind::Clear {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('a' | 'A'))
                    {
                        let candidates = self.candidates();
                        let all = candidates.iter().all(|c| self.checked.contains(&c.0));
                        for (id, _) in candidates {
                            if all {
                                self.checked.remove(&id);
                            } else {
                                self.checked.insert(id);
                            }
                        }
                        return CloseOutcome::Continue;
                    }
                    if matches!(
                        key.code,
                        KeyCode::Char(' ') | KeyCode::Tab | KeyCode::BackTab
                    ) {
                        if let Some((id, _)) = self.candidates().get(self.selected) {
                            if !self.checked.remove(id) {
                                self.checked.insert(id.clone());
                            }
                        }
                        if key.code == KeyCode::Tab {
                            self.navigate(1);
                        }
                        if key.code == KeyCode::BackTab {
                            self.navigate(-1);
                        }
                        return CloseOutcome::Continue;
                    }
                }
            }
        }
        match key.code {
            KeyCode::Up => self.navigate(-1),
            KeyCode::Down => self.navigate(1),
            KeyCode::PageUp => self.navigate(-8),
            KeyCode::PageDown => self.navigate(8),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.rows().len().saturating_sub(1),
            KeyCode::Backspace if !self.is_kind() => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCode::Char(c)
                if !self.is_kind()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(c);
                self.selected = 0;
            }
            _ => {}
        }
        CloseOutcome::Continue
    }

    fn navigate(&mut self, delta: isize) {
        let len = self.rows().len();
        if len > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    pub fn rows(&self) -> Vec<DisplayRow> {
        if self.is_kind() {
            return KINDS
                .iter()
                .map(|kind| DisplayRow {
                    title: kind.title().into(),
                    detail: if *kind == CloseKind::Clear {
                        "Keep workspace + fresh shell".into()
                    } else {
                        "Close selected live terminals".into()
                    },
                    tone: RowTone::Normal,
                    checked: false,
                })
                .collect();
        }
        self.candidates().into_iter().map(|(_, row)| row).collect()
    }

    fn candidates(&self) -> Vec<(String, DisplayRow)> {
        let Stage::Select(kind) = self.stage else {
            return vec![];
        };
        let workspace_name = |id: &str| {
            self.topology
                .workspaces
                .iter()
                .find(|w| w.workspace_id == id)
                .map(|w| w.label.as_str())
                .unwrap_or("")
        };
        let raw: Vec<(String, String, String)> = match kind {
            CloseKind::Panes => self
                .topology
                .panes
                .iter()
                .map(|p| {
                    (
                        p.pane_id.clone(),
                        format!(
                            "{} {}",
                            p.pane_id,
                            p.label
                                .as_deref()
                                .or(p.terminal_title_stripped.as_deref())
                                .unwrap_or("pane")
                        ),
                        format!(
                            "{} / {} · {} · {}",
                            workspace_name(&p.workspace_id),
                            p.tab_id,
                            p.agent.as_deref().unwrap_or("shell"),
                            p.cwd.as_deref().unwrap_or("")
                        ),
                    )
                })
                .collect(),
            CloseKind::Tabs => self
                .topology
                .tabs
                .iter()
                .map(|t| {
                    (
                        t.tab_id.clone(),
                        format!("{} {}", t.tab_id, t.label),
                        format!(
                            "{} · {} panes",
                            workspace_name(&t.workspace_id),
                            self.topology
                                .panes
                                .iter()
                                .filter(|p| p.tab_id == t.tab_id)
                                .count()
                        ),
                    )
                })
                .collect(),
            CloseKind::Workspaces | CloseKind::Clear => self
                .topology
                .workspaces
                .iter()
                .filter(|w| self.entry != Entry::ClearFt || w.label == "ft")
                .map(|w| {
                    (
                        w.workspace_id.clone(),
                        format!("{} {}", w.workspace_id, w.label),
                        format!(
                            "{} tabs · {} panes · {}",
                            self.topology
                                .tabs
                                .iter()
                                .filter(|t| t.workspace_id == w.workspace_id)
                                .count(),
                            self.topology
                                .panes
                                .iter()
                                .filter(|p| p.workspace_id == w.workspace_id)
                                .count(),
                            self.topology
                                .panes
                                .iter()
                                .find(|p| p.workspace_id == w.workspace_id)
                                .and_then(|p| p.cwd.as_deref())
                                .unwrap_or("")
                        ),
                    )
                })
                .collect(),
        };
        let mut scored = raw
            .into_iter()
            .filter_map(|(id, title, detail)| {
                fuzzy::score(&self.query, &format!("{title} {detail}")).map(|score| {
                    (
                        score,
                        id.clone(),
                        DisplayRow {
                            title: clean(&title),
                            detail: clean(&detail),
                            tone: RowTone::Normal,
                            checked: self.checked.contains(&id),
                        },
                    )
                })
            })
            .collect::<Vec<_>>();
        scored.sort_by_key(|row| Reverse(row.0));
        scored.into_iter().map(|(_, id, row)| (id, row)).collect()
    }

    pub fn review_text(&self) -> String {
        let Some(plan) = self.review() else {
            return String::new();
        };
        let mut lines =
            vec!["Closing terminals stops their processes. This cannot be undone.".into()];
        if let Some(cwd) = &plan.keeper_cwd {
            lines.push(format!(
                "Keep the same workspace with a fresh shell in {}.",
                clean(cwd)
            ));
        }
        for workspace in &plan.workspaces {
            lines.push(format!(
                "\nWorkspace {}  {}",
                workspace.workspace_id,
                clean(&workspace.label)
            ));
            for tab in plan
                .tabs
                .iter()
                .filter(|t| t.workspace_id == workspace.workspace_id)
            {
                lines.push(format!("  Tab {}  {}", tab.tab_id, clean(&tab.label)));
                for reviewed in plan.panes.iter().filter(|p| p.pane.tab_id == tab.tab_id) {
                    lines.push(format!(
                        "    {}  {}",
                        reviewed.pane.pane_id,
                        reviewed.pane.terminal_id.as_deref().unwrap_or("unknown")
                    ));
                    lines.push(format!(
                        "    Programs: {}",
                        clean(&reviewed.process.description())
                    ));
                    lines.push(format!(
                        "    Cwd: {}",
                        clean(reviewed.pane.cwd.as_deref().unwrap_or("unknown"))
                    ));
                }
            }
        }
        lines.join("\n")
    }
}

fn clean(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}
