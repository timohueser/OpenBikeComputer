use crate::column::Column;
use crate::{sort, Graph};

/// Keep nearby arrival states in the same graph, cost and geometry pages.
/// The returned permutation maps each new road ID to its original ID.
pub fn spatial_order(graph: &mut Graph) -> Result<Column<u32>, String> {
    let count = u32::try_from(graph.roads.len()).map_err(|_| "Too many roads")?;
    let keys = (0..count)
        .map(|road| {
            let point = graph.roads.shape(road as usize).last().ok_or("Road lacks geometry")?;
            if point.lon.unsigned_abs() > 180_000_000 || point.lat.unsigned_abs() > 85_000_000 {
                return Err("Road coordinate outside supported bounds");
            }
            let key =
                spread((point.lon as i64 + 180_000_000) as u64) | (spread((point.lat as i64 + 90_000_000) as u64) << 1);
            Ok((key, road))
        })
        .collect::<Result<Column<_>, _>>()?;
    let order: Column<u32> = sort::sorted(keys).map(|(_, road)| road).collect();
    let mut inverse = Column::filled(order.values().len(), 0);
    let inverse = inverse.values_mut();
    for (new, &old) in order.values().iter().enumerate() {
        inverse[old as usize] = new as u32;
    }
    for turns in [graph.forbidden.values_mut(), graph.forbidden_foot.values_mut()] {
        for (from, to) in turns.iter_mut() {
            *from = *inverse.get(*from as usize).ok_or("Unknown restriction road")?;
            *to = *inverse.get(*to as usize).ok_or("Unknown restriction road")?;
        }
        turns.sort_unstable();
    }
    graph.roads.permute(order.values());
    Ok(order)
}

fn spread(mut value: u64) -> u64 {
    value = (value | value << 16) & 0x0000ffff0000ffff;
    value = (value | value << 8) & 0x00ff00ff00ff00ff;
    value = (value | value << 4) & 0x0f0f0f0f0f0f0f0f;
    value = (value | value << 2) & 0x3333333333333333;
    (value | value << 1) & 0x5555555555555555
}
