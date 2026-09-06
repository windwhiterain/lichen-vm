//! The binary byte codec used by the artifact and registry formats.
//!
//! A little-endian byte reader/writer over a byte buffer, plus the
//! path/leaf-name helpers the registry and artifact formats share.  This is
//! the type-independent half of the codec: the per-leaf *value* and *operator*
//! codec traits ([`liche_lowlevel::codec::ValueCodec`] /
//! [`OperatorCodec`]) stay in the lowlevel crate (they name `Program` values
//! and operators).

use std::path::{Path, PathBuf};

/// A little-endian byte writer for the artifact format.
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer { buf: Vec::new() }
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
    /// Write a length-prefixed leaf-name discriminator: a one-byte length then
    /// the name bytes.  The composed `ProgramCodec` tags every value/operator
    /// leaf with its carry-variant name so the reader knows which leaf codec
    /// to dispatch to.
    pub fn leaf(&mut self, name: &str) {
        self.u8(name.len() as u8);
        self.bytes(name.as_bytes());
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
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
        if self.pos + len > self.buf.len() {
            return Err("truncated artifact".into());
        }
        let bytes = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Ok(bytes)
    }
    pub fn path(&mut self) -> Result<PathBuf, String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        Ok(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
    }
    /// Read a length-prefixed leaf-name discriminator (see [`Writer::leaf`]).
    /// The returned slice borrows the buffer, not the reader, so the reader
    /// stays usable for the leaf payload.
    pub fn leaf_name(&mut self) -> Result<&'a [u8], String> {
        let len = self.u8()? as usize;
        self.take(len)
    }
    pub fn done(&self) -> bool {
        self.pos == self.buf.len()
    }
}
