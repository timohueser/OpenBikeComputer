use obc_host_core::{flat_store::HostStore, FlatRouteStore, RouteRepository};
fn main() {
    let owner = HostStore::open_file(std::env::args().nth(1).unwrap()).unwrap();
    let store = FlatRouteStore::new(owner, &[]).unwrap();
    let checkpoint = store.read_checkpoint().unwrap().expect("accepted checkpoint");
    assert_eq!(store.fingerprint(checkpoint.route.object), Some(checkpoint.route));
    let original = checkpoint.original.expect("original route retained");
    assert_eq!(store.fingerprint(original.object), Some(original));
    assert_ne!(checkpoint.route.object, original.object);
    println!("checkpoint {:?}", checkpoint);
    println!("routes {:?}", store.ids());
    for route in store.catalog() {
        println!("route {:?}", route);
    }
    println!("Exact accepted and original payload fingerprints verified after card remount.");
}
