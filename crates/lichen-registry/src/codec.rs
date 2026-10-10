//! The binary byte codec the artifact and registry formats share.
//!
//! # Invariant
//!
//! Type-independent: the per-leaf value/operator codec traits stay in the lowlevel crate, since
//! they name `Program` values and operators.

use std::path::{Path, PathBuf};

/// The largest leaf name [`Writer::leaf`] can encode.
///
/// # Invariant
///
/// A leaf name is a Rust identifier from the vocabulary composition macro, never source text, so
/// this is a closed vocabulary: a longer name is a compiler bug, and truncating the one-byte
/// length would leave the rest of the name in the stream, desynchronising every field after it.
pub const MAX_LEAF_NAME_BYTES: usize = u8::MAX as usize;

/// A little-endian byte writer for the artifact format.
pub struct Writer {
    buf: Vec<u8>,
    /// The first leaf name that could not be encoded: once set, the writer's buffer
    /// is a prefix never to be handed out.
    error: Option<String>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer {
            buf: Vec::new(),
            error: None,
        }
    }
    pub fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }
    pub fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }
    pub fn u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }
    pub fn bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }
    pub fn path(&mut self, path: &Path) {
        let bytes = path.to_string_lossy();
        self.u32(bytes.len() as u32);
        self.buf.extend_from_slice(bytes.as_bytes());
    }
    /// Write a length-prefixed leaf-name discriminator: a one-byte length then the
    /// name bytes.
    ///
    /// # Invariant
    ///
    /// The composed `ProgramCodec` tags every value/operator leaf with its carry-variant name; a
    /// name longer than `MAX_LEAF_NAME_BYTES` is refused, since writing it would desynchronise the
    /// stream.
    pub fn leaf(&mut self, name: &str) -> Result<(), String> {
        if name.len() > MAX_LEAF_NAME_BYTES {
            let error = format!(
                "leaf name of {} bytes exceeds the {MAX_LEAF_NAME_BYTES}-byte limit",
                name.len()
            );
            self.error.get_or_insert_with(|| error.clone());
            return Err(error);
        }
        self.u8(name.len() as u8);
        self.bytes(name.as_bytes());
        Ok(())
    }
    /// The finished buffer, or the first leaf name `Writer::leaf` refused.
    ///
    /// # Invariant
    ///
    /// Wherever a leaf name can have been written this is the only way a refusal reaches the
    /// caller, instead of a buffer that misparses.
    pub fn finish(self) -> Result<Vec<u8>, String> {
        match self.error {
            Some(error) => Err(error),
            None => Ok(self.buf),
        }
    }
    /// The finished buffer, infallibly.
    ///
    /// # Invariant
    ///
    /// Nothing on this path goes through `Writer::leaf` — not the registry, not the artifact
    /// header — so no name can be refused; a writer that refused one must use `Writer::finish`.
    pub fn into_bytes(self) -> Vec<u8> {
        self.finish()
            .expect("no leaf name is written on this path, so none can be refused")
    }
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

/// A little-endian byte reader over an artifact buffer.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        let byte = *self.buf.get(self.pos).ok_or("truncated artifact")?;
        self.pos += 1;
        Ok(byte)
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }
    pub fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        // `self.pos + len` is an unchecked add: a crafted length wraps in
        // release and makes the bounds check below vacuous.
        let end = self.pos.checked_add(len).ok_or("truncated artifact")?;
        if end > self.buf.len() {
            return Err("truncated artifact".into());
        }
        let bytes = &self.buf[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }
    pub fn path(&mut self) -> Result<PathBuf, String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        Ok(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
    }
    /// Read a length-prefixed leaf-name discriminator (see `Writer::leaf`); the
    /// slice borrows the buffer, not the reader.
    pub fn leaf_name(&mut self) -> Result<&'a [u8], String> {
        let len = self.u8()? as usize;
        self.take(len)
    }
    /// How many bytes are still unread in the buffer.
    ///
    /// # Invariant
    ///
    /// Every length read out of the stream is checked against this: a list of `n` elements costs at
    /// least `n` bytes, so a larger declared count was produced by no writer, and preallocating
    /// from it would let a tiny file request an enormous allocation.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
    /// How many bytes have been read so far — the read offset into the buffer.
    ///
    /// # Invariant
    ///
    /// The artifact container finds its header's end here: the body digest covers exactly the bytes
    /// after the header, and the caller slices them from the original buffer at this offset.
    pub fn position(&self) -> usize {
        self.pos
    }
    pub fn done(&self) -> bool {
        self.pos == self.buf.len()
    }
}
