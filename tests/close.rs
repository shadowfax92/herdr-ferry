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
    keeper_available: bool,
    keeper_disappears: bool,
    after_close: Option<fn(&mut Topology)>,
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
            keeper_available: false,
            keeper_disappears: false,
            after_close: None,
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
        if let Some(change) = self.after_close {
            change(&mut t);
        }
        Ok(())
    }
    fn create_keeper(&self, workspace: &str, cwd: &str) -> Result<PaneInfo> {
        if !self.keeper_available {
            bail!("keeper unavailable");
        }
        let pane: PaneInfo = serde_json::from_value(serde_json::json!({"pane_id":"w1:p9","tab_id":"w1:t9","workspace_id":workspace,"terminal_id":"keeper-terminal","cwd":cwd})).unwrap();
        if self.keeper_disappears {
            return Ok(pane);
        }
        let mut t = self.topology.borrow_mut();
        t.panes.push(pane.clone());
        t.tabs.push(
            serde_json::from_value(
                serde_json::json!({"tab_id":"w1:t9","workspace_id":workspace,"label":"shell"}),
            )
            .unwrap(),
        );
        Ok(pane)
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

#[test]
fn tests_that_clear_retains_workspace_and_new_shell_with_reviewed_cwd() {
    let mut backend = Memory::new();
    backend.keeper_available = true;
    let plan = backend.plan(CloseKind::Clear, &["w1"]);
    let report = execute(&backend, &plan);
    assert_eq!((report.completed, report.failed), (2, 0));
    assert_eq!(report.keeper.as_ref().unwrap().cwd.as_deref(), Some("/tmp"));
    let t = backend.topology.borrow();
    assert!(t.workspaces.iter().any(|w| w.workspace_id == "w1"));
    assert_eq!(t.panes.iter().filter(|p| p.workspace_id == "w1").count(), 1);
    assert_eq!(
        t.panes
            .iter()
            .find(|p| p.workspace_id == "w1")
            .unwrap()
            .terminal_id
            .as_deref(),
        Some("keeper-terminal")
    );
}

#[test]
fn tests_that_failed_keeper_creation_preserves_every_old_terminal() {
    let backend = Memory::new();
    let report = execute(&backend, &backend.plan(CloseKind::Clear, &["w1"]));
    assert_eq!((report.completed, report.failed), (0, 2));
    assert!(report.errors.join(" ").contains("keeper unavailable"));
    assert!(backend.closed.borrow().is_empty());
}

#[test]
fn tests_that_clear_keeps_new_concurrent_tabs_outside_the_snapshot() {
    let mut backend = Memory::new();
    backend.keeper_available = true;
    let plan = backend.plan(CloseKind::Clear, &["w1"]);
    let pane: PaneInfo = serde_json::from_value(serde_json::json!({"pane_id":"w1:p8","tab_id":"w1:t8","workspace_id":"w1","terminal_id":"concurrent"})).unwrap();
    backend.topology.borrow_mut().panes.push(pane);
    backend.topology.borrow_mut().tabs.push(
        serde_json::from_value(serde_json::json!({"tab_id":"w1:t8","workspace_id":"w1"})).unwrap(),
    );
    let report = execute(&backend, &plan);
    assert_eq!((report.completed, report.failed), (2, 0));
    assert!(report.keeper.is_some());
    assert!(backend
        .topology
        .borrow()
        .panes
        .iter()
        .any(|p| p.terminal_id.as_deref() == Some("concurrent")));
}

#[test]
fn tests_that_partial_clear_preserves_keeper_and_reports_failure() {
    let mut backend = Memory::new();
    backend.keeper_available = true;
    backend.fail = Some("w1:p1".into());
    let report = execute(&backend, &backend.plan(CloseKind::Clear, &["w1"]));
    assert_eq!((report.completed, report.failed), (1, 1));
    let keeper = report.keeper.unwrap();
    assert!(backend
        .topology
        .borrow()
        .panes
        .iter()
        .any(|p| p.pane_id == keeper.pane_id));
}

#[test]
fn tests_that_noop_clear_does_not_create_an_extra_keeper() {
    let mut backend = Memory::new();
    backend.keeper_available = true;
    let plan = backend.plan(CloseKind::Clear, &["w1"]);
    execute(&backend, &plan);
    let report = execute(&backend, &plan);
    assert_eq!((report.completed, report.skipped, report.failed), (0, 2, 0));
    assert!(report.keeper.is_none());
    assert_eq!(
        backend
            .topology
            .borrow()
            .panes
            .iter()
            .filter(|p| p.workspace_id == "w1")
            .count(),
        1
    );
}

#[test]
fn tests_that_terminal_identity_resolves_a_stale_caller_before_review() {
    let backend = Memory::new();
    backend.topology.borrow_mut().panes[0].pane_id = "w1:p9".into();
    let mut process = backend.processes.borrow_mut().remove("w1:p1").unwrap();
    process.pane_id = "w1:p9".into();
    backend
        .processes
        .borrow_mut()
        .insert("w1:p9".into(), process);
    let plan = ClosePlan::capture(
        &backend,
        CloseRequest {
            kind: CloseKind::Tabs,
            ids: vec!["w1:t1".into()],
        },
        "term-a",
        "/tmp",
    )
    .unwrap();
    let report = execute(&backend, &plan);
    assert_eq!(report.completed, 2);
    assert_eq!(*backend.closed.borrow(), ["w1:p2", "w1:p9"]);
}

#[test]
fn tests_that_keeper_disappearance_keeps_its_identity_in_the_failed_report() {
    let mut backend = Memory::new();
    backend.keeper_available = true;
    backend.keeper_disappears = true;
    let report = execute(&backend, &backend.plan(CloseKind::Clear, &["w1"]));
    assert_eq!((report.completed, report.failed), (0, 2));
    assert_eq!(report.keeper.unwrap().pane_id, "w1:p9");
    assert!(backend.closed.borrow().is_empty());
}

#[test]
fn tests_that_membership_change_during_execution_stops_remaining_closures() {
    let mut backend = Memory::new();
    backend.after_close = Some(|t| {
        let mut pane = t.panes[0].clone();
        pane.pane_id = "w1:p7".into();
        pane.terminal_id = Some("unreviewed".into());
        t.panes.push(pane);
    });
    let report = execute(&backend, &backend.plan(CloseKind::Tabs, &["w1:t1"]));
    assert_eq!((report.completed, report.failed), (1, 1));
    assert_eq!(*backend.closed.borrow(), ["w1:p2"]);
}

#[test]
fn tests_that_replaced_terminal_identity_is_not_closed() {
    let backend = Memory::new();
    let plan = backend.plan(CloseKind::Panes, &["w1:p1"]);
    backend.topology.borrow_mut().panes[0].terminal_id = Some("replacement".into());
    let report = execute(&backend, &plan);
    assert_eq!(report.failed, 1);
    assert!(backend.closed.borrow().is_empty());
}

#[test]
fn tests_that_implicit_worktree_group_closure_is_rejected_before_mutation() {
    let backend = Memory::new();
    backend.topology.borrow_mut().workspaces=serde_json::from_value(serde_json::json!([
        {"workspace_id":"w1","label":"root","worktree":{"repo_key":"repo","is_linked_worktree":false}},
        {"workspace_id":"w2","label":"linked","worktree":{"repo_key":"repo","is_linked_worktree":true}}
    ])).unwrap();
    let plan = ClosePlan::capture(
        &backend,
        CloseRequest {
            kind: CloseKind::Workspaces,
            ids: vec!["w1".into()],
        },
        "w1:p1",
        "/tmp",
    );
    assert!(
        plan.is_err(),
        "last pane can cascade to unselected linked workspaces when Herdr confirm_close=false"
    );
    assert!(backend.closed.borrow().is_empty());
}
