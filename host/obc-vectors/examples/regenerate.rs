use obc_vectors::{all, dir};

fn main() {
    std::fs::create_dir_all(dir()).unwrap();
    for (name, bytes) in all() {
        std::fs::write(dir().join(name), bytes).unwrap();
    }
}
