use std::collections::{BTreeMap, BTreeSet};

use api_subway_core::{ApiMapV1, Confidence, DependencyKind};

use crate::RenderError;

const MAX_ENDPOINTS: usize = 20_000;
const CANVAS_WIDTH: f32 = 1_360.0;
const LEFT_MARGIN: f32 = 74.0;
const RIGHT_MARGIN: f32 = 74.0;
const LEFT_TRUNK_X: f32 = 430.0;
const RIGHT_TRUNK_X: f32 = 930.0;
const CORRIDOR_LEFT: f32 = 515.0;
const CORRIDOR_RIGHT: f32 = 845.0;
const STATION_GAP: f32 = 44.0;
const HEADER_HEIGHT: f32 = 152.0;
const DISTRICT_HEADER: f32 = 54.0;
const DISTRICT_FOOTER: f32 = 34.0;
const DISTRICT_GAP: f32 = 62.0;

#[derive(Debug)]
pub(crate) struct Layout {
    pub width: f32,
    pub height: f32,
    pub stations: Vec<StationLayout>,
    pub districts: Vec<DistrictLayout>,
    pub lines: Vec<LineLayout>,
    pub hidden_line_count: usize,
    pub legend_y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StationSide {
    Left,
    Right,
}

impl StationSide {
    pub const fn direction(self) -> f32 {
        match self {
            Self::Left => 1.0,
            Self::Right => -1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StationLayout {
    pub endpoint_index: usize,
    pub x: f32,
    pub y: f32,
    pub side: StationSide,
    pub interchange: bool,
}

#[derive(Debug)]
pub(crate) struct DistrictLayout {
    pub name: String,
    pub label_x: f32,
    pub count_x: f32,
    pub y: f32,
    pub trunk_x: f32,
    pub first_station_y: f32,
    pub last_station_y: f32,
    pub station_count: usize,
}

#[derive(Debug)]
pub(crate) struct LineLayout {
    pub dependency_index: usize,
    pub color: &'static str,
    pub rail_x: f32,
    pub fan_offset: f32,
    pub points: Vec<LinePoint>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LinePoint {
    pub x: f32,
    pub y: f32,
    pub side: StationSide,
    pub confidence: Confidence,
}

impl Layout {
    pub fn build(
        map: &ApiMapV1,
        max_lines: usize,
        min_line_stations: usize,
        include_singletons: bool,
    ) -> Result<Self, RenderError> {
        if map.endpoints.len() > MAX_ENDPOINTS {
            return Err(RenderError::LayoutBudget);
        }

        let mut district_endpoints = BTreeMap::<String, Vec<usize>>::new();
        for (index, endpoint) in map.endpoints.iter().enumerate() {
            district_endpoints
                .entry(endpoint.district.clone())
                .or_default()
                .push(index);
        }
        if district_endpoints.is_empty() {
            district_endpoints.insert("NO ROUTES".to_owned(), Vec::new());
        }

        let mut side_y = [HEADER_HEIGHT, HEADER_HEIGHT];
        let mut stations = Vec::with_capacity(map.endpoints.len());
        let mut districts = Vec::with_capacity(district_endpoints.len());
        let mut endpoint_positions = BTreeMap::<String, (f32, f32, StationSide)>::new();
        for (district_index, (name, endpoints)) in district_endpoints.into_iter().enumerate() {
            let side = if district_index % 2 == 0 {
                StationSide::Left
            } else {
                StationSide::Right
            };
            let side_index = usize::from(side == StationSide::Right);
            let y = side_y[side_index];
            let trunk_x = if side == StationSide::Left {
                LEFT_TRUNK_X
            } else {
                RIGHT_TRUNK_X
            };
            let first_station_y = y + DISTRICT_HEADER;
            let last_station_y =
                first_station_y + endpoints.len().saturating_sub(1) as f32 * STATION_GAP;
            for (station_index, endpoint_index) in endpoints.iter().copied().enumerate() {
                let station_y = first_station_y + station_index as f32 * STATION_GAP;
                endpoint_positions.insert(
                    map.endpoints[endpoint_index].id.clone(),
                    (trunk_x, station_y, side),
                );
                stations.push(StationLayout {
                    endpoint_index,
                    x: trunk_x,
                    y: station_y,
                    side,
                    interchange: false,
                });
            }
            let district_height = DISTRICT_HEADER
                + endpoints.len().saturating_sub(1) as f32 * STATION_GAP
                + DISTRICT_FOOTER;
            side_y[side_index] += district_height + DISTRICT_GAP;
            districts.push(DistrictLayout {
                name,
                label_x: if side == StationSide::Left {
                    LEFT_MARGIN
                } else {
                    RIGHT_TRUNK_X
                },
                count_x: if side == StationSide::Left {
                    LEFT_TRUNK_X
                } else {
                    CANVAS_WIDTH - RIGHT_MARGIN
                },
                y,
                trunk_x,
                first_station_y,
                last_station_y,
                station_count: endpoints.len(),
            });
        }
        stations.sort_by_key(|station| station.endpoint_index);

        let mut relation_counts = BTreeMap::<String, usize>::new();
        let mut relations_by_dependency = BTreeMap::<&str, Vec<&api_subway_core::Relation>>::new();
        for relation in &map.relations {
            *relation_counts
                .entry(relation.dependency_id.clone())
                .or_default() += 1;
            relations_by_dependency
                .entry(relation.dependency_id.as_str())
                .or_default()
                .push(relation);
        }
        let mut dependency_indices = map
            .dependencies
            .iter()
            .enumerate()
            .filter(|(_, dependency)| {
                let count = relation_counts.get(&dependency.id).copied().unwrap_or(0);
                dependency.pinned || include_singletons || count >= min_line_stations
            })
            .map(|(index, dependency)| {
                (
                    index,
                    dependency.pinned,
                    relation_counts.get(&dependency.id).copied().unwrap_or(0),
                    dependency.kind,
                    dependency.name.clone(),
                )
            })
            .collect::<Vec<_>>();
        dependency_indices.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| right.2.cmp(&left.2))
                .then_with(|| dependency_kind_rank(left.3).cmp(&dependency_kind_rank(right.3)))
                .then_with(|| left.4.cmp(&right.4))
        });
        let hidden_line_count = dependency_indices.len().saturating_sub(max_lines);
        dependency_indices.truncate(max_lines);

        let visible_ids = dependency_indices
            .iter()
            .map(|(index, ..)| map.dependencies[*index].id.as_str())
            .collect::<BTreeSet<_>>();
        let mut endpoint_line_counts = BTreeMap::<String, usize>::new();
        for relation in &map.relations {
            if visible_ids.contains(relation.dependency_id.as_str()) {
                *endpoint_line_counts
                    .entry(relation.endpoint_id.clone())
                    .or_default() += 1;
            }
        }
        for station in &mut stations {
            let endpoint = &map.endpoints[station.endpoint_index];
            station.interchange = endpoint_line_counts.get(&endpoint.id).copied().unwrap_or(0) > 1;
        }

        let palette = [
            "#2bd9fe", "#ff6f61", "#f7bd4a", "#a88bff", "#5ce1b6", "#ff8ccd", "#72a7ff", "#f58d44",
            "#7bdc65", "#dd79ff", "#38c7ae", "#e5d352",
        ];
        let line_count = dependency_indices.len();
        let rail_gap = if line_count <= 1 {
            0.0
        } else {
            (CORRIDOR_RIGHT - CORRIDOR_LEFT) / (line_count - 1) as f32
        };
        let mut lines = Vec::with_capacity(line_count);
        for (line_index, (dependency_index, ..)) in dependency_indices.into_iter().enumerate() {
            let dependency = &map.dependencies[dependency_index];
            let mut points = relations_by_dependency
                .get(dependency.id.as_str())
                .into_iter()
                .flat_map(|relations| relations.iter().copied())
                .filter_map(|relation| {
                    endpoint_positions
                        .get(&relation.endpoint_id)
                        .map(|(x, y, side)| LinePoint {
                            x: *x,
                            y: *y,
                            side: *side,
                            confidence: relation.confidence,
                        })
                })
                .collect::<Vec<_>>();
            points.sort_by(|left, right| {
                left.y
                    .total_cmp(&right.y)
                    .then_with(|| left.x.total_cmp(&right.x))
            });
            points.dedup_by(|left, right| {
                left.x.to_bits() == right.x.to_bits() && left.y.to_bits() == right.y.to_bits()
            });
            let centered_index = line_index as f32 - line_count.saturating_sub(1) as f32 / 2.0;
            lines.push(LineLayout {
                dependency_index,
                color: palette[line_index % palette.len()],
                rail_x: if line_count <= 1 {
                    f32::midpoint(CORRIDOR_LEFT, CORRIDOR_RIGHT)
                } else {
                    CORRIDOR_LEFT + line_index as f32 * rail_gap
                },
                fan_offset: centered_index * 2.2,
                points,
            });
        }

        let map_bottom = side_y[0].max(side_y[1]) - DISTRICT_GAP;
        let legend_y = map_bottom + 56.0;
        let legend_rows = lines.len().div_ceil(4).max(1);
        let height = legend_y + legend_rows as f32 * 42.0 + 76.0;

        Ok(Self {
            width: CANVAS_WIDTH,
            height,
            stations,
            districts,
            lines,
            hidden_line_count,
            legend_y,
        })
    }
}

fn dependency_kind_rank(kind: DependencyKind) -> u8 {
    match kind {
        DependencyKind::Middleware => 0,
        DependencyKind::Datastore => 1,
        DependencyKind::Service => 2,
        DependencyKind::External => 3,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use api_subway_core::{
        ApiMapBuilder, Confidence, Dependency, DependencyKind, Endpoint, Relation,
        canonical_endpoint_id, dependency_id, district_for_path,
    };

    use super::{CORRIDOR_LEFT, CORRIDOR_RIGHT, Layout, StationSide};

    #[test]
    fn places_large_station_sets_without_coordinate_collisions() {
        let mut builder = ApiMapBuilder::new("layout test");
        for index in 0..100 {
            let district = format!("district-{}", index % 8);
            let path = format!("/{district}/route-{index}");
            let method = if index % 2 == 0 { "GET" } else { "POST" };
            builder.add_endpoint(Endpoint {
                id: canonical_endpoint_id(method, &path),
                method: method.to_owned(),
                path: path.clone(),
                display_path: path.clone(),
                district: district_for_path(&path),
                framework: "test".to_owned(),
                operation_id: None,
                tags: Vec::new(),
                sources: Vec::new(),
                spec_only: false,
                contract: None,
            });
        }

        let layout = Layout::build(&builder.build(), 12, 2, false).expect("layout");
        let positions = layout
            .stations
            .iter()
            .map(|station| (station.x.to_bits(), station.y.to_bits()))
            .collect::<BTreeSet<_>>();
        assert_eq!(positions.len(), 100);
        assert!(
            layout
                .stations
                .iter()
                .any(|station| station.side == StationSide::Left)
        );
        assert!(
            layout
                .stations
                .iter()
                .any(|station| station.side == StationSide::Right)
        );
    }

    #[test]
    fn caps_readme_lines_and_keeps_rails_inside_the_corridor() {
        let mut builder = ApiMapBuilder::new("line budget");
        let path = "/users";
        let endpoint_id = canonical_endpoint_id("GET", path);
        builder.add_endpoint(Endpoint {
            id: endpoint_id.clone(),
            method: "GET".to_owned(),
            path: path.to_owned(),
            display_path: path.to_owned(),
            district: district_for_path(path),
            framework: "test".to_owned(),
            operation_id: None,
            tags: Vec::new(),
            sources: Vec::new(),
            spec_only: false,
            contract: None,
        });
        for index in 0..15 {
            let name = format!("Dependency {index}");
            let id = dependency_id(DependencyKind::Service, &name);
            builder.add_dependency(Dependency {
                id: id.clone(),
                name,
                kind: DependencyKind::Service,
                pinned: true,
                packages: Vec::new(),
            });
            builder.add_relation(Relation {
                endpoint_id: endpoint_id.clone(),
                dependency_id: id,
                confidence: Confidence::Exact,
                evidence: Vec::new(),
            });
        }

        let layout = Layout::build(&builder.build(), 12, 2, false).expect("layout");
        assert_eq!(layout.lines.len(), 12);
        assert_eq!(layout.hidden_line_count, 3);
        assert!(
            layout
                .lines
                .iter()
                .all(|line| (CORRIDOR_LEFT..=CORRIDOR_RIGHT).contains(&line.rail_x))
        );
    }
}
