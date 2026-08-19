use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use herdr_ferry::app::{
    MergeWorkspaceRequest, MovePaneRequest, MoveTabRequest, PaneDestination, PaneSource,
    TabDestination,
};
use herdr_ferry::herdr::Herdr;
use herdr_ferry::layout::SplitDirection;
use herdr_ferry::move_ops::Mover;
use tempfile::tempdir;

struct FakeHerdr {
    _directory: tempfile::TempDir,
    binary: PathBuf,
}

impl FakeHerdr {
    fn new() -> Self {
        let directory = tempdir().unwrap();
        let binary = directory.path().join("herdr");
        fs::write(
            &binary,
            r#"#!/bin/sh
echo "$*" >> "$0.log"
case "$1 $2" in
  "pane layout")
    case "$*" in
      *"w1:p1"*|*"w1:p2"*)
        echo '{"result":{"layout":{"workspace_id":"w1","tab_id":"w1:t1","zoomed":false,"area":{"x":0,"y":0,"width":100,"height":20},"focused_pane_id":"w1:p1","panes":[{"pane_id":"w1:p1","focused":true,"rect":{"x":0,"y":0,"width":50,"height":20}},{"pane_id":"w1:p2","focused":false,"rect":{"x":50,"y":0,"width":50,"height":20}}],"splits":[{"direction":"right","ratio":0.5,"rect":{"x":0,"y":0,"width":100,"height":20}}]}}}'
        ;;
      *"w1:p3"*)
        echo '{"result":{"layout":{"workspace_id":"w1","tab_id":"w1:t2","zoomed":false,"area":{"x":0,"y":0,"width":100,"height":20},"focused_pane_id":"w1:p3","panes":[{"pane_id":"w1:p3","focused":false,"rect":{"x":0,"y":0,"width":100,"height":20}}],"splits":[]}}}'
        ;;
      *"w2:p1"*)
        echo '{"result":{"layout":{"workspace_id":"w2","tab_id":"w2:t1","zoomed":false,"area":{"x":0,"y":0,"width":100,"height":20},"focused_pane_id":"w2:p1","panes":[{"pane_id":"w2:p1","focused":false,"rect":{"x":0,"y":0,"width":100,"height":20}}],"splits":[]}}}'
        ;;
      *)
        exit 3
        ;;
    esac
    ;;
  "workspace list")
    echo '{"result":{"workspaces":[{"workspace_id":"w1","label":"source"},{"workspace_id":"w2","label":"target"}]}}'
    ;;
  "tab list")
    echo '{"result":{"tabs":[{"tab_id":"w1:t1","workspace_id":"w1","label":"main","pane_count":2},{"tab_id":"w1:t2","workspace_id":"w1","label":"logs","pane_count":1},{"tab_id":"w2:t1","workspace_id":"w2","label":"target","pane_count":1}]}}'
    ;;
  "pane list")
    echo '{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","focused":true},{"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1"},{"pane_id":"w1:p3","tab_id":"w1:t2","workspace_id":"w1"},{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2"}]}}'
    ;;
  "pane move")
    case "$*" in
      *"w1:p1 --new-workspace"*)
        echo '{"result":{"move_result":{"changed":true,"pane":{"pane_id":"w3:p1","tab_id":"w3:t1","workspace_id":"w3"}}}}'
        ;;
      *"w1:p2 --tab w3:t1"*)
        echo '{"result":{"move_result":{"changed":true,"pane":{"pane_id":"w3:p2","tab_id":"w3:t1","workspace_id":"w3"}}}}'
        ;;
      *"w1:p3 --new-tab --workspace w3"*)
        echo '{"result":{"move_result":{"changed":true,"pane":{"pane_id":"w3:p3","tab_id":"w3:t2","workspace_id":"w3"}}}}'
        ;;
      *"w1:p1 --tab w2:t1"*)
        echo '{"result":{"move_result":{"changed":true,"pane":{"pane_id":"w2:p9","tab_id":"w2:t1","workspace_id":"w2"}}}}'
        ;;
      *"w1:p3 --tab w2:t1"*)
        echo '{"result":{"move_result":{"changed":true,"pane":{"pane_id":"w2:p8","tab_id":"w2:t1","workspace_id":"w2"}}}}'
        ;;
      *"w1:p3 --new-tab --workspace w2"*)
        if [ -f "$0.fail-second-tab" ]; then
          echo 'injected second-tab failure' >&2
          exit 4
        fi
        echo '{"result":{"move_result":{"changed":true,"pane":{"pane_id":"w2:p7","tab_id":"w2:t8","workspace_id":"w2"}}}}'
        ;;
      *"--new-tab"*)
        echo '{"result":{"move_result":{"changed":true,"pane":{"pane_id":"w2:p9","tab_id":"w2:t9","workspace_id":"w2"}}}}'
        ;;
      *"--tab w2:t9"*)
        echo '{"result":{"move_result":{"changed":true,"pane":{"pane_id":"w2:p8","tab_id":"w2:t9","workspace_id":"w2"}}}}'
        ;;
      *)
        exit 3
        ;;
    esac
    ;;
  "workspace focus"|"tab focus"|"plugin pane")
    echo '{"result":{"type":"ok"}}'
    ;;
  *)
    echo "unsupported: $*" >&2
    exit 2
    ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&binary, permissions).unwrap();
        Self {
            _directory: directory,
            binary,
        }
    }

    fn client(&self) -> Herdr {
        Herdr::new(&self.binary)
    }

    fn log(&self) -> String {
        fs::read_to_string(self.binary.with_extension("log")).unwrap_or_default()
    }

    fn fail_second_tab(&self) {
        fs::write(self.binary.with_extension("fail-second-tab"), "").unwrap();
    }
}

#[test]
fn launch_opens_the_native_popup_with_source_context() {
    let fake = FakeHerdr::new();

    fake.client().launch_picker("w1:p1").unwrap();

    assert_eq!(
        fake.log().trim(),
        "plugin pane open --plugin shadowfax.ferry --entrypoint picker --placement popup --width 76 --height 24 --env HERDR_FERRY_SOURCE_PANE_ID=w1:p1"
    );
}

#[test]
fn whole_tab_move_replays_the_reported_split_with_returned_pane_ids() {
    let fake = FakeHerdr::new();
    let mover = Mover::new(fake.client());

    let summary = mover
        .move_tab(&MoveTabRequest {
            tab_ids: vec!["w1:t1".into()],
            destination: TabDestination::Workspace {
                workspace_id: "w2".into(),
            },
        })
        .unwrap();

    assert_eq!(summary.message, "Tab “main” moved to w2");
    assert_eq!(summary.moved_panes, 2);
    assert_eq!(summary.moved_tabs, 1);
    let log = fake.log();
    assert!(log.contains("pane move w1:p1 --new-tab --workspace w2 --label main --no-focus"));
    assert!(log.contains(
        "pane move w1:p2 --tab w2:t9 --split right --target-pane w2:p9 --ratio 0.5 --no-focus"
    ));
    assert!(log.ends_with("workspace focus w2\ntab focus w2:t9\n"));
}

#[test]
fn multiple_panes_append_in_selection_order_and_focus_once() {
    let fake = FakeHerdr::new();
    let mover = Mover::new(fake.client());

    let summary = mover
        .move_pane(&MovePaneRequest {
            sources: vec![
                PaneSource {
                    pane_id: "w1:p1".into(),
                    expected_tab_id: "w1:t1".into(),
                },
                PaneSource {
                    pane_id: "w1:p3".into(),
                    expected_tab_id: "w1:t2".into(),
                },
            ],
            direction: SplitDirection::Down,
            destination: PaneDestination::ExistingTab {
                tab_id: "w2:t1".into(),
            },
        })
        .unwrap();

    assert_eq!(summary.message, "2 panes moved to w2:t1");
    let log = fake.log();
    assert!(log.contains("pane move w1:p1 --tab w2:t1 --split down --no-focus"));
    assert!(log.contains("pane move w1:p3 --tab w2:t1 --split down --target-pane w2:p9 --no-focus"));
    assert_eq!(log.matches("workspace focus").count(), 1);
    assert_eq!(log.matches("tab focus").count(), 1);
}

#[test]
fn multiple_tabs_append_to_one_new_workspace() {
    let fake = FakeHerdr::new();
    let mover = Mover::new(fake.client());

    let summary = mover
        .move_tab(&MoveTabRequest {
            tab_ids: vec!["w1:t1".into(), "w1:t2".into()],
            destination: TabDestination::NewWorkspace {
                label: Some("combined".into()),
            },
        })
        .unwrap();

    assert_eq!(summary.message, "2 tabs moved to w3");
    assert_eq!(summary.moved_panes, 3);
    assert_eq!(summary.moved_tabs, 2);
    let log = fake.log();
    let first = log
        .find("pane move w1:p1 --new-workspace --label combined --tab-label main --no-focus")
        .unwrap();
    let second = log
        .find("pane move w1:p3 --new-tab --workspace w3 --label logs --no-focus")
        .unwrap();
    assert!(first < second);
    assert!(log.contains(
        "pane move w1:p2 --tab w3:t1 --split right --target-pane w3:p1 --ratio 0.5 --no-focus"
    ));
    assert!(log.ends_with("workspace focus w3\ntab focus w3:t1\n"));
}

#[test]
fn workspace_merge_appends_every_tab_in_source_order() {
    let fake = FakeHerdr::new();
    let mover = Mover::new(fake.client());

    let summary = mover
        .merge_workspace(&MergeWorkspaceRequest {
            source_workspace_id: "w1".into(),
            destination_workspace_id: "w2".into(),
        })
        .unwrap();

    assert_eq!(
        summary.message,
        "Workspace “source” merged into “target” · 2 tabs"
    );
    assert_eq!(summary.moved_tabs, 2);
    let log = fake.log();
    let first = log
        .find("pane move w1:p1 --new-tab --workspace w2 --label main --no-focus")
        .unwrap();
    let second = log
        .find("pane move w1:p3 --new-tab --workspace w2 --label logs --no-focus")
        .unwrap();
    assert!(first < second);
}

#[test]
fn invalid_batch_is_rejected_before_any_move() {
    let fake = FakeHerdr::new();
    let mover = Mover::new(fake.client());

    let error = mover
        .move_tab(&MoveTabRequest {
            tab_ids: vec!["w1:t1".into(), "missing".into()],
            destination: TabDestination::Workspace {
                workspace_id: "w2".into(),
            },
        })
        .unwrap_err();

    assert!(error.to_string().contains("selected tab no longer exists"));
    assert!(!fake.log().contains("pane move"));
}

#[test]
fn partial_batch_failure_reports_completed_work() {
    let fake = FakeHerdr::new();
    fake.fail_second_tab();
    let mover = Mover::new(fake.client());

    let error = mover
        .move_tab(&MoveTabRequest {
            tab_ids: vec!["w1:t1".into(), "w1:t2".into()],
            destination: TabDestination::Workspace {
                workspace_id: "w2".into(),
            },
        })
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("after 1/2 tabs and 2/3 panes"));
    assert!(message.contains("all moved panes are still live"));
    assert!(!fake.log().contains("workspace focus"));
}
