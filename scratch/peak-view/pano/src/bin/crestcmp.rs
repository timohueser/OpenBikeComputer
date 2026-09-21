use obc_elevation::surface::{SurfaceCache, SurfaceReader};
use obc_formats::io::{ByteSource, SliceSource};

// crestcmp <plain.obcd> <crest.obcd>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let pb = std::fs::read(&a[0]).unwrap();
    let cb = std::fs::read(&a[1]).unwrap();
    let (ps, cs) = (SliceSource(&pb), SliceSource(&cb));
    let plain = SurfaceReader::parse(&ps as &dyn ByteSource).unwrap();
    let crest = SurfaceReader::parse(&cs as &dyn ByteSource).unwrap();
    let (mut pc, mut cc) = (cache(), cache());
    let (pc, cc) = (&mut *pc, &mut *cc);
    let g = plain.geometry(0).unwrap();
    let samples = 1u32 << (g.cell_log2 - g.posting_log2);
    let (y0, x0) = (g.cell_min_i * samples, g.cell_min_j * samples);
    let (rows, cols) = (u32::from(g.cell_rows) * samples, u32::from(g.cell_cols) * samples);
    let (mut lifted, mut total, mut sum, mut worst) = (0u64, 0u64, 0f64, (0f32, 0u32, 0u32));
    let mut hist = [0u64; 11];
    for y in y0..y0 + rows {
        for x in x0..x0 + cols {
            let (Some(p), Some(c)) = (plain.patch(pc, 0, y, x), crest.patch(cc, 0, y, x)) else {
                continue;
            };
            total += 1;
            let d = c.height - p.height;
            if d > 0.0 {
                lifted += 1;
                sum += f64::from(d);
                hist[((d / 50.0) as usize).min(10)] += 1;
                if d > worst.0 {
                    worst = (d, y, x);
                }
            }
        }
    }
    let origin = -(1i64 << 28);
    let step = 1i64 << g.posting_log2;
    let (lat, lon) = (origin + i64::from(worst.1) * step, origin + i64::from(worst.2) * step);
    println!("nodes {total}, lifted {lifted} ({:.2} %), mean lift {:.1} m", 100.0 * lifted as f64 / total as f64, sum / lifted as f64);
    println!("max lift {:.0} m at {:.5},{:.5}", worst.0, lat as f64 / 1e6, lon as f64 / 1e6);
    for (i, n) in hist.iter().enumerate() {
        println!("  {:>4}..{:<4} m  {n}", i * 50, i * 50 + 50);
    }
}

fn cache() -> Box<SurfaceCache> {
    let mut slot = Box::<SurfaceCache>::new_uninit();
    unsafe {
        SurfaceCache::init_at(slot.as_mut_ptr());
        slot.assume_init()
    }
}
