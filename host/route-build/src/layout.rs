use crate::Graph;

/// Keep nearby arrival states in the same graph, cost and geometry pages.
/// The returned permutation maps each new road ID to its original ID.
pub fn spatial_order(graph: &mut Graph) -> Result<Vec<u32>, String> {
    let keys = graph
        .roads
        .iter()
        .map(|road| {
            let point = road.shape.last().ok_or("Road lacks geometry")?;
            if point.lon.unsigned_abs() > 180_000_000 || point.lat.unsigned_abs() > 85_000_000 {
                return Err("Road coordinate outside supported bounds");
            }
            Ok(spread((point.lon as i64 + 180_000_000) as u64) | (spread((point.lat as i64 + 90_000_000) as u64) << 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let count = u32::try_from(graph.roads.len()).map_err(|_| "Too many roads")?;
    let mut order: Vec<_> = (0..count).collect();
    order.sort_unstable_by_key(|&road| (keys[road as usize], road));
    let mut inverse = vec![0; order.len()];
    for (new, &old) in order.iter().enumerate() {
        inverse[old as usize] = new as u32;
    }
    for turns in [&mut graph.forbidden, &mut graph.forbidden_foot] {
        for (from, to) in turns.iter_mut() {
            *from = *inverse.get(*from as usize).ok_or("Unknown restriction road")?;
            *to = *inverse.get(*to as usize).ok_or("Unknown restriction road")?;
        }
        turns.sort_unstable();
    }
    for from in 0..inverse.len() {
        while inverse[from] as usize != from {
            let to = inverse[from] as usize;
            graph.roads.swap(from, to);
            inverse.swap(from, to);
        }
    }
    Ok(order)
}

fn spread(mut value: u64) -> u64 {
    value = (value | value << 16) & 0x0000ffff0000ffff;
    value = (value | value << 8) & 0x00ff00ff00ff00ff;
    value = (value | value << 4) & 0x0f0f0f0f0f0f0f0f;
    value = (value | value << 2) & 0x3333333333333333;
    (value | value << 1) & 0x5555555555555555
}
