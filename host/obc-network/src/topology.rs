//! Compile source-wide turn ports before a cell can remove any incident segment.

use crate::nav::{segment_key, CutRun, Incidence, JunctionKey, SourceKey};
use obc_formats::obcm::NAV_MAX_DEGREE;
use obc_places::routing::data::Routing;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy)]
struct Port {
    run: usize,
    vertex: usize,
    direction: usize,
}

fn identity(run: &CutRun, port: &Port) -> (i64, SourceKey, bool) {
    let neighbor = if port.direction == 0 { port.vertex + 1 } else { port.vertex - 1 };
    let source = segment_key(run.way_id, run.keys[port.vertex].source(), run.keys[neighbor].source());
    let side =
        matches!(run.keys[port.vertex], JunctionKey::Boundary(..)) && run.coords[neighbor] > run.coords[port.vertex];
    (run.way_id, source, side)
}

pub(crate) fn compile(runs: &mut [CutRun], routing: &Routing) -> Result<(), String> {
    let mut rules = HashMap::<_, Vec<_>>::new();
    for rule in &routing.restrictions {
        rules.entry(rule.via).or_default().push(rule);
    }
    let mut seen = HashSet::new();
    let mut shared = HashSet::new();
    for run in &*runs {
        for &key in &run.keys {
            if !seen.insert(key) {
                shared.insert(key);
            }
        }
    }
    drop(seen);
    let mut junctions: HashMap<JunctionKey, Vec<Port>> = HashMap::new();
    for (r, run) in runs.iter_mut().enumerate() {
        if routing.excluded_ways.contains(&run.way_id) {
            run.access = [Default::default(); 2];
        }
        for (vertex, &key) in run.keys.iter().enumerate() {
            let forced = match key {
                JunctionKey::Boundary(..) => true,
                JunctionKey::Osm(id) => routing.nodes.contains_key(&id) || rules.contains_key(&id),
            };
            if vertex > 0 && vertex + 1 < run.keys.len() && !forced && !shared.contains(&key) {
                // A cell boundary can promote an ordinary vertex to a junction. Its two
                // incidences need the same source ordering without a junction allocation.
                let forward = identity(run, &Port { run: r, vertex, direction: 0 });
                let backward = identity(run, &Port { run: r, vertex, direction: 1 });
                run.incidences[vertex] = [
                    Incidence { port: u8::from(forward > backward), forbidden: 0 },
                    Incidence { port: u8::from(backward > forward), forbidden: 0 },
                ];
                continue;
            }
            if vertex + 1 < run.keys.len() {
                junctions.entry(key).or_default().push(Port { run: r, vertex, direction: 0 });
            }
            if vertex > 0 {
                junctions.entry(key).or_default().push(Port { run: r, vertex, direction: 1 });
            }
        }
    }
    for (key, ports) in junctions {
        // The same source segment can appear twice in a duplicate input. It owns one port.
        let mut identities = BTreeMap::new();
        for port in &ports {
            identities.entry(identity(&runs[port.run], port)).or_insert(0u8);
        }
        if identities.len() > NAV_MAX_DEGREE {
            return Err(format!(
                "source junction {key:?} degree {} exceeds supported {NAV_MAX_DEGREE}",
                identities.len()
            ));
        }
        for (index, value) in identities.values_mut().enumerate() {
            *value = index as u8;
        }
        for port in &ports {
            let run = &mut runs[port.run];
            let own = identities[&identity(run, port)];
            run.incidences[port.vertex][port.direction] = Incidence { port: own, forbidden: 0 };
        }
        let JunctionKey::Osm(via) = key else { continue };
        for rule in rules.get(&via).into_iter().flatten() {
            let approaches: Vec<_> = ports.iter().copied().filter(|port| runs[port.run].way_id == rule.from).collect();
            for incoming in approaches {
                let from_port = runs[incoming.run].incidences[incoming.vertex][incoming.direction].port;
                for outgoing in &ports {
                    let run = &runs[outgoing.run];
                    let to_port = run.incidences[outgoing.vertex][outgoing.direction].port;
                    let target =
                        run.way_id == rule.to && (rule.from != rule.to || rule.u_turn == (from_port == to_port));
                    if target != rule.only {
                        runs[outgoing.run].incidences[outgoing.vertex][outgoing.direction].forbidden |= 1 << from_port;
                    }
                }
            }
        }
        let barrier = routing.nodes.get(&via).is_some_and(|access| access.allowed & 15 != 15 || access.pushing != 0);
        let restricted = ports.iter().any(|port| runs[port.run].incidences[port.vertex][port.direction].forbidden != 0);
        for port in ports {
            runs[port.run].requires_context[port.vertex] = barrier || restricted;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_places::routing::RoutableWay;

    #[test]
    fn ordinary_vertex_ports_survive_way_reversal_and_duplicate_input() {
        let mut way = RoutableWay::new(10, vec![3, 2, 1], vec![(0, 0), (100, 0), (200, 0)], 7);
        let mut single = [CutRun::from_way(&way)];
        compile(&mut single, &Routing::default()).unwrap();
        let ports = single[0].incidences[1];
        assert_ne!(ports[0].port, ports[1].port);

        way.node_ids.reverse();
        way.coords.reverse();
        let mut reverse = [CutRun::from_way(&way)];
        compile(&mut reverse, &Routing::default()).unwrap();
        assert_eq!(reverse[0].incidences[1], [ports[1], ports[0]]);

        let mut duplicates = [single[0].clone(), reverse[0].clone()];
        compile(&mut duplicates, &Routing::default()).unwrap();
        assert_eq!(duplicates[0].incidences, single[0].incidences);
        assert_eq!(duplicates[1].incidences, reverse[0].incidences);
    }
}
