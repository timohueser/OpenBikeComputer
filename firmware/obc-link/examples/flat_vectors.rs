fn main() {
    let written = obc_link::flat::vectors::write_all().expect("the suite writes");
    println!("wrote {written} files");
}
