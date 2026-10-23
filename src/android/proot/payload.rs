//! Guest files embedded in the host library and written into the rootfs.
//!
//! The large ones (KWin, Turnip, Xwayland) are stored xz-compressed by
//! build.rs (`COMPRESSED_PAYLOADS`) and expanded only when the installed copy
//! is stale, which keeps them out of the APK's deflate and out of the
//! extracted `liblocaldesktop.so` at full size.

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

/// A guest file Portal installs.
pub trait GuestPayload {
    /// Size of the installed file.
    fn size(&self) -> u64;
    /// Recorded beside an installed copy to skip rewriting it on every launch.
    fn fingerprint(&self) -> String;
    fn write_to(&self, out: &mut dyn Write) -> io::Result<()>;
}

impl GuestPayload for [u8] {
    fn size(&self) -> u64 {
        self.len() as u64
    }

    fn fingerprint(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{}:{:016x}", self.len(), hasher.finish())
    }

    fn write_to(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(self)
    }
}

/// So a `&[u8]` constant can sit beside compressed ones as `&dyn GuestPayload`.
impl GuestPayload for &[u8] {
    fn size(&self) -> u64 {
        (**self).size()
    }

    fn fingerprint(&self) -> String {
        (**self).fingerprint()
    }

    fn write_to(&self, out: &mut dyn Write) -> io::Result<()> {
        (**self).write_to(out)
    }
}

/// An xz-compressed payload; build one with [`compressed_payload!`].
pub struct CompressedPayload {
    pub xz: &'static [u8],
    pub len: u64,
    pub sha256: &'static str,
}

impl CompressedPayload {
    /// Whether `path` holds exactly this payload. Hashes the file: for
    /// repairs and validation, not the per-launch freshness check.
    pub fn matches_file(&self, path: &Path) -> bool {
        let Ok(mut file) = fs::File::open(path) else {
            return false;
        };
        if file.metadata().map(|m| m.len()).ok() != Some(self.len) {
            return false;
        }
        let mut hasher = Sha256::new();
        let mut buffer = vec![0; 1 << 20];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => hasher.update(&buffer[..read]),
                Err(_) => return false,
            }
        }
        format!("{:x}", hasher.finalize()) == self.sha256
    }
}

impl GuestPayload for CompressedPayload {
    fn size(&self) -> u64 {
        self.len
    }

    fn fingerprint(&self) -> String {
        format!("{}:sha256:{}", self.len, self.sha256)
    }

    fn write_to(&self, out: &mut dyn Write) -> io::Result<()> {
        let written = io::copy(&mut xz2::read::XzDecoder::new(self.xz), out)?;
        if written != self.len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("payload expanded to {written} bytes, expected {}", self.len),
            ));
        }
        Ok(())
    }
}

/// Write `payload` to a new file at `path`.
pub fn write_file(path: &Path, payload: &(impl GuestPayload + ?Sized)) -> io::Result<()> {
    let mut file = io::BufWriter::with_capacity(1 << 20, fs::File::create(path)?);
    payload.write_to(&mut file)?;
    file.flush()
}

/// The [`CompressedPayload`] build.rs made from `assets/<path>`; the path
/// must be listed in its `COMPRESSED_PAYLOADS`.
macro_rules! compressed_payload {
    ($asset:literal) => {
        include!(concat!(env!("OUT_DIR"), "/payloads/", $asset, ".rs"))
    };
}
pub(crate) use compressed_payload;
