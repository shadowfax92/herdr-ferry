use anyhow::{bail, Result};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Right,
    Down,
}

impl SplitDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SnapshotPane {
    pub pane_id: String,
    pub rect: Rect,
    pub focused: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SnapshotSplit {
    pub direction: SplitDirection,
    pub ratio: f32,
    pub rect: Rect,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LayoutSnapshot {
    pub workspace_id: String,
    pub tab_id: String,
    pub zoomed: bool,
    pub area: Rect,
    pub focused_pane_id: String,
    pub panes: Vec<SnapshotPane>,
    pub splits: Vec<SnapshotSplit>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutNode {
    Pane {
        pane_id: String,
    },
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    pub fn from_snapshot(snapshot: &LayoutSnapshot) -> Result<Self> {
        let rects = snapshot
            .panes
            .iter()
            .map(|pane| pane.rect)
            .chain(snapshot.splits.iter().map(|split| split.rect))
            .collect::<Vec<_>>();
        build(snapshot, &rects, snapshot.area)
    }

    pub fn anchor(&self) -> &str {
        match self {
            Self::Pane { pane_id } => pane_id,
            Self::Split { first, .. } => first.anchor(),
        }
    }

    pub fn leaves(&self) -> Vec<&str> {
        let mut leaves = Vec::new();
        self.collect_leaves(&mut leaves);
        leaves
    }

    fn collect_leaves<'a>(&'a self, leaves: &mut Vec<&'a str>) {
        match self {
            Self::Pane { pane_id } => leaves.push(pane_id),
            Self::Split { first, second, .. } => {
                first.collect_leaves(leaves);
                second.collect_leaves(leaves);
            }
        }
    }
}

fn build(snapshot: &LayoutSnapshot, rects: &[Rect], area: Rect) -> Result<LayoutNode> {
    let Some(split) = snapshot.splits.iter().find(|split| split.rect == area) else {
        let Some(pane) = snapshot.panes.iter().find(|pane| pane.rect == area) else {
            bail!("layout area does not map to a pane or split: {area:?}");
        };
        return Ok(LayoutNode::Pane {
            pane_id: pane.pane_id.clone(),
        });
    };
    let second_rect = second_child_rect(rects, area, split.direction)?;
    let first_rect = match split.direction {
        SplitDirection::Right => Rect {
            x: area.x,
            y: area.y,
            width: second_rect.x.saturating_sub(area.x),
            height: area.height,
        },
        SplitDirection::Down => Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: second_rect.y.saturating_sub(area.y),
        },
    };
    Ok(LayoutNode::Split {
        direction: split.direction,
        ratio: split.ratio,
        first: Box::new(build(snapshot, rects, first_rect)?),
        second: Box::new(build(snapshot, rects, second_rect)?),
    })
}

fn second_child_rect(rects: &[Rect], area: Rect, direction: SplitDirection) -> Result<Rect> {
    let candidate = rects
        .iter()
        .copied()
        .filter(|rect| match direction {
            SplitDirection::Right => {
                rect.y == area.y
                    && rect.height == area.height
                    && rect.x > area.x
                    && rect.x.saturating_add(rect.width) == area.x.saturating_add(area.width)
            }
            SplitDirection::Down => {
                rect.x == area.x
                    && rect.width == area.width
                    && rect.y > area.y
                    && rect.y.saturating_add(rect.height) == area.y.saturating_add(area.height)
            }
        })
        .max_by_key(|rect| u32::from(rect.width) * u32::from(rect.height));
    candidate.ok_or_else(|| {
        anyhow::anyhow!(
            "could not find the second child of the {} split at {area:?}",
            direction.as_str()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, x: u16, width: u16) -> SnapshotPane {
        SnapshotPane {
            pane_id: id.into(),
            rect: Rect {
                x,
                y: 0,
                width,
                height: 20,
            },
            focused: id == "p1",
        }
    }

    #[test]
    fn rebuilds_nested_layout_from_reported_rectangles() {
        let snapshot = LayoutSnapshot {
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            zoomed: false,
            area: Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 20,
            },
            focused_pane_id: "p1".into(),
            panes: vec![pane("p1", 0, 25), pane("p2", 25, 25), pane("p3", 50, 50)],
            splits: vec![
                SnapshotSplit {
                    direction: SplitDirection::Right,
                    ratio: 0.5,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 20,
                    },
                },
                SnapshotSplit {
                    direction: SplitDirection::Right,
                    ratio: 0.5,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        width: 50,
                        height: 20,
                    },
                },
            ],
        };

        let root = LayoutNode::from_snapshot(&snapshot).unwrap();

        assert_eq!(root.anchor(), "p1");
        assert_eq!(root.leaves(), vec!["p1", "p2", "p3"]);
        let LayoutNode::Split { first, second, .. } = root else {
            panic!("expected root split");
        };
        assert_eq!(first.leaves(), vec!["p1", "p2"]);
        assert_eq!(second.leaves(), vec!["p3"]);
    }

    #[test]
    fn rejects_incomplete_layout_snapshots() {
        let snapshot = LayoutSnapshot {
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            zoomed: false,
            area: Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 20,
            },
            focused_pane_id: "p1".into(),
            panes: Vec::new(),
            splits: Vec::new(),
        };

        assert!(LayoutNode::from_snapshot(&snapshot).is_err());
    }
}
