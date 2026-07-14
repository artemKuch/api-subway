use std::{fs, path::PathBuf};

use api_subway_core::Theme;
use api_subway_renderer::{RenderOptions, render_html, render_svg};

#[path = "../tests/support/mod.rs"]
mod support;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures/golden");
    fs::create_dir_all(&output)?;
    let options = RenderOptions {
        title: None,
        theme: Theme::Midnight,
        max_lines: 12,
        min_line_stations: 2,
    };
    for count in [10, 40, 100] {
        let map = support::synthetic_map(count);
        fs::write(
            output.join(format!("map-{count}.json")),
            format!("{}\n", serde_json::to_string_pretty(&map)?),
        )?;
        fs::write(
            output.join(format!("map-{count}.svg")),
            format!("{}\n", render_svg(&map, &options)?),
        )?;
        fs::write(
            output.join(format!("map-{count}.html")),
            format!("{}\n", render_html(&map, &options)?),
        )?;
    }
    Ok(())
}
