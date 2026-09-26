use herdr_ferry::close_plan::{CloseKind, ClosePlan};
use herdr_ferry::close_worker::{JobStatus, JobStore};
use std::fs;

fn plan() -> ClosePlan {
    ClosePlan {
        kind: CloseKind::Panes,
        panes: vec![],
        tabs: vec![],
        workspaces: vec![],
        caller_pane_id: "source".into(),
        caller_tab_id: None,
        keeper_cwd: None,
    }
}

#[test]
fn tests_that_only_this_session_claims_confirmed_work_once() {
    let dir = tempfile::tempdir().unwrap();
    let one = JobStore::new(dir.path(), "/session/one.sock").unwrap();
    let two = JobStore::new(dir.path(), "/session/two.sock").unwrap();
    let ticket = one.enqueue(&plan()).unwrap();
    assert!(two.claim().unwrap().is_empty());
    let claimed = one.claim().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, ticket.id);
    assert!(one.claim().unwrap().is_empty());
    assert!(matches!(
        one.status(&ticket.id).unwrap(),
        JobStatus::Running
    ));
}

#[test]
fn tests_that_cancelled_handoff_never_runs_later() {
    let dir = tempfile::tempdir().unwrap();
    let store = JobStore::new(dir.path(), "/session/one.sock").unwrap();
    let ticket = store.enqueue(&plan()).unwrap();
    assert!(store.revoke(&ticket.id).unwrap());
    assert!(store.claim().unwrap().is_empty());
}

#[test]
fn tests_that_claimed_work_cannot_be_cancelled_or_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let store = JobStore::new(dir.path(), "/session/one.sock").unwrap();
    let ticket = store.enqueue(&plan()).unwrap();
    store.claim().unwrap();
    assert!(!store.revoke(&ticket.id).unwrap());
    let reopened = JobStore::new(dir.path(), "/session/one.sock").unwrap();
    assert!(reopened.claim().unwrap().is_empty());
}

#[test]
fn tests_that_expired_work_is_reported_without_closing() {
    let dir = tempfile::tempdir().unwrap();
    let store = JobStore::new(dir.path(), "/session/one.sock").unwrap();
    let ticket = store.enqueue(&plan()).unwrap();
    let path = dir.path().join(format!("{}.pending.json", ticket.id));
    let mut json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    json["confirmed_at"] = 0.into();
    fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(store.claim().unwrap().is_empty());
    assert!(
        matches!(store.status(&ticket.id).unwrap(), JobStatus::Finished(report) if report.errors.join(" ").contains("expired"))
    );
}

#[test]
fn tests_that_result_and_error_survive_popup_exit() {
    let dir = tempfile::tempdir().unwrap();
    let store = JobStore::new(dir.path(), "/session/one.sock").unwrap();
    let ticket = store.enqueue(&plan()).unwrap();
    let claimed = store.claim().unwrap().pop().unwrap();
    let report = herdr_ferry::close_ops::CloseReport {
        completed: 2,
        skipped: 1,
        failed: 1,
        errors: vec!["target changed".into()],
        keeper: None,
    };
    store.finish(&claimed, &report).unwrap();
    let reopened = JobStore::new(dir.path(), "/session/one.sock").unwrap();
    assert!(
        matches!(reopened.status(&ticket.id).unwrap(), JobStatus::Finished(r) if r.completed==2 && r.failed==1 && r.errors==["target changed"])
    );
    assert!(fs::read_to_string(ticket.report_path)
        .unwrap()
        .contains("target changed"));
    assert!(store.claim().unwrap().is_empty());
}
