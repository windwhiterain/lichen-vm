//! The on-disk cell cache's container: a path, a key, and artifact bytes.
//! See docs/notes/heavy-value-cache.md §9.13.
//!
//! # Invariant
//!
//! The artifact bytes are stored **unchanged**: the artifact codec is the format, and this
//! container only says which path and which `K_in` a file belongs to. A malformed file is a
//! cache miss, never a panic — the directory is as untrusted as the artifact store (`D1`).

use std::fs;
use std::io;
use std::path::{Path as HostPath, PathBuf};

use lichen_language_parser::path::{Path, Step};

/// The container's own version, separate from the artifact's: it moves only when the framing does.
const FORMAT: u8 = 1;

/// One record: a cell's path, the `K_in` it was frozen under, and its artifact bytes.
pub struct Record {
    pub path: Path,
    /// The artifact's own module key, which its bytes were written with.
    pub module: u64,
    pub key: Vec<u64>,
    pub artifact: Vec<u8>,
}

/// One source's cache file.
fn file(dir: &HostPath, source_id: &str) -> PathBuf {
    dir.join(format!("cells-{:016x}.bin", tag(source_id)))
}

/// A stable tag for a source id: a name, hashed so any id is a legal file name.
fn tag(source_id: &str) -> u64 {
    source_id
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        })
}

/// Write `records` for `source_id`, replacing whatever was there.
pub fn write(dir: &HostPath, source_id: &str, records: &[Record]) -> io::Result<()> {
    let mut bytes = vec![FORMAT];
    put_u32(&mut bytes, records.len() as u32);
    for record in records {
        put_path(&mut bytes, &record.path);
        bytes.extend_from_slice(&record.module.to_le_bytes());
        put_u32(&mut bytes, record.key.len() as u32);
        for word in &record.key {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&(record.artifact.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&record.artifact);
    }
    fs::create_dir_all(dir)?;
    let target = file(dir, source_id);
    let temporary = target.with_extension("tmp");
    fs::write(&temporary, &bytes)?;
    fs::rename(&temporary, &target)
}

/// Read `source_id`'s records; a missing or malformed file is an empty cache.
pub fn read(dir: &HostPath, source_id: &str) -> Vec<Record> {
    match fs::read(file(dir, source_id)) {
        Ok(bytes) => decode(&bytes).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn decode(bytes: &[u8]) -> Option<Vec<Record>> {
    let mut cursor = 0usize;
    if take(bytes, &mut cursor, 1)?[0] != FORMAT {
        return None;
    }
    let count = u32_of(take(bytes, &mut cursor, 4)?)?;
    let mut records = Vec::new();
    for _ in 0..count {
        let path = read_path(bytes, &mut cursor)?;
        let module = u64_of(take(bytes, &mut cursor, 8)?)?;
        let key_len = u32_of(take(bytes, &mut cursor, 4)?)? as usize;
        let mut key = Vec::with_capacity(key_len.min(1024));
        for _ in 0..key_len {
            key.push(u64_of(take(bytes, &mut cursor, 8)?)?);
        }
        let len = u64::from_le_bytes(take(bytes, &mut cursor, 8)?.try_into().ok()?) as usize;
        let artifact = take(bytes, &mut cursor, len)?.to_vec();
        records.push(Record {
            path,
            module,
            key,
            artifact,
        });
    }
    Some(records)
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_path(out: &mut Vec<u8>, path: &Path) {
    put_u32(out, path.0.len() as u32);
    for step in &path.0 {
        match step {
            Step::Name(name) => {
                out.push(0);
                put_u32(out, name.len() as u32);
                out.extend_from_slice(name.as_bytes());
            }
            Step::Index(index) => {
                out.push(1);
                put_u32(out, *index);
            }
            Step::Tail => out.push(2),
        }
    }
}

fn read_path(bytes: &[u8], cursor: &mut usize) -> Option<Path> {
    let count = u32_of(take(bytes, cursor, 4)?)? as usize;
    let mut steps = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        match take(bytes, cursor, 1)?[0] {
            0 => {
                let len = u32_of(take(bytes, cursor, 4)?)? as usize;
                let name = std::str::from_utf8(take(bytes, cursor, len)?).ok()?;
                steps.push(Step::Name(name.to_string()));
            }
            1 => steps.push(Step::Index(u32_of(take(bytes, cursor, 4)?)?)),
            2 => steps.push(Step::Tail),
            _ => return None,
        }
    }
    Some(Path(steps))
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = cursor.checked_add(len)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(slice)
}

fn u32_of(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn u64_of(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}
