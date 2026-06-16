mod html;
mod layout;
mod svg;

use api_subway_core::{ApiMapV1, Theme};
use thiserror::Error;

pub use html::render_html;
pub use svg::render_svg;

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub title: Option<String>,
    pub theme: Theme,
    pub max_lines: usize,
    pub min_line_stations: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            title: None,
            theme: Theme::Auto,
            max_lines: 12,
            min_line_stations: 2,
        }
    }
}

impl RenderOptions {
    fn title<'a>(&'a self, map: &'a ApiMapV1) -> &'a str {
        self.title.as_deref().unwrap_or(&map.project.name)
    }
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("failed to serialize map data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("map dimensions exceed the renderer budget")]
    LayoutBudget,
    #[error("rendered artifact exceeds the renderer output budget")]
    OutputBudget,
}
