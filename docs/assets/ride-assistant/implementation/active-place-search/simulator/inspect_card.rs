use obc_formats::io::ByteSource;
use obc_storage::flat::{BlockDevice, FlatStore, ObjectKind, Store};
use std::{fs::File, io::{self, Write}, os::unix::fs::FileExt, path::Path};
struct ReadOnly(File);
impl BlockDevice for ReadOnly {
    type Error = io::Error;
    fn block_count(&self) -> io::Result<u64> { Ok(self.0.metadata()?.len()/512) }
    fn read(&self, lba:u64, bytes:&mut[u8])->io::Result<()> { self.0.read_exact_at(bytes,lba*512) }
    fn write(&self,_:u64,_:&[u8])->io::Result<()> { Err(io::ErrorKind::PermissionDenied.into()) }
    fn sync(&self)->io::Result<()> { Ok(()) }
}
fn geometry(route:&obc_route::RouteReader)->Vec<obc_route::RoutePoint> {
    let mut all = Vec::new();
    let mut buf = heapless::Vec::new();
    for k in 0..route.chunks().len() {
        route.decode_chunk(k,&mut buf).unwrap();
        all.extend(buf.iter().copied().skip(usize::from(k>0)));
    }
    all
}
fn span(route:&obc_route::RouteReader,lo:u32,hi:u32)->Vec<(i32,i32)> {
    let mut out = Vec::new();
    route.visit_points_between(lo,hi,|points| {
        for &p in points { if out.last()!=Some(&p) {out.push(p);} }
    });
    out
}
fn main() {
    let args:Vec<_>=std::env::args().collect();
    let store=FlatStore::mount(ReadOnly(File::open(&args[1]).unwrap()));
    assert!(store.mode().readable());
    let output=Path::new(&args[2]);
    std::fs::create_dir_all(output).unwrap();
    println!("store {:?}; sequence {}; checkpoint {:?}",store.store_id(),store.sequence(),obc_storage::flat::metadata::read_checkpoint(&store).unwrap());
    let mut candidates=0;
    for entry in store.entries().filter(|e|e.kind==ObjectKind::Route) {
        store.with_source(entry.id,Some(entry.revision),|source| {
            let mut bytes=vec![0;source.len() as usize];source.read_at(0,&mut bytes).unwrap();
            let mut crc=obc_crc::Crc32::new();crc.update(&bytes);assert_eq!(crc.finalize(),entry.payload_crc);
            let info=obc_route::RouteObjectInfo::read(source).unwrap();
            let index=obc_route::RouteIndex::read(source).unwrap();
            let route=obc_route::RouteReader::new(&index,source);
            let points=geometry(&route);
            let name=format!("route-{}-{}",entry.id.0,entry.revision.0);
            std::fs::write(output.join(format!("{name}.obcr")),&bytes).unwrap();
            let mut csv=File::create(output.join(format!("{name}.csv"))).unwrap();
            writeln!(csv,"lon,lat,ele,surface,incomplete").unwrap();
            for p in &points {writeln!(csv,"{},{},{},{},{}",p.lon,p.lat,p.ele,p.surface,p.elevation_incomplete).unwrap();}
            println!("route {} revision {} bytes {} candidate {} total_m {} vertices {} visit {:?}",entry.id.0,entry.revision.0,source.len(),info.assistant_candidate,route.total_distance_m,points.len(),info.visit);
            if let Some(visit)=info.visit {
                candidates+=1;
                let preview=route.assistant_preview_polyline::<64>().unwrap();
                let mut shape=File::create(output.join(format!("{name}-preview.csv"))).unwrap();
                writeln!(shape,"lon,lat").unwrap();
                for point in &preview {writeln!(shape,"{},{}",point.0,point.1).unwrap();}
                let near=|a:(i32,i32),b:(i32,i32)| {
                    let scale=std::f64::consts::PI/180.0*6_371_000.0/1_000_000.0;
                    let x=f64::from(a.0-b.0)*scale*(f64::from(b.1)/1_000_000.0).to_radians().cos();
                    let y=f64::from(a.1-b.1)*scale;x.hypot(y)<=1.01
                };
                let stop=route.position_at(visit.accepted_anchors_m[1]).unwrap();
                let end=route.position_at(visit.accepted_anchors_m[2]).unwrap();
                assert!(preview.iter().any(|&p|near(p,(stop.lon,stop.lat))),"stop missing from preview");
                assert!(preview.last().is_some_and(|&p|near(p,(end.lon,end.lat))),"preview exceeds rejoin");
                println!("  preview {} vertices; stop and final rejoin retained; displayed interval 0..{}m, stored {}m",preview.len(),visit.accepted_anchors_m[2],route.total_distance_m);
                assert_eq!(visit.original.store,store.store_id().0);
                store.with_source(obc_storage::flat::ObjectId(visit.original.object),Some(obc_storage::flat::Revision(visit.original.revision)),|original| {
                    let oi=obc_route::RouteIndex::read(original).unwrap();
                    let or=obc_route::RouteReader::new(&oi,original);
                    let prefix=span(&or,visit.original_anchors_m[0],visit.original_anchors_m[1]);
                    let mut suffix=span(&or,visit.original_anchors_m[2],or.total_distance_m);
                    // The builder retains the actual terminal vertex beyond the floored total.
                    if let (Some(end),Some(last))=(geometry(&or).last(),suffix.last_mut()) {*last=(end.lon,end.lat);}
                    // At the join the builder coalesces the sub-metre quantized seam point.
                    let tail_start=if !suffix.is_empty() {Some(suffix.remove(0))}else{None};
                    let mut full:Vec<_>=points.iter().map(|p|(p.lon,p.lat)).collect();
                    full.dedup();
                    let prefix_exact=full.starts_with(&prefix);
                    let suffix_exact=full.ends_with(&suffix);
                    let tail_seam_m=tail_start.map(|expected| {
                        let actual=full[full.len()-suffix.len()-1];
                        let scale=std::f64::consts::PI/180.0*6_371_000.0/1_000_000.0;
                        let x=f64::from(actual.0-expected.0)*scale*(f64::from(expected.1)/1_000_000.0).to_radians().cos();
                        let y=f64::from(actual.1-expected.1)*scale;
                        x.hypot(y)
                    });
                    println!("  coalesced tail seam distance_m {:?}",tail_seam_m);
                    assert!(tail_seam_m.is_none_or(|m|m<=1.01),"tail join exceeds one metre");
                    println!("  preserved prefix {} vertices exact {}; tail {} vertices exact {}; source_progress {} source_rejoin {} accepted {:?}",prefix.len(),prefix_exact,suffix.len(),suffix_exact,visit.original_anchors_m[0],visit.original_anchors_m[2],visit.accepted_anchors_m);
                    if !prefix_exact {println!("  prefix mismatch {:?}",full.iter().zip(&prefix).enumerate().find(|(_, (a,b))|a!=b));}
                    if !suffix_exact {println!("  suffix mismatch {:?}",full.iter().rev().zip(suffix.iter().rev()).enumerate().find(|(_, (a,b))|a!=b));}
                    assert!(prefix_exact,"original prefix differs");assert!(suffix_exact,"original tail differs");
                }).unwrap();
            }
        }).unwrap();
    }
    assert!(store.entries_ok());
    println!("visit_candidates {candidates}");
}
