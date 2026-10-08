//! The three Local apps share only their declared service children.

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum App {
    WebPlanner,
    MapBuilder,
    Simulator,
}

impl App {
    pub const ALL: [Self; 3] = [Self::WebPlanner, Self::MapBuilder, Self::Simulator];

    pub fn name(self) -> &'static str {
        match self {
            Self::WebPlanner => "Web planner",
            Self::MapBuilder => "Map builder",
            Self::Simulator => "Simulator",
        }
    }

    pub fn children(self) -> &'static [&'static str] {
        match self {
            Self::WebPlanner => &["routing", "search", "tiles", "frontend"],
            Self::MapBuilder => &["tiles", "frontend"],
            Self::Simulator => &["simulator"],
        }
    }

    pub fn url(self) -> Option<&'static str> {
        match self {
            Self::WebPlanner => Some("http://127.0.0.1:5173/planner.html"),
            Self::MapBuilder => Some("http://127.0.0.1:5173/"),
            Self::Simulator => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppState {
    pub status: String,
    pub message: Option<String>,
}
