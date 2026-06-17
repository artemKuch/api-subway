use std::{collections::BTreeMap, fmt::Write};

use api_subway_core::{ApiMapV1, Confidence, DependencyKind, Theme};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    RenderError, RenderOptions,
    layout::{Layout, LinePoint, StationSide},
};

pub fn render_svg(map: &ApiMapV1, options: &RenderOptions) -> Result<String, RenderError> {
    render_svg_inner(map, options, false)
}

pub(crate) fn render_svg_inner(
    map: &ApiMapV1,
    options: &RenderOptions,
    interactive: bool,
) -> Result<String, RenderError> {
    const MAX_SVG_BYTES: usize = 64 * 1024 * 1024;
    let layout = Layout::build(
        map,
        if interactive {
            usize::MAX
        } else {
            options.max_lines
        },
        options.min_line_stations,
        interactive,
    )?;
    let title = options.title(map);
    let mut output = String::with_capacity(32_768);
    if !interactive {
        output.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    }
    write!(
        output,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"api-map theme-{}\" role=\"img\" aria-labelledby=\"map-title map-description\" viewBox=\"0 0 {:.0} {:.0}\" data-width=\"{:.0}\" data-height=\"{:.0}\">",
        theme_name(options.theme), layout.width, layout.height, layout.width, layout.height
    )
    .expect("writing to a string cannot fail");
    write!(
        output,
        "<title id=\"map-title\">{}</title>",
        escape_xml(title)
    )
    .expect("writing to a string cannot fail");
    write!(
        output,
        "<desc id=\"map-description\">API subway map with {} endpoint stations and {} dependency lines.</desc>",
        map.endpoints.len(),
        layout.lines.len()
    )
    .expect("writing to a string cannot fail");
    output.push_str(STYLES);
    output.push_str("<rect class=\"map-background\" width=\"100%\" height=\"100%\" rx=\"18\"/>");
    output.push_str("<g id=\"map-scene\">");

    write!(
        output,
        "<g class=\"map-heading\"><text class=\"brand\" x=\"72\" y=\"60\">api-subway</text><text class=\"map-name\" x=\"72\" y=\"99\">{}</text><text class=\"map-meta\" x=\"{:.0}\" y=\"62\" text-anchor=\"end\">{} STATIONS · {} LINES</text></g>",
        escape_xml(title),
        layout.width - 72.0,
        map.endpoints.len(),
        layout.lines.len()
    )
    .expect("writing to a string cannot fail");

    for district in &layout.districts {
        write!(
            output,
            "<g class=\"district\" data-district=\"{}\"><text class=\"district-label\" x=\"{:.1}\" y=\"{:.1}\">{}</text><text class=\"district-count\" x=\"{:.1}\" y=\"{:.1}\">{:02}</text>",
            escape_attr(&district.name),
            district.label_x,
            district.y + 18.0,
            escape_xml(&district.name),
            district.count_x,
            district.y + 18.0,
            district.station_count
        )
        .expect("writing to a string cannot fail");
        if district.station_count > 0 {
            write!(
                output,
                "<path class=\"district-trunk\" d=\"M {:.1} {:.1} L {:.1} {:.1}\"/>",
                district.trunk_x,
                district.first_station_y,
                district.trunk_x,
                district.last_station_y
            )
            .expect("writing to a string cannot fail");
        } else {
            write!(
                output,
                "<text class=\"empty-district\" x=\"{:.1}\" y=\"{:.1}\">No supported routes found</text>",
                district.label_x,
                district.y + 66.0
            )
            .expect("writing to a string cannot fail");
            output.push_str("<circle class=\"simulation-status-ring\" r=\"17\"/>");
        }
        output.push_str("</g>");
    }

    output.push_str("<g class=\"dependency-lines\">");
    for line in &layout.lines {
        let dependency = &map.dependencies[line.dependency_index];
        if line.points.is_empty() {
            continue;
        }
        let first_y = line.points.first().expect("checked as non-empty").y;
        let last_y = line.points.last().expect("checked as non-empty").y;
        let (rail_start, rail_end) = if line.points.len() == 1 {
            (first_y - 18.0, last_y + 18.0)
        } else {
            (first_y, last_y)
        };
        write!(
            output,
            "<path class=\"dependency-rail\" data-dependency-id=\"{}\" data-kind=\"{}\" style=\"--line-color:{}\" d=\"M {:.1} {:.1} L {:.1} {:.1}\"/>",
            escape_attr(&dependency.id),
            kind_name(dependency.kind),
            line.color,
            line.rail_x,
            rail_start,
            line.rail_x,
            rail_end
        )
        .expect("writing to a string cannot fail");
        for point in &line.points {
            write!(
                output,
                "<path class=\"dependency-line {}\" data-dependency-id=\"{}\" data-kind=\"{}\" data-confidence=\"{}\" style=\"--line-color:{}\" d=\"{}\"/>",
                confidence_class(point.confidence),
                escape_attr(&dependency.id),
                kind_name(dependency.kind),
                confidence_name(point.confidence),
                line.color,
                connector_path(*point, line.rail_x, line.fan_offset)
            )
            .expect("writing to a string cannot fail");
        }
    }
    output.push_str("</g>");
    ensure_output_budget(&output, MAX_SVG_BYTES)?;

    let endpoint_dependencies = endpoint_dependencies(map);
    let schemas = map
        .schemas
        .iter()
        .map(|schema| (schema.id.as_str(), schema))
        .collect::<BTreeMap<_, _>>();
    output.push_str("<g class=\"stations\">");
    for station in &layout.stations {
        let endpoint = &map.endpoints[station.endpoint_index];
        let relation_ids = endpoint_dependencies
            .get(endpoint.id.as_str())
            .map_or_else(String::new, |ids| ids.join(","));
        let (input_count, response_count) = contract_counts(&schemas, endpoint);
        let label = truncate_label(&endpoint.display_path, 30);
        write!(
            output,
            "<g class=\"station\" tabindex=\"{}\" role=\"button\" aria-label=\"{} {}, {} inputs and {} responses\" data-endpoint-id=\"{}\" data-method=\"{}\" data-path=\"{}\" data-dependencies=\"{}\" data-input-count=\"{}\" data-response-count=\"{}\" transform=\"translate({:.1} {:.1})\"><title>{} {} — {}</title>",
            if interactive { "0" } else { "-1" },
            escape_attr(&endpoint.method),
            escape_attr(&endpoint.path),
            input_count,
            response_count,
            escape_attr(&endpoint.id),
            escape_attr(&endpoint.method),
            escape_attr(&endpoint.path),
            escape_attr(&relation_ids),
            input_count,
            response_count,
            station.x,
            station.y,
            escape_xml(&endpoint.method),
            escape_xml(&endpoint.path),
            escape_xml(&endpoint.framework)
        )
        .expect("writing to a string cannot fail");
        if interactive {
            let hit_area_x = if station.side == StationSide::Left {
                -360.0
            } else {
                -20.0
            };
            write!(
                output,
                "<rect class=\"station-hit-area\" x=\"{hit_area_x}\" y=\"-18\" width=\"380\" height=\"36\" rx=\"8\"/>"
            )
            .expect("writing to a string cannot fail");
        }
        if station.interchange {
            output.push_str("<circle class=\"interchange-ring\" r=\"12\"/>");
        }
        output.push_str("<circle class=\"station-dot\" r=\"6\"/>");
        let badge_width = method_badge_width(&endpoint.method);
        let badge_x = if station.side == StationSide::Left {
            -350.0
        } else {
            20.0
        };
        let label_x = if station.side == StationSide::Left {
            badge_x + badge_width + 10.0
        } else {
            30.0 + badge_width
        };
        write!(
            output,
            "<rect class=\"method-badge method-{}\" x=\"{badge_x}\" y=\"-13\" width=\"{badge_width}\" height=\"26\" rx=\"6\"/><text class=\"method-text\" x=\"{}\" y=\"4\">{}</text><text class=\"endpoint-label\" x=\"{label_x}\" y=\"5\">{}</text>",
            escape_attr(&endpoint.method.to_ascii_lowercase()),
            badge_x + badge_width / 2.0,
            escape_xml(&endpoint.method),
            escape_xml(&label)
        )
        .expect("writing to a string cannot fail");
        if interactive && endpoint.contract.is_some() {
            write!(
                output,
                "<text class=\"contract-indicator\" x=\"{badge_x}\" y=\"28\">IN {input_count} · OUT {response_count}</text>"
            )
            .expect("writing to a string cannot fail");
        }
        output.push_str("</g>");
        ensure_output_budget(&output, MAX_SVG_BYTES)?;
    }
    output.push_str("</g>");

    output.push_str("<g class=\"legend\">");
    for (index, line) in layout.lines.iter().enumerate() {
        let dependency = &map.dependencies[line.dependency_index];
        let column = index % 4;
        let row = index / 4;
        let x = CANVAS_LEGEND_X + column as f32 * ((layout.width - 2.0 * CANVAS_LEGEND_X) / 4.0);
        let y = layout.legend_y + row as f32 * 40.0;
        write!(
            output,
            "<g class=\"legend-item\" data-dependency-id=\"{}\" data-kind=\"{}\"><path style=\"--line-color:{}\" d=\"M {:.1} {:.1} L {:.1} {:.1}\"/><circle style=\"--line-color:{}\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"4\"/><text x=\"{:.1}\" y=\"{:.1}\">{}</text><text class=\"legend-kind\" x=\"{:.1}\" y=\"{:.1}\">{}</text></g>",
            escape_attr(&dependency.id),
            kind_name(dependency.kind),
            line.color,
            x,
            y,
            x + 28.0,
            y,
            line.color,
            x + 14.0,
            y,
            x + 40.0,
            y + 4.0,
            escape_xml(&truncate_label(&dependency.name, 18)),
            x + 40.0,
            y + 18.0,
            kind_name(dependency.kind)
        )
        .expect("writing to a string cannot fail");
    }
    if layout.hidden_line_count > 0 {
        write!(
            output,
            "<text class=\"hidden-lines\" x=\"{:.1}\" y=\"{:.1}\">+{} less-connected lines available in HTML</text>",
            CANVAS_LEGEND_X,
            layout.height - 34.0,
            layout.hidden_line_count
        )
        .expect("writing to a string cannot fail");
    }
    output.push_str("</g></g></svg>");
    ensure_output_budget(&output, MAX_SVG_BYTES)?;

    Ok(output)
}

const CANVAS_LEGEND_X: f32 = 72.0;

fn endpoint_dependencies(map: &ApiMapV1) -> BTreeMap<&str, Vec<&str>> {
    let mut output = BTreeMap::<&str, Vec<&str>>::new();
    for relation in &map.relations {
        output
            .entry(relation.endpoint_id.as_str())
            .or_default()
            .push(relation.dependency_id.as_str());
    }
    for ids in output.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }
    output
}

fn connector_path(point: LinePoint, rail_x: f32, fan_offset: f32) -> String {
    let direction = point.side.direction();
    let fan_y = point.y + fan_offset;
    format!(
        "M {:.1} {:.1} L {:.1} {:.1} L {:.1} {:.1} L {:.1} {:.1}",
        point.x,
        point.y,
        point.x + direction * 14.0,
        fan_y,
        rail_x - direction * 10.0,
        fan_y,
        rail_x,
        point.y
    )
}

fn method_badge_width(method: &str) -> f32 {
    26.0 + method.len() as f32 * 7.0
}

fn contract_counts(
    schemas: &BTreeMap<&str, &api_subway_core::ApiSchema>,
    endpoint: &api_subway_core::Endpoint,
) -> (usize, usize) {
    let Some(contract) = &endpoint.contract else {
        return (0, 0);
    };
    let body_fields = contract.request.bodies.iter().map(|body| {
        schemas
            .get(body.schema_id.as_str())
            .map_or(1, |schema| schema.properties.len().max(1))
    });
    (
        contract.request.parameters.len() + body_fields.sum::<usize>(),
        contract.responses.len(),
    )
}

fn truncate_label(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_owned();
    }
    let mut output = String::new();
    let mut width = 0;
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width + 1 > max_width {
            break;
        }
        output.push(character);
        width += character_width;
    }
    output.push('…');
    output
}

fn theme_name(theme: Theme) -> &'static str {
    match theme {
        Theme::Auto => "auto",
        Theme::Paper => "paper",
        Theme::Midnight => "midnight",
    }
}

fn kind_name(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::Middleware => "middleware",
        DependencyKind::Service => "service",
        DependencyKind::Datastore => "datastore",
        DependencyKind::External => "external",
    }
}

fn confidence_name(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Exact => "exact",
        Confidence::Inferred => "inferred",
    }
}

fn confidence_class(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Exact => "exact",
        Confidence::Inferred => "inferred",
    }
}

pub(crate) fn escape_xml(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            character if is_xml_character(character) => output.push(character),
            _ => output.push('\u{fffd}'),
        }
    }
    output
}

fn is_xml_character(character: char) -> bool {
    matches!(character, '\u{9}' | '\u{a}' | '\u{d}')
        || matches!(character as u32, 0x20..=0xd7ff | 0xe000..=0xfffd | 0x1_0000..=0x0010_ffff)
            && !matches!(character, '\u{fffe}' | '\u{ffff}')
}

fn ensure_output_budget(output: &str, maximum: usize) -> Result<(), RenderError> {
    if output.len() > maximum {
        Err(RenderError::OutputBudget)
    } else {
        Ok(())
    }
}

fn escape_attr(value: &str) -> String {
    escape_xml(value)
}

const STYLES: &str = r#"<style>
.api-map{--bg:#071521;--surface:#0d2130;--text:#f6f1e7;--muted:#8ea5b5;--faint:#294353;--trunk:#426070;--station:#f6f1e7;--badge:#183344;background:var(--bg);font-family:Inter,ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}.api-map.theme-paper{--bg:#f7f4ec;--surface:#fffdf7;--text:#17242c;--muted:#68777f;--faint:#d9ddd9;--trunk:#aeb9bc;--station:#17242c;--badge:#e9eeec}.api-map.theme-auto{--bg:#f7f4ec;--surface:#fffdf7;--text:#17242c;--muted:#68777f;--faint:#d9ddd9;--trunk:#aeb9bc;--station:#17242c;--badge:#e9eeec}@media(prefers-color-scheme:dark){.api-map.theme-auto{--bg:#071521;--surface:#0d2130;--text:#f6f1e7;--muted:#8ea5b5;--faint:#294353;--trunk:#426070;--station:#f6f1e7;--badge:#183344}}.map-background{fill:var(--bg)}text{fill:var(--text)}.brand{font-size:17px;font-weight:800;letter-spacing:.08em;fill:#2bd9fe}.map-name{font-size:28px;font-weight:680;letter-spacing:-.025em}.map-meta{font-size:11px;font-weight:700;letter-spacing:.12em;fill:var(--muted)}.district-label{font-size:12px;font-weight:800;letter-spacing:.16em}.district-count{font-size:10px;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;text-anchor:end;fill:var(--muted)}.district-trunk{stroke:var(--trunk);stroke-width:3;fill:none;stroke-linecap:round}.empty-district{font-size:13px;fill:var(--muted)}.dependency-rail{stroke:var(--line-color);stroke-width:6;fill:none;stroke-linecap:round;opacity:.92}.dependency-line{stroke:var(--line-color);stroke-width:4;fill:none;stroke-linecap:round;stroke-linejoin:round;opacity:.9}.dependency-line.inferred{stroke-dasharray:8 7;opacity:.7}.station{cursor:pointer}.station-hit-area{fill:none;pointer-events:all}.simulation-status-ring{display:none;fill:none;pointer-events:none}.station-dot{fill:var(--station);stroke:var(--bg);stroke-width:3}.interchange-ring{fill:var(--bg);stroke:var(--station);stroke-width:3}.method-badge{fill:var(--badge)}.method-get{fill:#123e4a}.method-post{fill:#193f35}.method-put{fill:#453b1d}.method-patch{fill:#372d57}.method-delete{fill:#4e282c}.method-options,.method-head,.method-any{fill:#263b49}.theme-paper .method-get,.theme-auto .method-get{fill:#d9f5fa}.theme-paper .method-post,.theme-auto .method-post{fill:#dbf4e8}.theme-paper .method-put,.theme-auto .method-put{fill:#f9edc9}.theme-paper .method-patch,.theme-auto .method-patch{fill:#eae2ff}.theme-paper .method-delete,.theme-auto .method-delete{fill:#ffe2df}@media(prefers-color-scheme:dark){.theme-auto .method-get{fill:#123e4a}.theme-auto .method-post{fill:#193f35}.theme-auto .method-put{fill:#453b1d}.theme-auto .method-patch{fill:#372d57}.theme-auto .method-delete{fill:#4e282c}}.method-text{fill:var(--text);font-size:10px;font-weight:800;text-anchor:middle;letter-spacing:.04em}.endpoint-label{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12px;font-weight:550}.contract-indicator{display:none;fill:#f7bd4a;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:8px;font-weight:800;letter-spacing:.08em}.station.selected .contract-indicator{display:block}.legend-item path{stroke:var(--line-color);stroke-width:5;stroke-linecap:round}.legend-item circle{fill:var(--line-color)}.legend-item text{font-size:11px;font-weight:650}.legend-item .legend-kind{font-size:8px;text-transform:uppercase;letter-spacing:.1em;fill:var(--muted)}.hidden-lines{font-size:11px;fill:var(--muted)}
</style>"#;

#[cfg(test)]
mod tests {
    use super::{escape_xml, truncate_label};

    #[test]
    fn escapes_untrusted_svg_text() {
        assert_eq!(
            escape_xml("<script a='b'>&\""),
            "&lt;script a=&apos;b&apos;&gt;&amp;&quot;"
        );
        assert_eq!(escape_xml("safe\u{0}\u{1b}text"), "safe��text");
    }

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate_label("/a/very/long/path", 10), "/a/very/l…");
    }
}
