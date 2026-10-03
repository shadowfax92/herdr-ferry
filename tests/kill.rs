use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use herdr_ferry::herdr::Herdr;
use herdr_ferry::kill::{KillPlan, KillScope, KillTarget};
use herdr_ferry::kill_ops;
use serde_json::{json, Value};
use tempfile::{tempdir, TempDir};

/// A scripted `herdr` that serves a topology, logs every call, and forgets what it closes.
///
/// Each list is a JSON-lines file; a successful close appends the target's `"<kind>_id":"<id>"`
/// fragment to `closed`, so later lists drop it and everything inside it (containers emptied
/// by a close are not removed implicitly). Closes fail when the ID contains `gone` (Herdr's
/// not-found error, as when a target vanishes between the executor's read and its close) or
/// `broken` (any other error). While a `hold` file exists, `workspace list` blocks, keeping
/// the executor alive mid-run.
struct FakeHerdr {
    directory: TempDir,
    binary: PathBuf,
}

const SCRIPT: &str = r#"#!/bin/sh
echo "$*" >> "$0.log"
directory=$(dirname "$0")
list() {
  printf '{"result":{"%s":[%s]}}\n' "$1" \
    "$(grep -v -F -f "$directory/closed" "$directory/$1.jsonl" | paste -sd, -)"
}
case "$1 $2" in
  "workspace list")
    waited=0
    while [ -f "$directory/hold" ] && [ "$waited" -lt 200 ]; do
      sleep 0.05
      waited=$((waited + 1))
    done
    list workspaces
    ;;
  "tab list") list tabs ;;
  "pane list") list panes ;;
  "pane close"|"tab close"|"workspace close")
    case "$3" in
      *gone*)
        echo "{\"id\":\"cli:$1:close\",\"error\":{\"code\":\"$1_not_found\",\"message\":\"$1 $3 not found\"}}" >&2
        exit 1
        ;;
      *broken*)
        echo '{"id":"cli:pane:close","error":{"code":"confirmation_required","message":"closing this pane would close a worktree group"}}' >&2
        exit 1
        ;;
      *)
        echo "\"$1_id\":\"$3\"" >> "$directory/closed"
        echo '{"id":"cli","result":{"type":"ok"}}'
        ;;
    esac
    ;;
  "notification show") echo '{"id":"cli","result":{"type":"ok"}}' ;;
  *)
    echo "unsupported: $*" >&2
    exit 2
    ;;
esac
"#;

impl FakeHerdr {
    fn new(workspaces: Vec<Value>, tabs: Vec<Value>, panes: Vec<Value>) -> Self {
        let directory = tempdir().unwrap();
        let binary = directory.path().join("herdr");
        fs::write(&binary, SCRIPT).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let write = |name: &str, items: Vec<Value>| {
            let lines = items
                .iter()
                .map(|item| format!("{item}\n"))
                .collect::<String>();
            fs::write(directory.path().join(name), lines).unwrap();
        };
        write("workspaces.jsonl", workspaces);
        write("tabs.jsonl", tabs);
        write("panes.jsonl", panes);
        // BSD grep treats an empty pattern file as matching every line; start with a sentinel.
        fs::write(directory.path().join("closed"), "\"none_id\":\"none\"\n").unwrap();
        Self { directory, binary }
    }

    /// `main` (w1) holds tabs w1:t1 (w1:p1, w1:p2) and w1:t2 (w1:p3); `api` (w2) holds w2:p1.
    fn standard() -> Self {
        Self::new(
            vec![workspace("w1", "main"), workspace("w2", "api")],
            vec![tab("w1:t1"), tab("w1:t2"), tab("w2:t1")],
            vec![
                pane("w1:p1", "w1:t1"),
                pane("w1:p2", "w1:t1"),
                pane("w1:p3", "w1:t2"),
                pane("w2:p1", "w2:t1"),
                pane("w1:pgone", "w1:t2"),
                pane("w1:pbroken", "w1:t2"),
            ],
        )
    }

    fn client(&self) -> Herdr {
        Herdr::new(&self.binary)
    }

    fn log(&self) -> String {
        fs::read_to_string(self.binary.with_extension("log")).unwrap_or_default()
    }

    fn hold(&self) {
        fs::write(self.directory.path().join("hold"), "").unwrap();
    }

    fn release(&self) {
        fs::remove_file(self.directory.path().join("hold")).unwrap();
    }
}

fn workspace(id: &str, label: &str) -> Value {
    json!({ "workspace_id": id, "label": label })
}

fn worktree(id: &str, label: &str, is_linked_worktree: bool) -> Value {
    json!({
        "workspace_id": id,
        "label": label,
        "worktree": { "repo_key": "repo", "is_linked_worktree": is_linked_worktree },
    })
}

fn tab(id: &str) -> Value {
    json!({ "tab_id": id, "workspace_id": id.split(':').next().unwrap(), "label": id })
}

fn pane(id: &str, tab_id: &str) -> Value {
    json!({
        "pane_id": id,
        "tab_id": tab_id,
        "workspace_id": tab_id.split(':').next().unwrap(),
    })
}

fn plan(scope: KillScope, targets: &[(&str, &str, &[&str])]) -> KillPlan {
    KillPlan {
        scope,
        targets: targets
            .iter()
            .map(|(id, label, pane_ids)| KillTarget {
                id: id.to_string(),
                label: label.to_string(),
                pane_ids: pane_ids.iter().map(|pane_id| pane_id.to_string()).collect(),
            })
            .collect(),
    }
}

#[test]
fn tests_that_each_scope_closes_its_targets_with_its_own_command() {
    let fake = FakeHerdr::standard();
    let herdr = fake.client();

    let panes = kill_ops::execute(
        &herdr,
        &plan(KillScope::Panes, &[("w1:p2", "two", &["w1:p2"])]),
    );
    let tabs = kill_ops::execute(
        &herdr,
        &plan(KillScope::Tabs, &[("w2:t1", "api", &["w2:p1"])]),
    );
    let workspaces = kill_ops::execute(
        &herdr,
        &plan(KillScope::Workspaces, &[("w2", "api", &["w2:p1"])]),
    );

    assert_eq!(panes.unwrap().message(), "Killed pane “two”");
    assert_eq!(tabs.unwrap().message(), "Killed tab “api”");
    assert_eq!(workspaces.unwrap().message(), "Killed workspace “api”");
    let log = fake.log();
    assert!(log.contains("pane close w1:p2\n"));
    assert!(log.contains("tab close w2:t1\n"));
    assert!(log.contains("workspace close w2\n"));
}

#[test]
fn tests_that_several_targets_are_reported_by_count() {
    let fake = FakeHerdr::standard();

    let report = kill_ops::execute(
        &fake.client(),
        &plan(
            KillScope::Panes,
            &[("w1:p1", "one", &["w1:p1"]), ("w2:p1", "api", &["w2:p1"])],
        ),
    );

    assert_eq!(report.unwrap().message(), "Killed 2 panes");
    let log = fake.log();
    assert!(log.find("pane close w1:p1").unwrap() < log.find("pane close w2:p1").unwrap());
}

#[test]
fn tests_that_targets_already_gone_are_counted_not_failed() {
    let fake = FakeHerdr::standard();

    let report = kill_ops::execute(
        &fake.client(),
        &plan(
            KillScope::Panes,
            &[
                ("w1:p1", "one", &["w1:p1"]),
                ("w1:p8", "closed earlier", &["w1:p8"]),
                ("w1:pgone", "closing now", &["w1:pgone"]),
            ],
        ),
    );

    assert_eq!(
        report.unwrap().message(),
        "Killed pane “one” · 2 panes already gone"
    );
    assert!(!fake.log().contains("pane close w1:p8"));
}

#[test]
fn tests_that_tabs_that_gained_panes_after_review_are_left_alone() {
    let fake = FakeHerdr::standard();

    let report = kill_ops::execute(
        &fake.client(),
        &plan(
            KillScope::Tabs,
            &[
                ("w1:t1", "main", &["w1:p1"]),
                ("w2:t1", "api", &["w2:p1", "w2:p7"]),
            ],
        ),
    );

    assert_eq!(
        report.unwrap().message(),
        "Killed tab “api” · skipped “main”: it changed after review"
    );
    assert!(!fake.log().contains("tab close w1:t1"));
}

#[test]
fn tests_that_a_failed_close_does_not_stop_the_rest() {
    let fake = FakeHerdr::standard();

    let report = kill_ops::execute(
        &fake.client(),
        &plan(
            KillScope::Panes,
            &[
                ("w1:pbroken", "stuck", &["w1:pbroken"]),
                ("w1:p1", "one", &["w1:p1"]),
            ],
        ),
    );

    assert_eq!(
        report.unwrap().message(),
        "Killed pane “one” · “stuck” failed: closing this pane would close a worktree group"
    );
    assert!(fake.log().contains("pane close w1:p1"));
}

#[test]
fn tests_that_linked_worktrees_close_before_their_root() {
    let fake = FakeHerdr::new(
        vec![
            worktree("w1", "repo", false),
            worktree("w2", "feature", true),
        ],
        vec![tab("w1:t1"), tab("w2:t1")],
        vec![pane("w1:p1", "w1:t1"), pane("w2:p1", "w2:t1")],
    );

    let report = kill_ops::execute(
        &fake.client(),
        &plan(
            KillScope::Workspaces,
            &[("w1", "repo", &["w1:p1"]), ("w2", "feature", &["w2:p1"])],
        ),
    );

    assert_eq!(report.unwrap().message(), "Killed 2 workspaces");
    let log = fake.log();
    assert!(log.find("workspace close w2").unwrap() < log.find("workspace close w1").unwrap());
}

/// `repo` (w1) is a worktree root and `feature` its linked worktree; each has one tab.
/// The linked tab also holds `<linked>:p9`, a pane nobody reviewed.
fn worktree_group(linked_id: &str) -> FakeHerdr {
    let linked_tab = format!("{linked_id}:t1");
    FakeHerdr::new(
        vec![
            worktree("w1", "repo", false),
            worktree(linked_id, "feature", true),
        ],
        vec![tab("w1:t1"), tab(&linked_tab)],
        vec![
            pane("w1:p1", "w1:t1"),
            pane(&format!("{linked_id}:p1"), &linked_tab),
            pane(&format!("{linked_id}:p9"), &linked_tab),
        ],
    )
}

#[test]
fn tests_that_a_root_whose_linked_worktree_survives_is_never_closed() {
    let fake = worktree_group("w2");

    let report = kill_ops::execute(
        &fake.client(),
        &plan(KillScope::Workspaces, &[("w1", "repo", &["w1:p1"])]),
    );

    assert_eq!(
        report.unwrap().message(),
        "Nothing killed · skipped “repo”: it would also close linked worktree “feature”"
    );
    assert!(!fake.log().contains("close"));
}

#[test]
fn tests_that_a_root_tab_stays_open_while_its_skipped_linked_tab_lives() {
    let fake = worktree_group("w2");

    let report = kill_ops::execute(
        &fake.client(),
        &plan(
            KillScope::Tabs,
            &[
                ("w1:t1", "repo", &["w1:p1"]),
                ("w2:t1", "feature", &["w2:p1"]),
            ],
        ),
    );

    assert_eq!(
        report.unwrap().message(),
        "Nothing killed · skipped “feature”: it changed after review · \
         skipped “repo”: it would also close linked worktree “feature”"
    );
    assert!(!fake.log().contains("tab close"));
}

#[test]
fn tests_that_a_root_pane_stays_open_while_its_linked_workspace_lives() {
    let fake = worktree_group("w2");

    let report = kill_ops::execute(
        &fake.client(),
        &plan(
            KillScope::Panes,
            &[
                ("w1:p1", "root shell", &["w1:p1"]),
                ("w2:p1", "linked shell", &["w2:p1"]),
            ],
        ),
    );

    assert_eq!(
        report.unwrap().message(),
        "Killed pane “linked shell” · \
         skipped “root shell”: it would also close linked worktree “feature”"
    );
    let log = fake.log();
    assert!(log.contains("pane close w2:p1\n"));
    assert!(!log.contains("pane close w1:p1"));
}

#[test]
fn tests_that_a_failed_linked_close_keeps_its_root_open() {
    let fake = worktree_group("w2broken");

    let report = kill_ops::execute(
        &fake.client(),
        &plan(
            KillScope::Workspaces,
            &[
                ("w1", "repo", &["w1:p1"]),
                ("w2broken", "feature", &["w2broken:p1", "w2broken:p9"]),
            ],
        ),
    );

    assert_eq!(
        report.unwrap().message(),
        "Nothing killed · “feature” failed: closing this pane would close a worktree group · \
         skipped “repo”: it would also close linked worktree “feature”"
    );
    assert!(!fake.log().contains("workspace close w1\n"));
}

#[test]
fn tests_that_the_executor_outlives_its_caller_in_a_session_of_its_own() {
    let fake = FakeHerdr::standard();
    fake.hold();
    let ferry = Path::new(env!("CARGO_BIN_EXE_herdr-ferry"));

    let mut executor = kill_ops::spawn_executor(
        ferry,
        &fake.client(),
        &plan(KillScope::Panes, &[("w1:p2", "two", &["w1:p2"])]),
    )
    .unwrap();

    let pid = executor.id() as libc::pid_t;
    // SAFETY: getsid only reads the session ID of a live process.
    let (executor_session, own_session) = unsafe { (libc::getsid(pid), libc::getsid(0)) };
    assert_eq!(executor_session, pid);
    assert_ne!(executor_session, own_session);

    fake.release();
    assert!(executor.wait().unwrap().success());
    let log = fake.log();
    assert!(log.contains("pane close w1:p2\n"));
    assert!(log.contains("notification show Ferry --body Killed pane “two”"));
}
