use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use herdr_ferry::app::{MoveTabRequest, TabDestination};
use herdr_ferry::herdr::Herdr;
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
    echo '{"result":{"layout":{"workspace_id":"w1","tab_id":"w1:t1","zoomed":false,"area":{"x":0,"y":0,"width":100,"height":20},"focused_pane_id":"w1:p1","panes":[{"pane_id":"w1:p1","focused":true,"rect":{"x":0,"y":0,"width":50,"height":20}},{"pane_id":"w1:p2","focused":false,"rect":{"x":50,"y":0,"width":50,"height":20}}],"splits":[{"direction":"right","ratio":0.5,"rect":{"x":0,"y":0,"width":100,"height":20}}]}}}'
    ;;
  "workspace list")
    echo '{"result":{"workspaces":[{"workspace_id":"w1","label":"source"},{"workspace_id":"w2","label":"target"}]}}'
    ;;
  "tab list")
    echo '{"result":{"tabs":[{"tab_id":"w1:t1","workspace_id":"w1","label":"main","pane_count":2},{"tab_id":"w2:t1","workspace_id":"w2","label":"target","pane_count":1}]}}'
    ;;
  "pane list")
    echo '{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1"},{"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1"}]}}'
    ;;
  "pane move")
    case "$*" in
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
    let log = fake.log();
    assert!(log.contains("pane move w1:p1 --new-tab --workspace w2 --label main --no-focus"));
    assert!(log.contains(
        "pane move w1:p2 --tab w2:t9 --split right --target-pane w2:p9 --ratio 0.5 --no-focus"
    ));
    assert!(log.ends_with("workspace focus w2\ntab focus w2:t9\n"));
}
