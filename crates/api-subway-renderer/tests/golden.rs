use api_subway_core::Theme;
use api_subway_renderer::{RenderOptions, render_html, render_svg};

mod support;

const SVG_10: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/golden/map-10.svg"
));
const SVG_40: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/golden/map-40.svg"
));
const SVG_100: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/golden/map-100.svg"
));
const HTML_10: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/golden/map-10.html"
));
const HTML_40: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/golden/map-40.html"
));
const HTML_100: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/golden/map-100.html"
));
const JSON_10: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/golden/map-10.json"
));
const JSON_40: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/golden/map-40.json"
));
const JSON_100: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/golden/map-100.json"
));

fn options() -> RenderOptions {
    RenderOptions {
        title: None,
        theme: Theme::Midnight,
        max_lines: 12,
        min_line_stations: 2,
    }
}

#[test]
fn renders_byte_stable_golden_artifacts() {
    for (count, expected_json, expected_svg, expected_html) in [
        (10, JSON_10, SVG_10, HTML_10),
        (40, JSON_40, SVG_40, HTML_40),
        (100, JSON_100, SVG_100, HTML_100),
    ] {
        let map = support::synthetic_map(count);
        assert_eq!(
            format!("{}\n", serde_json::to_string_pretty(&map).expect("JSON")),
            expected_json
        );
        assert_eq!(
            format!("{}\n", render_svg(&map, &options()).expect("SVG")),
            expected_svg
        );
        assert_eq!(
            format!("{}\n", render_html(&map, &options()).expect("HTML")),
            expected_html
        );
    }
}

#[test]
fn static_svg_uses_only_readme_safe_primitives() {
    let svg = render_svg(&support::synthetic_map(40), &options()).expect("SVG");
    assert!(!svg.contains("<script"));
    assert!(!svg.contains("foreignObject"));
    assert!(!svg.contains("http://") || svg.contains("http://www.w3.org/2000/svg"));
    assert!(!svg.contains("/Users/"));
    assert!(svg.contains("stroke-dasharray"));
    assert!(svg.contains("interchange-ring"));
}

#[test]
fn interactive_html_embeds_data_and_viewer_without_runtime_assets() {
    let html = render_html(&support::synthetic_map(10), &options()).expect("HTML");
    assert!(html.contains("id=\"api-map-data\""));
    assert!(html.contains("id=\"method-filter\""));
    assert!(html.contains("id=\"kind-filters\""));
    assert!(html.contains("id=\"open-backend\""));
    assert!(html.contains("id=\"workspace-layer\""));
    assert!(html.contains("Virtual backend"));
    assert!(html.contains("Editable JSON store"));
    assert!(html.contains("http-equiv=\"Content-Security-Policy\""));
    assert!(html.contains("connect-src &apos;none&apos;"));
    assert!(html.contains("name=\"referrer\" content=\"no-referrer\""));
    assert!(!html.contains("Simulate all"));
    assert!(!html.contains("<link"));
    assert!(!html.contains("<script src="));
}
