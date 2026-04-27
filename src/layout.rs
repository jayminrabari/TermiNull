use anyhow::Result;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
    #[serde(default = "default_shell")]
    pub shell: String,
    #[serde(default)]
    pub panes: Vec<PaneSpec>,
    #[serde(default)]
    pub ui: UiConfig,
}

impl Config {
    pub fn from_json(raw: &str) -> Result<Self> {
        Ok(serde_json::from_str(raw)?)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UiConfig {
    pub font: Option<String>,
    pub font_size: Option<f32>,
    pub scrollback_lines: Option<usize>,
    #[serde(default)]
    pub theme: ThemeConfig,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub background: Option<String>,
    pub pane_background: Option<String>,
    pub title_background: Option<String>,
    pub title_active_background: Option<String>,
    pub border: Option<String>,
    pub border_active: Option<String>,
    pub text: Option<String>,
    pub dim_text: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PaneSpec {
    Command(String),
    Detailed {
        title: Option<String>,
        command: Option<String>,
    },
}

impl PaneSpec {
    pub fn command(&self) -> Option<&str> {
        match self {
            Self::Command(command) => non_empty(command),
            Self::Detailed { command, .. } => command.as_deref().and_then(non_empty),
        }
    }

    pub fn title(&self) -> Option<&str> {
        match self {
            Self::Command(command) => non_empty(command),
            Self::Detailed { title, command } => title
                .as_deref()
                .and_then(non_empty)
                .or_else(|| command.as_deref().and_then(non_empty)),
        }
    }
}

fn default_shell() -> String {
    "bash".to_string()
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
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
        LayoutNode::Split {
            axis,
            weights,
            children,
        } => {
            let direction = match axis {
                Axis::Row => Direction::Horizontal,
                Axis::Column => Direction::Vertical,
            };
            let total = weights.iter().map(|v| *v as u32).sum::<u32>().max(1);
            let constraints = weights
                .iter()
                .map(|w| Constraint::Ratio(*w as u32, total))
                .collect::<Vec<_>>();
            let chunks = Layout::default()
                .direction(direction)
                .constraints(constraints)
                .split(area);
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
        assert!(matches!(
            LayoutPreset::from_cli("nope"),
            LayoutPreset::TwoByTwo
        ));
    }

    #[test]
    fn parses_old_string_pane_config() {
        let config =
            Config::from_json(r#"{"layout":"2x2","shell":"bash","panes":["htop"]}"#).unwrap();
        assert_eq!(config.panes[0].command(), Some("htop"));
        assert_eq!(config.panes[0].title(), Some("htop"));
    }

    #[test]
    fn parses_titled_pane_config() {
        let config = Config::from_json(
            r#"{"layout":"2x2","panes":[{"title":"Logs","command":"tail -f app.log"}]}"#,
        )
        .unwrap();
        assert_eq!(config.shell, "bash");
        assert_eq!(config.panes[0].command(), Some("tail -f app.log"));
        assert_eq!(config.panes[0].title(), Some("Logs"));
    }

    #[test]
    fn parses_ui_config() {
        let config = Config::from_json(
            r##"{"layout":"2x2","ui":{"font":"Ubuntu Sans Mono","font_size":13,"scrollback_lines":800,"theme":{"text":"#ffffff"}}}"##,
        )
        .unwrap();
        assert_eq!(config.ui.font.as_deref(), Some("Ubuntu Sans Mono"));
        assert_eq!(config.ui.font_size, Some(13.0));
        assert_eq!(config.ui.scrollback_lines, Some(800));
        assert_eq!(config.ui.theme.text.as_deref(), Some("#ffffff"));
    }
}
