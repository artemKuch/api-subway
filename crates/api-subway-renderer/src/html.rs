use std::fmt::Write;

use api_subway_core::ApiMapV1;

use crate::{
    RenderError, RenderOptions,
    svg::{escape_xml, render_svg_inner},
};

const VIEWER_JS: &str = include_str!("../viewer/dist/viewer.js");
const VIEWER_CSS: &str = include_str!("../viewer/src/viewer.css");
const MAX_EMBEDDED_JSON_BYTES: usize = 64 * 1024 * 1024;
const MAX_HTML_BYTES: usize = 128 * 1024 * 1024;

pub fn render_html(map: &ApiMapV1, options: &RenderOptions) -> Result<String, RenderError> {
    let svg = render_svg_inner(map, options, true)?;
    let map_json = safe_json(map)?;
    let title = options.title(map);
    let expected_size = svg
        .len()
        .saturating_add(map_json.len())
        .saturating_add(VIEWER_JS.len())
        .saturating_add(VIEWER_CSS.len())
        .saturating_add(28_000);
    if expected_size > MAX_HTML_BYTES {
        return Err(RenderError::OutputBudget);
    }
    let mut output = String::with_capacity(expected_size);
    write!(
        output,
        "<!doctype html><html lang=\"en\" data-theme=\"midnight\"><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src &apos;none&apos;; script-src &apos;unsafe-inline&apos;; style-src &apos;unsafe-inline&apos;; img-src data:; connect-src &apos;none&apos;; object-src &apos;none&apos;; base-uri &apos;none&apos;; form-action &apos;none&apos;\"><meta name=\"referrer\" content=\"no-referrer\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"color-scheme\" content=\"dark light\"><title>{} · api-subway</title><style>{}</style></head><body>",
        escape_xml(title), VIEWER_CSS
    )
    .expect("writing to a string cannot fail");
    output.push_str(
        "<div class=\"app-shell\"><header class=\"topbar\"><a class=\"product\" href=\"https://github.com/artemKuch/api-subway\" aria-label=\"api-subway project\"><span class=\"product-mark\" aria-hidden=\"true\">⌁</span><span>api-subway</span></a><div class=\"project-chip\"><span class=\"live-dot\"></span>",
    );
    write!(output, "{}</div>", escape_xml(title)).expect("writing to a string cannot fail");
    output.push_str(
        "<label class=\"search-control\"><span class=\"sr-only\">Search endpoints</span><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><circle cx=\"11\" cy=\"11\" r=\"7\"></circle><path d=\"m16 16 4 4\"></path></svg><input id=\"search\" type=\"search\" autocomplete=\"off\" aria-label=\"Search endpoints\" placeholder=\"Search endpoints…\"><kbd aria-hidden=\"true\">/</kbd></label><select id=\"method-filter\" aria-label=\"Filter by HTTP method\"><option value=\"all\">All methods</option></select><div class=\"topbar-actions\"><button id=\"open-backend\" class=\"backend-button\" type=\"button\" aria-label=\"Open virtual backend\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><ellipse cx=\"12\" cy=\"5\" rx=\"7\" ry=\"3\"></ellipse><path d=\"M5 5v6c0 1.7 3.1 3 7 3s7-1.3 7-3V5M5 11v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6\"></path></svg><span>Open backend</span></button><button id=\"reset-backend\" class=\"icon-button\" type=\"button\" aria-label=\"Reset virtual backend\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M4 7v5h5M5.6 16a8 8 0 1 0 .4-9l-2 2\"></path></svg></button><button id=\"theme-toggle\" class=\"icon-button\" type=\"button\" aria-label=\"Toggle color theme\"><span aria-hidden=\"true\">◐</span></button></div></header><main><section class=\"map-panel\"><div class=\"map-toolbar\"><div id=\"kind-filters\" class=\"filter-pills\" aria-label=\"Filter dependency lines\"><button class=\"active\" data-kind=\"all\">All lines</button><button data-kind=\"middleware\">Security</button><button data-kind=\"datastore\">Data</button><button data-kind=\"service\">Services</button><button data-kind=\"external\">External</button></div><div class=\"map-actions\"><button id=\"zoom-out\" class=\"icon-button\" type=\"button\" aria-label=\"Zoom out\">−</button><button id=\"fit-map\" class=\"compact-button\" type=\"button\">Fit map</button><button id=\"zoom-in\" class=\"icon-button\" type=\"button\" aria-label=\"Zoom in\">+</button></div></div><div id=\"map-viewport\" class=\"map-viewport\" tabindex=\"0\" aria-label=\"Interactive API map\">",
    );
    output.push_str(&svg);
    output.push_str(
        "<div class=\"map-hint\">Scroll to zoom · drag to pan · click stations to open live endpoint windows</div></div><div id=\"workspace-layer\" class=\"workspace-layer\" aria-label=\"Virtual API workspace\"></div></section></main><footer><span id=\"result-count\"></span><span class=\"backend-status\">Virtual backend</span><span><b>Solid</b> exact&nbsp;&nbsp; <b class=\"dashed-key\">Dashed</b> inferred</span><span>Local only</span></footer></div>",
    );
    write!(
        output,
        "<script id=\"api-map-data\" type=\"application/json\">{map_json}</script><script>{VIEWER_JS}</script></body></html>"
    )
    .expect("writing to a string cannot fail");
    if output.len() > MAX_HTML_BYTES {
        return Err(RenderError::OutputBudget);
    }
    Ok(output)
}

fn safe_json(map: &ApiMapV1) -> Result<String, RenderError> {
    let json = serde_json::to_string(map)?;
    let escaped_size = json.chars().try_fold(0_usize, |size, character| {
        let width = if matches!(character, '<' | '\u{2028}' | '\u{2029}') {
            6
        } else {
            character.len_utf8()
        };
        size.checked_add(width)
            .filter(|next| *next <= MAX_EMBEDDED_JSON_BYTES)
    });
    let Some(escaped_size) = escaped_size else {
        return Err(RenderError::OutputBudget);
    };
    let mut output = String::with_capacity(escaped_size);
    for character in json.chars() {
        match character {
            '<' => output.push_str("\\u003c"),
            '\u{2028}' => output.push_str("\\u2028"),
            '\u{2029}' => output.push_str("\\u2029"),
            _ => output.push(character),
        }
    }
    Ok(output)
}
