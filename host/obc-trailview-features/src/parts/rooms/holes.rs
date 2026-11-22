//! The holes of a seam, from the row of the room before it (Rooms in the spec): a hole per room behind the line and a
//! door per lane that ends on it.

/// Where a room behind a seam starts: its hole's point on the line, and per lane its arm, its metres across to the left
/// of the seam's heading from that point, its half-width along the line and its metres along its arm.
#[derive(Clone, Debug)]
pub(super) struct Start {
    pub p: [f64; 2],
    pub lanes: Vec<(usize, f64, f64, f64)>,
}

/// The holes and the doors of a seam: per room behind the line its hole's metres across, half-width and start; per lane
/// that ends on the line its door's metres across and half-width, and its arm.
pub(super) struct Holes {
    pub holes: Vec<(f64, f64, Start)>,
    pub doors: Vec<(f64, f64, usize)>,
}

/// A length to the centimetre, and one down to it.
fn cm(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn cm_down(v: f64) -> f64 {
    ((v * 100.0) + 1e-4).floor() / 100.0
}

/// The holes of a seam in the row of the room before it (per lane its arm, its metres across to the left of the
/// heading, its half-width along the line and its metres along its arm), one per room behind the line (by the arms of
/// its lanes), and a door per lane of no room behind it. A hole runs from the right side of its first lane to the left
/// side of its last, a door over its lane; a side between two lanes lies in the middle between them. Each is cut back
/// by `shell_m` from a side that it shares with another, so the tubes behind stand a wall apart and no side stands on
/// the step between two lanes' floors. Each lane of a hole is across from its middle, its half-width kept inside the
/// hole. Metres across are to the centimetre, half-widths down to it. None where a room's lanes are not side by side in
/// the row, or a hole has no width; a door of no width is none.
pub(super) fn holes_of(row: &[(usize, f64, f64, f64)], children: &[Vec<usize>], shell: f64) -> Option<Holes> {
    let mut row = row.to_vec();
    row.sort_by(|x, y| x.1.total_cmp(&y.1));
    let n = row.len();
    let owner: Vec<Option<usize>> = row.iter().map(|l| children.iter().position(|c| c.contains(&l.0))).collect();
    let right =
        |k: usize| if k == 0 { row[0].1 - row[0].2 } else { (row[k - 1].1 + row[k - 1].2 + row[k].1 - row[k].2) / 2.0 };
    let left = |k: usize| if k + 1 == n { row[k].1 + row[k].2 } else { right(k + 1) };
    // A side of lane k at the next lane that is in another hole or door: there the two stand a wall apart, and no side
    // stands on the step between two floors.
    let cut = |k: usize, next: usize| owner[k].is_none() || owner[k] != owner[next];
    let span = |first: usize, last: usize| {
        let lo = right(first) + if first > 0 && cut(first, first - 1) { shell } else { 0.0 };
        let hi = left(last) - if last + 1 < n && cut(last, last + 1) { shell } else { 0.0 };
        let across = cm((lo + hi) / 2.0);
        (across, cm_down((across - lo).min(hi - across)))
    };
    let mut holes = Vec::new();
    for (c, arms) in children.iter().enumerate() {
        let at: Vec<usize> = (0..n).filter(|&k| owner[k] == Some(c)).collect();
        if at.len() != arms.len() || at.is_empty() || at.windows(2).any(|w| w[1] != w[0] + 1) {
            return None;
        }
        let (across, half) = span(at[0], at[at.len() - 1]);
        if half <= 0.0 {
            return None;
        }
        let lanes = (at.iter())
            .map(|&k| {
                let (arm, x, h, s) = row[k];
                let off = x - across;
                (arm, off, h.min(half - off).min(half + off).max(0.0), s)
            })
            .collect();
        holes.push((across, half, Start { p: [0.0, 0.0], lanes }));
    }
    let doors = (0..n)
        .filter(|&k| owner[k].is_none())
        .map(|k| {
            let (across, half) = span(k, k);
            (across, half, row[k].0)
        })
        .filter(|d| d.1 > 0.0)
        .collect();
    Some(Holes { holes, doors })
}
