use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Axis {
    Row,
    Column,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LayoutNode {
    Pane,
    Split {
        axis: Axis,
        weights: Vec<u16>,
        children: Vec<LayoutNode>,
    },
}

#[derive(Clone, Debug)]
pub enum LayoutPreset {
    Two,
    TwoByTwo,
    ThreeByFour,
    FourByFour,
}

impl Serialize for LayoutPreset {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Two => "2",
            Self::TwoByTwo => "2x2",
            Self::ThreeByFour => "3x4",
            Self::FourByFour => "4x4",
        })
    }
}

impl<'de> Deserialize<'de> for LayoutPreset {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_cli(&value))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub layout: LayoutPreset,
    pub shell: String,
    #[serde(default)]
    pub panes: Vec<String>,
}

impl Config {
    pub fn from_json(raw: &str) -> Result<Self> {
        Ok(serde_json::from_str(raw)?)
    }
}

impl LayoutPreset {
    pub fn from_cli(value: &str) -> Self {
        match value {
            "2" | "2x1" => Self::Two,
            "2x2" => Self::TwoByTwo,
            "3x4" => Self::ThreeByFour,
            "4x4" => Self::FourByFour,
            _ => Self::TwoByTwo,
        }
    }

    pub fn pane_count(&self) -> usize {
        match self {
            Self::Two => 2,
            Self::TwoByTwo => 4,
            Self::ThreeByFour => 12,
            Self::FourByFour => 16,
        }
    }

    pub fn tree(&self) -> LayoutNode {
        match self {
            Self::Two => LayoutNode::Split {
                axis: Axis::Row,
                weights: vec![1, 1],
                children: vec![LayoutNode::Pane, LayoutNode::Pane],
            },
            Self::TwoByTwo => column_of(vec![row_of(2), row_of(2)]),
            Self::ThreeByFour => column_of(vec![row_of(3), row_of(3), row_of(3), row_of(3)]),
            Self::FourByFour => column_of(vec![row_of(4), row_of(4), row_of(4), row_of(4)]),
        }
    }
}

pub fn leaf_rects(node: &LayoutNode, area: Rect, out: &mut Vec<Rect>) {
    match node {
        LayoutNode::Pane => out.push(area),
        LayoutNode::Split { axis, weights, children } => {
            let direction = match axis {
                Axis::Row => Direction::Horizontal,
                Axis::Column => Direction::Vertical,
            };
            let total = weights.iter().map(|v| *v as u32).sum::<u32>().max(1);
            let constraints = weights
                .iter()
                .map(|w| Constraint::Ratio(*w as u32, total))
                .collect::<Vec<_>>();
            let chunks = Layout::default().direction(direction).constraints(constraints).split(area);
            for (child, rect) in children.iter().zip(chunks.iter()) {
                leaf_rects(child, *rect, out);
            }
        }
    }
}

fn row_of(panes: usize) -> LayoutNode {
    LayoutNode::Split {
        axis: Axis::Row,
        weights: vec![1; panes],
        children: (0..panes).map(|_| LayoutNode::Pane).collect(),
    }
}

fn column_of(children: Vec<LayoutNode>) -> LayoutNode {
    let weights = vec![1; children.len()];
    LayoutNode::Split {
        axis: Axis::Column,
        weights,
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_cli_to_default() {
        assert!(matches!(LayoutPreset::from_cli("nope"), LayoutPreset::TwoByTwo));
    }
}
