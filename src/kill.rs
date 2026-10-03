//! What a Ferry kill is: which kind of container it closes.
//!
//! The popup (`app`) decides *what* to kill; this module owns the vocabulary both the popup
//! and the detached executor share, so neither has to re-derive what a selection means.

/// The kind of Herdr container a kill closes. Every target in one kill has the same scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillScope {
    Panes,
    Tabs,
    Workspaces,
}

impl KillScope {
    /// Singular noun for headings and reports, e.g. "pane".
    pub fn noun(self) -> &'static str {
        match self {
            Self::Panes => "pane",
            Self::Tabs => "tab",
            Self::Workspaces => "workspace",
        }
    }
}
