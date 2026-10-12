//! An R2 target against the bucket of `OBC_R2_*`, in a throwaway folder `test/<time>-<pid>/` of
//! it: it never writes another key. It writes a live bucket, so it runs only when asked, and
//! without `OBC_R2_BUCKET` it skips. On a machine that publishes, with rclone on PATH:
//!
//! ```sh
//! (set -a; . tools/obc.local; set +a; cargo test -p obc-data --test r2_live -- --ignored --nocapture)
//! ```
//!
//! `OBC_R2_PUBLIC` names the origin that serves the bucket; the default is the one of `prod`.

use obc_data::lock::{self, Held, LockFile};
use obc_data::r2::Remote;
use obc_data::target::{Condition, Target};

/// Deletes the keys of the test when dropped.
struct Throwaway(Target, Vec<String>);

impl Drop for Throwaway {
    fn drop(&mut self) {
        for key in &self.1 {
            let _ = self.0.remove(key);
        }
    }
}

fn target() -> Option<Target> {
    let Some(bucket) = std::env::var("OBC_R2_BUCKET").ok().filter(|bucket| !bucket.is_empty()) else {
        eprintln!("skipped: OBC_R2_BUCKET is not set");
        return None;
    };
    let public = std::env::var("OBC_R2_PUBLIC").unwrap_or_else(|_| "https://maps.openbikecomputer.com".into());
    let prefix = format!("test/{}-{}", obc_data::date::now(), std::process::id());
    Some(Target::R2(Remote { public, bucket, prefix }))
}

#[test]
#[ignore = "writes the R2 bucket of OBC_R2_*: run it with --ignored"]
fn an_r2_target_writes_on_its_conditions_and_serves_a_key_with_a_hash() {
    let Some(target) = target() else { return };
    let key = "sources/test/1/a#b.txt";
    let _throwaway = Throwaway(target.clone(), vec!["release.json".into(), key.into(), lock::LOCK.into()]);

    let first = target.put("release.json", b"one", &Condition::Absent).unwrap().unwrap();
    assert_eq!(target.put("release.json", b"two", &Condition::Absent).unwrap(), None, "the key holds bytes");
    assert_eq!(target.get_tagged("release.json").unwrap(), Some((b"one".to_vec(), first.clone())));
    let second = target.put("release.json", b"two", &Condition::Match(first.clone())).unwrap().unwrap();
    assert_eq!(target.put("release.json", b"three", &Condition::Match(first)).unwrap(), None, "an old tag");
    assert!(target.put("release.json", b"four", &Condition::Match(second)).unwrap().is_some());
    let now = target.now().unwrap();
    assert!(now.abs_diff(obc_data::date::now()) < 300, "the server time is near this clock");

    let file = std::env::temp_dir().join(format!("obc-data-r2-live-{}", std::process::id()));
    std::fs::write(&file, b"hash").unwrap();
    assert_eq!(target.missing(&[(key.into(), 4)]).unwrap(), [key]);
    target.upload(&[(file.clone(), key.into())]).unwrap();
    let _ = std::fs::remove_file(&file);
    assert!(target.missing(&[(key.into(), 4)]).unwrap().is_empty());
    assert!(target.list().unwrap().iter().any(|listed| listed.key == key));
    let Target::R2(remote) = &target else { unreachable!() };
    let public = obc_data::fetch::http::Http::new().bytes(&remote.url(key), "*/*").unwrap();
    assert_eq!(public, b"hash", "the public URL encodes `#`");

    let held = Held::take(&target, "live", obc_data::date::now()).unwrap();
    let Some((LockFile::Held(holder), _)) = lock::read(&target).unwrap() else { panic!("free") };
    let (stale_at, now) = lock::stale_since(&target, &holder).unwrap();
    assert!(stale_at > now, "a young heartbeat by the clock of the server");
    held.release().unwrap();
    assert_eq!(lock::read(&target).unwrap().unwrap().0, LockFile::Free);

    target.remove(key).unwrap();
    assert_eq!(target.get_tagged(key).unwrap(), None);
}
