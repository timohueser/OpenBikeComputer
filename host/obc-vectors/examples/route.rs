//! Rebuild a route artifact from its retained GPX source with the production converter.
use obc_formats::io::{ByteSink, Error, SliceSource};
#[derive(Default)]
struct Sink(Vec<u8>);
impl ByteSink for Sink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn patch_at(&mut self, at: u32, bytes: &[u8]) -> Result<(), Error> {
        self.0[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--inspect" {
        let bytes = std::fs::read(&args[2]).unwrap();
        let source = SliceSource(&bytes);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = obc_route::RouteReader::new(&index, &source);
        let facts = route.interval_facts(0, route.total_distance_m).unwrap();
        println!("{facts:?}");
        assert_eq!(facts.ascent_m, route.total_ascent_m);
        assert_eq!(facts.descent_m, route.total_descent_m);
        let first = route.interval_facts(0, route.total_distance_m / 2).unwrap();
        let second = route.interval_facts(route.total_distance_m / 2, route.total_distance_m).unwrap();
        assert_eq!(first.ascent_m + second.ascent_m, facts.ascent_m);
        assert_eq!(first.descent_m + second.descent_m, facts.descent_m);
        for i in 0..8 {
            assert_eq!(first.surface_m[i] + second.surface_m[i], facts.surface_m[i]);
        }
        println!("Reload and adjacent-interval conservation passed");
        return;
    }
    assert_eq!(args.len(), 4, "route INPUT.gpx OUTPUT.obcr NAME | --inspect ROUTE.obcr");
    let source = std::fs::read(&args[1]).unwrap();
    let mut sink = Sink::default();
    let stats = obc_route::gpx_to_obcr(&SliceSource(&source), &args[3], &mut sink).unwrap();
    std::fs::write(&args[2], sink.0).unwrap();
    println!("{stats:?}");
}
