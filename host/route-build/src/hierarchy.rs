use crate::compact::Compact;
use route_engine::storage::{self, Arc, EdgeRef, Node, Page, NODES_PER_PAGE};

pub fn export(
    graph: &Compact,
    write: &mut impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<(Vec<String>, Vec<u32>), String> {
    let mut input = fast_paths::InputGraph::new();
    for (from, outgoing) in graph.arcs.iter().enumerate() {
        for arc in outgoing {
            let (to, cost) = (arc.to, arc.cost);
            if cost == 0 || cost >= u32::MAX as u64 {
                return Err("CH requires positive costs below u32::MAX".into());
            }
            input.add_edge(from, to as usize, cost as usize);
        }
    }
    input.freeze();
    let ch = fast_paths::prepare(&input);
    drop(input);
    // FastGraph32 is Fast Paths' public export type. No runtime dependency on its layout remains.
    let raw = std::panic::catch_unwind(|| fast_paths::FastGraph32::new(&ch))
        .map_err(|_| "Prepared CH exceeds the 32-bit export limits")?;
    drop(ch);
    let mut ranks = raw.ranks;
    let n = graph.arcs.len();
    ranks.extend((ranks.len()..n).map(|i| i as u32));
    let mut ordering = vec![0usize; n];
    for (id, &rank) in ranks.iter().enumerate() {
        ordering[rank as usize] = id;
    }
    let reference = |id: u32, backward: bool| {
        let edges = if backward { &raw.edges_bwd } else { &raw.edges_fwd };
        let offsets = if backward { &raw.first_edge_ids_bwd } else { &raw.first_edge_ids_fwd };
        let node = ranks[edges[id as usize].base_node as usize];
        EdgeRef { node, index: id - offsets[node as usize], backward }
    };
    let mut pages = Vec::new();
    let page_count = n.div_ceil(NODES_PER_PAGE as usize);
    for page_id in 0..page_count {
        let first = page_id * NODES_PER_PAGE as usize;
        let mut nodes = Vec::new();
        for &original in &ordering[first..(first + NODES_PER_PAGE as usize).min(n)] {
            let rank = ranks[original] as usize;
            let mut node = Node::default();
            for backward in [false, true] {
                let edges = if backward { &raw.edges_bwd } else { &raw.edges_fwd };
                let offsets = if backward { &raw.first_edge_ids_bwd } else { &raw.first_edge_ids_fwd };
                if rank + 1 >= offsets.len() {
                    continue;
                }
                let list = if backward { &mut node.backward } else { &mut node.forward };
                for edge in &edges[offsets[rank] as usize..offsets[rank + 1] as usize] {
                    let children = (edge.replaced_in_edge != u32::MAX)
                        .then(|| [reference(edge.replaced_in_edge, true), reference(edge.replaced_out_edge, false)]);
                    list.push(Arc {
                        to: ranks[edge.adj_node as usize],
                        cost: edge.weight as u64,
                        children,
                        road: if children.is_some() {
                            0
                        } else {
                            let (from, to) = if backward {
                                (edge.adj_node, edge.base_node)
                            } else {
                                (edge.base_node, edge.adj_node)
                            };
                            graph.arcs[from as usize]
                                .iter()
                                .filter(|a| a.to == to && a.cost == edge.weight as u64)
                                .min_by_key(|a| a.road)
                                .ok_or("Missing base road witness")?
                                .road
                        },
                    });
                }
            }
            nodes.push(node);
        }
        let encoded = storage::encode(&Page { first: first as u32, nodes })?;
        pages.push(write(&encoded)?);
    }
    Ok((pages, ranks))
}
