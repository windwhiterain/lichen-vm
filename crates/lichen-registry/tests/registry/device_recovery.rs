//! Device-registry recovery: an unreadable registry and its artifacts are
//! preserved, and the key space restarts empty.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use lichen_registry::{DeviceRegistry, ModuleKey};

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lichen-registry-{name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Read `registry.corrupt` or its first numbered sibling.
fn preserved_registry(dir: &std::path::Path) -> PathBuf {
    let plain = dir.join("registry.corrupt");
    if plain.is_file() {
        return plain;
    }
    for suffix in 1..16 {
        let candidate = dir.join(format!("registry.corrupt.{suffix}"));
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!(
        "the recovery preserved no registry bytes under {}",
        dir.display()
    )
}

#[test]
fn an_unreadable_registry_is_preserved_and_the_key_space_restarts_clean() {
    let dir = temp_dir("recover");
    let mut device = DeviceRegistry::open(dir.clone());
    let (key, is_new) = device.alloc("/x/pkg.lichen");
    assert!(is_new);
    device.publish("/x/pkg.lichen", key, [7; 32], Vec::new());
    device.store_artifact("/x/pkg.lichen", b"artifact bytes");
    drop(device);
    assert_eq!(fs::read_dir(dir.join("artifacts")).unwrap().count(), 1);

    // Bit rot, or an interrupted writer, leaves bytes the parser rejects.
    fs::write(dir.join("registry"), b"LCHREGgarbage").unwrap();

    let mut recovered = DeviceRegistry::open(dir.clone());

    assert_eq!(
        recovered.entry_count(),
        0,
        "an index that cannot be read is not trusted as state"
    );
    assert_eq!(
        fs::read(preserved_registry(&dir)).unwrap(),
        b"LCHREGgarbage",
        "the unreadable bytes are preserved where the recovery said they went"
    );
    assert!(
        dir.join("artifacts.corrupt").is_dir(),
        "the artifacts the lost index alone could describe are moved aside with it"
    );
    assert_eq!(
        fs::read_dir(dir.join("artifacts")).unwrap().count(),
        0,
        "the restarted key space has no artifact on disk to collide with"
    );

    // The store keeps working: the key space starts over, and the new state
    // survives a reopen.
    let (fresh, is_new) = recovered.alloc("/x/pkg.lichen");
    assert!(is_new);
    assert_eq!(fresh, ModuleKey::from_raw(0));
    recovered.publish("/x/pkg.lichen", fresh, [9; 32], Vec::new());
    drop(recovered);

    let mut reopened = DeviceRegistry::open(dir.clone());
    assert_eq!(reopened.entry_count(), 1);
    assert_eq!(
        reopened.alloc("/x/pkg.lichen"),
        (fresh, false),
        "the restarted registry is a working store, not a wedged one"
    );

    // A second corruption preserves the second registry too: the first
    // quarantine is never overwritten.
    fs::write(dir.join("registry"), b"LCHREGgarbage2").unwrap();
    let _second = DeviceRegistry::open(dir.clone());
    assert_eq!(
        fs::read(preserved_registry(&dir)).unwrap(),
        b"LCHREGgarbage",
        "the first preserved registry is still readable"
    );
    let second = dir.join("registry.corrupt.1");
    assert_eq!(fs::read(&second).unwrap(), b"LCHREGgarbage2");
}

#[test]
fn a_recovery_does_not_hand_out_a_key_this_process_already_used() {
    // The quarantine removed every artifact, so the restarted space must still
    // stay above any key already handed out here.
    let dir = temp_dir("recover-frontier");
    let mut device = DeviceRegistry::open(dir.clone());
    let (first, _) = device.alloc("/x/first.lichen");
    assert_eq!(first, ModuleKey::from_raw(0));
    device.publish("/x/first.lichen", first, [1; 32], Vec::new());

    fs::write(dir.join("registry"), b"LCHREGgarbage").unwrap();
    let (second, is_new) = device.alloc("/x/second.lichen");
    assert!(is_new);
    assert!(
        second.as_raw() > first.as_raw(),
        "a restarted space must not reuse a key already handed out in this process"
    );
    device.publish("/x/second.lichen", second, [2; 32], Vec::new());
    drop(device);

    let reopened = DeviceRegistry::open(dir.clone());
    assert_eq!(
        reopened.entry_count(),
        1,
        "the entry table restarted; only the post-recovery entry is described"
    );
}

#[test]
fn gc_keeps_a_key_a_surviving_entry_still_names() {
    let dir = temp_dir("gcref");
    let mut device = DeviceRegistry::open(dir.clone());
    // A dead (non-lichen) entry, as only an out-of-band registration produces.
    let (dead_key, _) = device.alloc("junk.txt");
    device.publish("junk.txt", dead_key, [1; 32], Vec::new());
    // A surviving, kept entry that still names the dead entry's key.
    let (live_key, _) = device.alloc("/x/keep.lichen");
    device.publish(
        "/x/keep.lichen",
        live_key,
        [2; 32],
        vec![("junk.txt".to_string(), dead_key)],
    );

    assert_eq!(
        device.gc(),
        0,
        "an entry a surviving artifact still names is not reclaimed"
    );
    let (fresh, is_new) = device.alloc("/x/other.lichen");
    assert!(is_new);
    assert_ne!(
        fresh, dead_key,
        "the referenced key is not handed to a different module"
    );
}
