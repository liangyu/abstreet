use crate::reader::{Document, Relation};
use abstutil::Timer;
use geom::{HashablePt2D, Polygon, Pt2D};
use map_model::osm;
use map_model::osm::{NodeID, OsmID, RelationID, WayID};
use map_model::raw::{OriginalIntersection, OriginalRoad, RawBusRoute, RawBusStop, RawMap};
use std::collections::HashMap;

pub fn extract_route(
    rel_id: RelationID,
    rel: &Relation,
    doc: &Document,
    boundary: &Polygon,
    timer: &mut Timer,
) -> Option<RawBusRoute> {
    let full_name = rel.tags.get("name")?.clone();
    let short_name = rel
        .tags
        .get("ref")
        .cloned()
        .unwrap_or_else(|| full_name.clone());
    let is_bus = match rel.tags.get("route")?.as_ref() {
        "bus" => true,
        "light_rail" => false,
        x => {
            if x != "road" && x != "bicycle" && x != "foot" && x != "railway" {
                // TODO Handle these at some point
                println!(
                    "Skipping route {} of unknown type {}: {}",
                    full_name, x, rel_id
                );
            }
            return None;
        }
    };

    // Gather stops in order. Platforms may exist or not; match them up by name.
    let mut stops = Vec::new();
    let mut platforms = HashMap::new();
    let mut all_ways = Vec::new();
    for (role, member) in &rel.members {
        if role == "stop" {
            if let OsmID::Node(n) = member {
                let node = &doc.nodes[n];
                stops.push(RawBusStop {
                    name: node
                        .tags
                        .get("name")
                        .cloned()
                        .unwrap_or_else(|| format!("stop #{}", stops.len() + 1)),
                    vehicle_pos: (OriginalIntersection { osm_node_id: *n }, node.pt),
                    matched_road: None,
                    ped_pos: None,
                });
            }
        } else if role == "platform" {
            let (platform_name, pt) = match member {
                OsmID::Node(n) => {
                    let node = &doc.nodes[n];
                    (
                        node.tags
                            .get("name")
                            .cloned()
                            .unwrap_or_else(|| format!("stop #{}", platforms.len() + 1)),
                        node.pt,
                    )
                }
                OsmID::Way(w) => {
                    let way = &doc.ways[w];
                    (
                        way.tags
                            .get("name")
                            .cloned()
                            .unwrap_or_else(|| format!("stop #{}", platforms.len() + 1)),
                        Pt2D::center(&way.pts),
                    )
                }
                _ => continue,
            };
            platforms.insert(platform_name, pt);
        } else if let OsmID::Way(w) = member {
            all_ways.push(*w);
        }
    }
    for stop in &mut stops {
        if let Some(pt) = platforms.remove(&stop.name) {
            stop.ped_pos = Some(pt);
        }
    }

    let all_pts: Vec<(OriginalIntersection, WayID, Pt2D)> = match glue_route(all_ways, doc) {
        Ok(nodes) => nodes
            .into_iter()
            .map(|(osm_node_id, osm_way_id)| {
                (
                    OriginalIntersection { osm_node_id },
                    osm_way_id,
                    doc.nodes[&osm_node_id].pt,
                )
            })
            .collect(),
        Err(err) => {
            timer.error(format!(
                "Skipping route {} ({}): {}",
                rel_id, full_name, err
            ));
            return None;
        }
    };

    // Remove stops that're out of bounds. Once we find the first in-bound point, keep all in-bound
    // stops and halt as soon as we go out of bounds again. If a route happens to dip in and out of
    // the boundary, we don't want to leave gaps.
    let mut keep_stops = Vec::new();
    let orig_num = stops.len();
    for stop in stops {
        if boundary.contains_pt(stop.vehicle_pos.1) {
            keep_stops.push(stop);
        } else {
            if !keep_stops.is_empty() {
                // That's the end of them
                break;
            }
        }
    }
    println!(
        "Kept {} / {} contiguous stops from route {}",
        keep_stops.len(),
        orig_num,
        rel_id
    );

    if keep_stops.len() < 2 {
        // Routes with only 1 stop are pretty much useless, and it makes border matching quite
        // confusing.
        return None;
    }

    Some(RawBusRoute {
        full_name,
        short_name,
        is_bus,
        osm_rel_id: rel_id,
        gtfs_trip_marker: rel.tags.get("gtfs:trip_marker").cloned(),
        stops: keep_stops,
        border_start: None,
        border_end: None,
        all_pts,
    })
}

// Figure out the actual order of nodes in the route. We assume the ways are at least listed in
// order. Match them up by endpoints. There are gaps sometimes, though!
fn glue_route(all_ways: Vec<WayID>, doc: &Document) -> Result<Vec<(NodeID, WayID)>, String> {
    if all_ways.len() == 1 {
        return Err(format!("route only has one way: {}", all_ways[0]));
    }
    let mut nodes = Vec::new();
    let mut extra = Vec::new();
    for pair in all_ways.windows(2) {
        let way1 = &doc.ways[&pair[0]];
        let way2 = &doc.ways[&pair[1]];
        let (nodes1, nodes2) = if way1.nodes[0] == way2.nodes[0] {
            (
                way1.nodes.iter().rev().cloned().collect(),
                way2.nodes.clone(),
            )
        } else if way1.nodes[0] == *way2.nodes.last().unwrap() {
            (
                way1.nodes.iter().rev().cloned().collect(),
                way2.nodes.iter().rev().cloned().collect(),
            )
        } else if *way1.nodes.last().unwrap() == way2.nodes[0] {
            (way1.nodes.clone(), way2.nodes.clone())
        } else if *way1.nodes.last().unwrap() == *way2.nodes.last().unwrap() {
            (
                way1.nodes.clone(),
                way2.nodes.iter().rev().cloned().collect(),
            )
        } else {
            return Err(format!("gap between {} and {}", pair[0], pair[1]));
        };
        let mut nodes1 = nodes1.into_iter().map(|n| (n, pair[0])).collect::<Vec<_>>();
        let nodes2 = nodes2.into_iter().map(|n| (n, pair[0])).collect::<Vec<_>>();
        // Keep the way before the intersection, so bus stops at intersections match to the road
        // before.
        if let Some((n, _)) = nodes.last() {
            if *n != nodes1[0].0 {
                return Err(format!(
                    "{} and {} match up, but last piece was {}",
                    pair[0], pair[1], n
                ));
            }
            nodes1.remove(0);
        }
        nodes.extend(nodes1);
        extra = nodes2;
    }
    // And the last lil bit
    if nodes.is_empty() {
        return Err(format!("empty? ways: {:?}", all_ways));
    }
    assert_eq!(nodes.last().unwrap().0, extra[0].0);
    extra.remove(0);
    nodes.extend(extra);
    Ok(nodes)
}

pub fn snap_bus_stops(
    mut route: RawBusRoute,
    raw: &mut RawMap,
    pt_to_road: &HashMap<HashablePt2D, OriginalRoad>,
    timer: &mut Timer,
) -> Result<RawBusRoute, String> {
    // For every stop, figure out what road segment and direction it matches up to.
    for stop in &mut route.stops {
        let idx_in_route = route
            .all_pts
            .iter()
            .position(|(node, _, _)| stop.vehicle_pos.0 == *node)
            .unwrap();
        let road = if raw.intersections.contains_key(&stop.vehicle_pos.0) {
            // The stop is at an intersection. Which of the two connected roads do we use? We want
            // the one just before the intersection.
            let osm_way_id = route.all_pts[idx_in_route].1;
            if idx_in_route == 0 {
                return Err(format!(
                    "Route begins with a stop that's also an intersection: {}",
                    stop.vehicle_pos.0.osm_node_id
                ));
            }
            let (prev_node, prev_way, prev_pt) = route.all_pts[idx_in_route - 1];
            assert_eq!(osm_way_id, prev_way);
            if raw.intersections.contains_key(&prev_node) {
                let id1 = OriginalRoad {
                    osm_way_id,
                    i1: OriginalIntersection {
                        osm_node_id: stop.vehicle_pos.0.osm_node_id,
                    },
                    i2: prev_node,
                };
                let id2 = OriginalRoad {
                    osm_way_id,
                    i2: OriginalIntersection {
                        osm_node_id: stop.vehicle_pos.0.osm_node_id,
                    },
                    i1: prev_node,
                };
                if raw.roads.contains_key(&id1) {
                    id1
                } else if raw.roads.contains_key(&id2) {
                    id2
                } else {
                    unreachable!()
                }
            } else {
                pt_to_road[&prev_pt.to_hashable()]
            }
        } else {
            // The stop is in the middle of a road, thankfully
            pt_to_road[&stop.vehicle_pos.1.to_hashable()]
        };

        // Scan backwards and forwards in the route for the nearest intersections.
        // TODO Express better with iterators
        let mut i1 = None;
        for idx in (0..idx_in_route).rev() {
            let i = route.all_pts[idx].0;
            if raw.intersections.contains_key(&i) {
                i1 = Some(i);
                break;
            }
        }
        let mut i2 = None;
        // If we're at an intersection, i2 should be the intersection, because earlier we preferred
        // a road starting before it.
        for idx in idx_in_route..route.all_pts.len() {
            let i = route.all_pts[idx].0;
            if raw.intersections.contains_key(&i) {
                i2 = Some(i);
                break;
            }
        }

        let i1 = i1.unwrap();
        let i2 = i2.unwrap();
        let fwds = if road.i1 == i1 && road.i2 == i2 {
            true
        } else if road.i1 == i2 && road.i2 == i1 {
            false
        } else {
            return Err(format!(
                "Can't figure out where {} is along route. At {}, between {:?} and {:?}. {} of {}",
                stop.vehicle_pos.0.osm_node_id,
                road,
                i1,
                i2,
                idx_in_route,
                route.all_pts.len()
            ));
        };

        stop.matched_road = Some((road, fwds));
        if false {
            println!(
                "{} matched to {}, fwds={}",
                stop.vehicle_pos.0.osm_node_id, road, fwds
            );
        }

        // If this road is missing a sidewalk (likely because it's a motorway), add one.
        // https://www.openstreetmap.org/way/325148569 is a motivating example. When we understand
        // bus platforms properly, won't need this hack.
        let tags = &mut raw.roads.get_mut(&road).unwrap().osm_tags;
        if tags.is(osm::INFERRED_SIDEWALKS, "true") {
            let current = tags.get(osm::SIDEWALK).unwrap();
            if current == "none" {
                tags.insert(osm::SIDEWALK, if fwds { "right" } else { "left" });
            } else if current == "right" && !fwds {
                tags.insert(osm::SIDEWALK, "both");
            } else if current == "left" && fwds {
                tags.insert(osm::SIDEWALK, "both");
            } else {
                continue;
            }
            timer.note(format!(
                "Inferring a sidewalk on {} for bus stop {}",
                road, stop.vehicle_pos.0.osm_node_id
            ));
        }
    }
    Ok(route)
}
