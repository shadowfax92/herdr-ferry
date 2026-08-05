use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::layout::{LayoutSnapshot, SplitDirection};
use crate::PLUGIN_ID;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub number: usize,
    #[serde(default)]
    pub tab_count: usize,
    #[serde(default)]
    pub pane_count: usize,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct TabInfo {
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub number: usize,
    #[serde(default)]
    pub pane_count: usize,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: String,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Topology {
    pub workspaces: Vec<WorkspaceInfo>,
    pub tabs: Vec<TabInfo>,
    pub panes: Vec<PaneInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedPane {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct InvocationContext {
    pub focused_pane_id: Option<String>,
}

impl InvocationContext {
    pub fn parse(source: &str) -> Result<Self> {
        serde_json::from_str(source).context("invalid HERDR_PLUGIN_CONTEXT_JSON")
    }

    pub fn source_pane_id(&self) -> Result<&str> {
        let pane_id = self
            .focused_pane_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
            .context("plugin context has no valid focused pane")?;
        Ok(pane_id)
    }
}

#[derive(Clone, Debug)]
pub struct Herdr {
    binary: PathBuf,
}

impl Herdr {
    pub fn from_environment() -> Self {
        Self::new(runtime_binary(std::env::var_os("HERDR_BIN_PATH")))
    }

    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    pub fn topology(&self) -> Result<Topology> {
        let workspaces: Envelope<WorkspaceListResult> = self.json(["workspace", "list"])?;
        let tabs: Envelope<TabListResult> = self.json(["tab", "list"])?;
        let panes: Envelope<PaneListResult> = self.json(["pane", "list"])?;
        Ok(Topology {
            workspaces: workspaces.result.workspaces,
            tabs: tabs.result.tabs,
            panes: panes.result.panes,
        })
    }

    pub fn pane(&self, pane_id: &str) -> Result<PaneInfo> {
        let response: Envelope<PaneResult> = self.json(["pane", "get", pane_id])?;
        Ok(response.result.pane)
    }

    pub fn layout(&self, pane_id: &str) -> Result<LayoutSnapshot> {
        let response: Envelope<LayoutResult> = self.json(["pane", "layout", "--pane", pane_id])?;
        Ok(response.result.layout)
    }

    pub fn launch_picker(&self, source_pane_id: &str) -> Result<()> {
        let source = format!("HERDR_FERRY_SOURCE_PANE_ID={source_pane_id}");
        self.output([
            OsStr::new("plugin"),
            OsStr::new("pane"),
            OsStr::new("open"),
            OsStr::new("--plugin"),
            OsStr::new(PLUGIN_ID),
            OsStr::new("--entrypoint"),
            OsStr::new("picker"),
            OsStr::new("--placement"),
            OsStr::new("popup"),
            OsStr::new("--width"),
            OsStr::new("76"),
            OsStr::new("--height"),
            OsStr::new("24"),
            OsStr::new("--env"),
            OsStr::new(&source),
        ])?;
        Ok(())
    }

    pub fn move_to_tab(
        &self,
        pane_id: &str,
        tab_id: &str,
        direction: SplitDirection,
        target_pane_id: Option<&str>,
        ratio: Option<f32>,
        focus: bool,
    ) -> Result<MovedPane> {
        let mut args = vec![
            OsString::from("pane"),
            OsString::from("move"),
            OsString::from(pane_id),
            OsString::from("--tab"),
            OsString::from(tab_id),
            OsString::from("--split"),
            OsString::from(direction.as_str()),
        ];
        if let Some(target_pane_id) = target_pane_id {
            args.push(OsString::from("--target-pane"));
            args.push(OsString::from(target_pane_id));
        }
        if let Some(ratio) = ratio {
            args.push(OsString::from("--ratio"));
            args.push(OsString::from(ratio.to_string()));
        }
        args.push(OsString::from(if focus { "--focus" } else { "--no-focus" }));
        self.move_pane(args)
    }

    pub fn move_to_new_tab(
        &self,
        pane_id: &str,
        workspace_id: &str,
        label: Option<&str>,
        focus: bool,
    ) -> Result<MovedPane> {
        let mut args = vec![
            OsString::from("pane"),
            OsString::from("move"),
            OsString::from(pane_id),
            OsString::from("--new-tab"),
            OsString::from("--workspace"),
            OsString::from(workspace_id),
        ];
        push_value(&mut args, "--label", label);
        args.push(OsString::from(if focus { "--focus" } else { "--no-focus" }));
        self.move_pane(args)
    }

    pub fn move_to_new_workspace(
        &self,
        pane_id: &str,
        label: Option<&str>,
        tab_label: Option<&str>,
        focus: bool,
    ) -> Result<MovedPane> {
        let mut args = vec![
            OsString::from("pane"),
            OsString::from("move"),
            OsString::from(pane_id),
            OsString::from("--new-workspace"),
        ];
        push_value(&mut args, "--label", label);
        push_value(&mut args, "--tab-label", tab_label);
        args.push(OsString::from(if focus { "--focus" } else { "--no-focus" }));
        self.move_pane(args)
    }

    pub fn focus_workspace(&self, workspace_id: &str) -> Result<()> {
        self.output(["workspace", "focus", workspace_id])?;
        Ok(())
    }

    pub fn focus_tab(&self, tab_id: &str) -> Result<()> {
        self.output(["tab", "focus", tab_id])?;
        Ok(())
    }

    pub fn reload_config(&self) -> Result<()> {
        self.output(["server", "reload-config"])?;
        Ok(())
    }

    pub fn notify(&self, body: &str) -> Result<()> {
        let body = body.chars().take(220).collect::<String>();
        self.output([
            OsStr::new("notification"),
            OsStr::new("show"),
            OsStr::new("Ferry"),
            OsStr::new("--body"),
            OsStr::new(&body),
        ])?;
        Ok(())
    }

    fn move_pane(&self, args: Vec<OsString>) -> Result<MovedPane> {
        let response: Envelope<PaneMoveResult> = self.json(args)?;
        let result = response.result.move_result;
        if !result.changed {
            let reason = result.reason.unwrap_or_else(|| "unknown reason".into());
            bail!("Herdr declined the move: {reason}");
        }
        Ok(MovedPane {
            pane_id: result.pane.pane_id,
            tab_id: result.pane.tab_id,
            workspace_id: result.pane.workspace_id,
        })
    }

    fn json<T, I, S>(&self, args: I) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        serde_json::from_slice(&self.output(args)?).context("failed to decode Herdr response")
    }

    fn output<I, S>(&self, args: I) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.command(args)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "Herdr command failed with {}: {}",
                output.status,
                stderr.trim()
            );
        }
        Ok(output.stdout)
    }

    fn command<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = args
            .into_iter()
            .map(|value| value.as_ref().to_os_string())
            .collect::<Vec<_>>();
        Command::new(&self.binary)
            .args(args)
            .output()
            .with_context(|| format!("failed to run {}", self.binary.display()))
    }
}

pub fn launch_from_environment() -> Result<()> {
    let herdr = Herdr::from_environment();
    let result = (|| {
        let context = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
            .context("HERDR_PLUGIN_CONTEXT_JSON is not set")?;
        let context = InvocationContext::parse(&context)?;
        herdr.launch_picker(context.source_pane_id()?)
    })();
    if let Err(error) = &result {
        let _ = herdr.notify(&format!("Could not open Ferry: {error:#}"));
    }
    result
}

fn push_value(args: &mut Vec<OsString>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        args.push(OsString::from(key));
        args.push(OsString::from(value));
    }
}

fn runtime_binary(injected: Option<OsString>) -> OsString {
    match injected {
        Some(binary) if !Path::new(&binary).is_absolute() || Path::new(&binary).is_file() => binary,
        _ => OsString::from("herdr"),
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

#[derive(Deserialize)]
struct WorkspaceListResult {
    workspaces: Vec<WorkspaceInfo>,
}

#[derive(Deserialize)]
struct TabListResult {
    tabs: Vec<TabInfo>,
}

#[derive(Deserialize)]
struct PaneListResult {
    panes: Vec<PaneInfo>,
}

#[derive(Deserialize)]
struct PaneResult {
    pane: PaneInfo,
}

#[derive(Deserialize)]
struct LayoutResult {
    layout: LayoutSnapshot,
}

#[derive(Deserialize)]
struct PaneMoveResult {
    move_result: RawMoveResult,
}

#[derive(Deserialize)]
struct RawMoveResult {
    changed: bool,
    #[serde(default)]
    reason: Option<String>,
    pane: PaneInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_context_requires_a_valid_focused_pane() {
        let context = InvocationContext::parse(r#"{"focused_pane_id":"w1:p2"}"#).unwrap();

        assert_eq!(context.source_pane_id().unwrap(), "w1:p2");
        assert!(InvocationContext::parse("{}")
            .unwrap()
            .source_pane_id()
            .is_err());
        assert!(InvocationContext::parse(r#"{"focused_pane_id":"  "}"#)
            .unwrap()
            .source_pane_id()
            .is_err());
    }

    #[test]
    fn missing_injected_binary_falls_back_to_path_lookup() {
        let temporary = tempfile::tempdir().unwrap();
        let existing = temporary.path().join("herdr");
        std::fs::write(&existing, "").unwrap();

        assert_eq!(
            runtime_binary(Some(existing.clone().into_os_string())),
            existing.into_os_string()
        );
        assert_eq!(
            runtime_binary(Some(temporary.path().join("missing").into_os_string())),
            OsString::from("herdr")
        );
    }
}
