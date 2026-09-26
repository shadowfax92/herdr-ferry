use herdr_ferry::close_app::{CloseApp, CloseOutcome, Entry};
use herdr_ferry::close_plan::{CloseKind, ClosePlan};
use herdr_ferry::herdr::Topology;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn topology() -> Topology {
    Topology {
        workspaces:serde_json::from_value(serde_json::json!([{"workspace_id":"w1","label":"ft"},{"workspace_id":"w2","label":"ft"}])).unwrap(),
        tabs:serde_json::from_value(serde_json::json!([{"tab_id":"w1:t1","workspace_id":"w1","label":"build"},{"tab_id":"w2:t1","workspace_id":"w2","label":"safe"}])).unwrap(),
        panes:serde_json::from_value(serde_json::json!([{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","terminal_id":"term-a","label":"agent","cwd":"/project"},{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","terminal_id":"term-b","label":"shell"}])).unwrap(),
    }
}
fn key(app: &mut CloseApp, code: KeyCode) -> CloseOutcome {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn review() -> ClosePlan {
    ClosePlan {
        kind: CloseKind::Panes,
        panes: vec![],
        tabs: vec![],
        workspaces: vec![],
        caller_pane_id: "w1:p1".into(),
        caller_tab_id: None,
        keeper_cwd: None,
    }
}

#[test]
fn tests_that_selection_requires_a_separate_typed_confirmation() {
    let mut app = CloseApp::new(topology(), Entry::Close).unwrap();
    key(&mut app, KeyCode::Char('p'));
    assert!(matches!(key(&mut app,KeyCode::Enter),CloseOutcome::Review(r) if r.ids==["w1:p1"]));
    app.set_review(review());
    assert!(matches!(
        key(&mut app, KeyCode::Enter),
        CloseOutcome::Continue
    ));
    for c in "close".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    assert!(matches!(
        key(&mut app, KeyCode::Enter),
        CloseOutcome::Confirm(_)
    ));
}

#[test]
fn tests_that_multiselect_uses_visible_matches_and_preserves_checked_rows() {
    let mut app = CloseApp::new(topology(), Entry::Close).unwrap();
    key(&mut app, KeyCode::Char('p'));
    for c in "agent".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    for _ in 0..5 {
        key(&mut app, KeyCode::Backspace);
    }
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Tab);
    assert!(matches!(key(&mut app,KeyCode::Enter),CloseOutcome::Review(r) if r.ids.len()==2));
}

#[test]
fn tests_that_escape_and_control_c_cancel_without_confirmation() {
    let mut app = CloseApp::new(topology(), Entry::Close).unwrap();
    key(&mut app, KeyCode::Char('t'));
    key(&mut app, KeyCode::Enter);
    app.set_review(review());
    key(&mut app, KeyCode::Esc);
    assert!(app.review().is_none());
    assert!(matches!(
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        CloseOutcome::Cancel
    ));
}

#[test]
fn tests_that_duplicate_ft_requires_one_explicit_identified_workspace() {
    let mut app = CloseApp::new(topology(), Entry::ClearFt).unwrap();
    assert!(app.initial_request().is_none());
    let rows = app.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r.title.contains("w1")));
    assert!(rows.iter().any(|r| r.title.contains("w2")));
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    key(&mut app, KeyCode::Down);
    assert!(
        matches!(key(&mut app,KeyCode::Enter),CloseOutcome::Review(r) if r.kind==CloseKind::Clear && r.ids==["w2"])
    );
}

#[test]
fn tests_that_missing_ft_is_actionable_and_single_ft_only_preselects() {
    let mut t = topology();
    t.workspaces.clear();
    assert!(CloseApp::new(t, Entry::ClearFt)
        .err()
        .unwrap()
        .to_string()
        .contains("No workspace labelled ft"));
    let mut t = topology();
    t.workspaces.pop();
    let app = CloseApp::new(t, Entry::ClearFt).unwrap();
    let request = app.initial_request().unwrap();
    assert_eq!(request.ids, ["w1"]);
    assert_eq!(request.kind, CloseKind::Clear);
    assert!(app.review().is_none());
}

#[test]
fn tests_that_review_renders_identities_programs_and_confirmation() {
    use ratatui::{backend::TestBackend, Terminal};
    let mut app = CloseApp::new(topology(), Entry::Close).unwrap();
    let t = topology();
    let mut plan = review();
    plan.workspaces = vec![t.workspaces[0].clone()];
    plan.tabs = vec![t.tabs[0].clone()];
    plan.panes=vec![herdr_ferry::close_plan::ReviewedPane {pane:t.panes[0].clone(),process:serde_json::from_value(serde_json::json!({"pane_id":"w1:p1","shell_pid":42,"foreground_processes":[{"pid":43,"name":"sleep"}]})).unwrap()}];
    app.set_review(plan);
    let mut terminal = Terminal::new(TestBackend::new(90, 28)).unwrap();
    terminal
        .draw(|frame| herdr_ferry::close_ui::render(&app, frame))
        .unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    for expected in [
        "w1:p1",
        "term-a",
        "sleep (43)",
        "Type close",
        "cannot be undone",
    ] {
        assert!(text.contains(expected), "missing {expected}");
    }
}
