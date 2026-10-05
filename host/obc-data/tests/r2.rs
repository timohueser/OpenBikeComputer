//! The R2 client against rclone's `local` backend: a temporary directory stands in for the bucket.

use std::path::{Path, PathBuf};

use obc_data::r2::{Bucket, Put, Upload};

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("obc-data-r2-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bucket")).unwrap();
        Self(dir)
    }

    fn bucket(&self) -> PathBuf {
        self.0.join("bucket")
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn put(bucket: &Bucket, file: &Path, key: &str) {
    assert_eq!(bucket.put(file, key, &Upload::default()).unwrap(), Put::Uploaded);
}

#[test]
fn verify_detects_a_mismatch_and_an_immutable_key_keeps_its_bytes() {
    let temp = Temp::new("verify");
    let bucket = Bucket::local(&temp.bucket());
    let original = temp.file("original", b"cell bytes");
    let same_size = temp.file("same-size", b"cell bytez");
    put(&bucket, &original, "cells/a.obcm");
    bucket.verify(&original, "cells/a.obcm").unwrap();

    let error = bucket.verify(&same_size, "cells/a.obcm").unwrap_err();
    assert!(error.contains("MD5"), "{error}");
    let error = bucket.verify(&temp.file("short", b"cell"), "cells/a.obcm").unwrap_err();
    assert!(error.contains("holds 10 bytes"), "{error}");
    assert!(bucket.verify(&original, "cells/b.obcm").unwrap_err().contains("not in"));

    let immutable = Upload { immutable: true, ..Upload::default() };
    assert!(bucket.put(&same_size, "cells/a.obcm", &immutable).unwrap_err().contains("immutable"));
    assert_eq!(bucket.put(&original, "cells/a.obcm", &immutable).unwrap(), Put::AlreadyThere);
    assert_eq!(std::fs::read(temp.bucket().join("cells/a.obcm")).unwrap(), b"cell bytes");
}

#[test]
fn stat_tells_an_absent_object_from_an_empty_one() {
    let temp = Temp::new("stat");
    let bucket = Bucket::local(&temp.bucket());
    put(&bucket, &temp.file("empty", b""), "cells/empty.obcm");
    let keys = ["cells/empty.obcm".to_string(), "cells/never.obcm".to_string()];
    let found = bucket.stat(&keys).unwrap();
    assert_eq!(found.keys().collect::<Vec<_>>(), ["cells/empty.obcm"]);
    assert_eq!(found["cells/empty.obcm"].bytes, 0);
    assert!(bucket.list("nothing/here").unwrap().is_empty());
}
