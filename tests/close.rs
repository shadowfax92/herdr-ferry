use std::cell::RefCell;
use std::collections::BTreeMap;

use anyhow::{bail, Result};
use herdr_ferry::close_ops::execute;
use herdr_ferry::close_plan::{CloseBackend, CloseKind, ClosePlan, CloseRequest, ProcessInfo};
use herdr_ferry::herdr::{PaneInfo, Topology};

struct Memory {
    topology: RefCell<Topology>,
    processes: RefCell<BTreeMap<String, ProcessInfo>>,
    closed: RefCell<Vec<String>>,
    fail: Option<String>,
}

impl Memory {
    fn new() -> Self {
        let panes = serde_json::from_value(serde_json::json!([
            {"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","terminal_id":"term-a","cwd":"/tmp"},
            {"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1","terminal_id":"term-b","cwd":"/tmp"},
            {"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","terminal_id":"term-c","cwd":"/tmp"}
        ])).unwrap();
        Self {
            topology: RefCell::new(Topology {
                panes,
                tabs: serde_json::from_value(serde_json::json!([
                    {"tab_id":"w1:t1","workspace_id":"w1","label":"jobs"},
                    {"tab_id":"w2:t1","workspace_id":"w2","label":"keep"}
                ]))
                .unwrap(),
                workspaces: serde_json::from_value(serde_json::json!([
                    {"workspace_id":"w1","label":"ft"},{"workspace_id":"w2","label":"safe"}
                ]))
                .unwrap(),
            }),
            processes: RefCell::new(
                ["w1:p1", "w1:p2", "w2:p1"]
                    .into_iter()
                    .enumerate()
                    .map(|(i, id)| {
                        (
                            id.into(),
                            serde_json::from_value(serde_json::json!({
                                "pane_id":id,"shell_pid":100+i,"foreground_process_group_id":100+i,
                                "foreground_processes":[{"pid":100+i,"name":"sh"}]
                            }))
                            .unwrap(),
                        )
                    })
                    .collect(),
            ),
            closed: RefCell::new(vec![]),
            fail: None,
        }
    }

    fn plan(&self, kind: CloseKind, ids: &[&str]) -> ClosePlan {
        ClosePlan::capture(
            self,
            CloseRequest {
                kind,
                ids: ids.iter().map(|id| (*id).into()).collect(),
            },
            "w1:p1",
            "/tmp",
        )
        .unwrap()
    }
}

impl CloseBackend for Memory {
    fn topology(&self) -> Result<Topology> {
        Ok(self.topology.borrow().clone())
    }
    fn process_info(&self, id: &str) -> Result<ProcessInfo> {
        self.processes
            .borrow()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing process"))
    }
    fn close_pane(&self, id: &str) -> Result<()> {
        if self.fail.as_deref() == Some(id) {
            bail!("injected close failure");
        }
        self.closed.borrow_mut().push(id.into());
        let mut t = self.topology.borrow_mut();
        t.panes.retain(|p| p.pane_id != id);
        let tabs: Vec<_> = t.panes.iter().map(|p| p.tab_id.clone()).collect();
        t.tabs.retain(|tab| tabs.contains(&tab.tab_id));
        let workspaces: Vec<_> = t.panes.iter().map(|p| p.workspace_id.clone()).collect();
        t.workspaces
            .retain(|w| workspaces.contains(&w.workspace_id));
        Ok(())
    }
    fn create_keeper(&self, _: &str, _: &str) -> Result<PaneInfo> {
        bail!("keeper unavailable")
    }
}

#[test]
fn tests_that_confirmed_tab_closure_preserves_unselected_terminals_and_closes_caller_last() {
    let backend = Memory::new();
    let plan = backend.plan(CloseKind::Tabs, &["w1:t1"]);
    let report = execute(&backend, &plan);
    assert_eq!((report.completed, report.skipped, report.failed), (2, 0, 0));
    assert_eq!(*backend.closed.borrow(), ["w1:p2", "w1:p1"]);
    assert_eq!(backend.topology.borrow().panes[0].pane_id, "w2:p1");
    assert_eq!(backend.topology.borrow().workspaces[0].workspace_id, "w2");
}

#[test]
fn tests_that_workspace_closure_removes_only_reviewed_workspace() {
    let backend = Memory::new();
    let report = execute(&backend, &backend.plan(CloseKind::Workspaces, &["w1"]));
    assert_eq!(report.completed, 2);
    assert_eq!(backend.topology.borrow().workspaces.len(), 1);
}

#[test]
fn tests_that_new_member_aborts_before_any_closure() {
    let backend = Memory::new();
    let plan = backend.plan(CloseKind::Tabs, &["w1:t1"]);
    let mut extra = backend.topology.borrow().panes[0].clone();
    extra.pane_id = "w1:p3".into();
    extra.terminal_id = Some("new-terminal".into());
    backend.topology.borrow_mut().panes.push(extra);
    let report = execute(&backend, &plan);
    assert_eq!(report.completed, 0);
    assert_eq!(report.failed, 2);
    assert!(report.errors.join(" ").contains("changed"));
    assert!(backend.closed.borrow().is_empty());
}

#[test]
fn tests_that_moved_terminal_is_not_treated_as_already_closed() {
    let backend = Memory::new();
    let plan = backend.plan(CloseKind::Panes, &["w1:p1"]);
    let mut t = backend.topology.borrow_mut();
    t.panes[0].pane_id = "w2:p9".into();
    t.panes[0].tab_id = "w2:t1".into();
    t.panes[0].workspace_id = "w2".into();
    drop(t);
    let report = execute(&backend, &plan);
    assert_eq!((report.completed, report.skipped, report.failed), (0, 0, 1));
    assert!(backend.closed.borrow().is_empty());
}

#[test]
fn tests_that_changed_program_requires_new_confirmation() {
    let backend = Memory::new();
    let plan = backend.plan(CloseKind::Panes, &["w1:p1"]);
    backend
        .processes
        .borrow_mut()
        .get_mut("w1:p1")
        .unwrap()
        .foreground_process_group_id = Some(999);
    let report = execute(&backend, &plan);
    assert_eq!(report.failed, 1);
    assert!(backend.closed.borrow().is_empty());
}

#[test]
fn tests_that_already_closed_targets_are_skipped_on_repeat() {
    let backend = Memory::new();
    let plan = backend.plan(CloseKind::Tabs, &["w1:t1"]);
    assert_eq!(execute(&backend, &plan).completed, 2);
    let report = execute(&backend, &plan);
    assert_eq!((report.completed, report.skipped, report.failed), (0, 2, 0));
}

#[test]
fn tests_that_partial_failure_reports_completed_and_failed_targets() {
    let mut backend = Memory::new();
    backend.fail = Some("w1:p1".into());
    let report = execute(&backend, &backend.plan(CloseKind::Tabs, &["w1:t1"]));
    assert_eq!((report.completed, report.skipped, report.failed), (1, 0, 1));
    assert!(report.errors.join(" ").contains("injected close failure"));
    assert_eq!(backend.topology.borrow().panes.len(), 2);
}

#[test]
fn tests_that_empty_or_unknown_selection_is_rejected() {
    let backend = Memory::new();
    for ids in [vec![], vec!["missing".into()]] {
        assert!(ClosePlan::capture(
            &backend,
            CloseRequest {
                kind: CloseKind::Panes,
                ids
            },
            "w1:p1",
            "/tmp"
        )
        .is_err());
    }
}
