//! The slice of `crc-fast` that `aws-smithy-checksums` 0.65 actually calls.
//! Real `crc-fast` 1.10.0 depends on `digest` 0.10 → `block-buffer` 0.10.4
//! (GHSA-qwgh-2vcv-g2f7). Drop this patch when crc-fast moves to digest 0.11.

use crc::{Crc, CRC_32_ISCSI, CRC_32_ISO_HDLC, CRC_64_NVME};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);
const CRC32C: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);
const CRC64: Crc<u64> = Crc::<u64>::new(&CRC_64_NVME);

#[derive(Debug, Clone, Copy)]
pub enum CrcAlgorithm {
    Crc32IsoHdlc,
    Crc32Iscsi,
    Crc64Nvme,
}

pub struct Digest {
    inner: Inner,
}

enum Inner {
    C32(crc::Digest<'static, u32>),
    C64(crc::Digest<'static, u64>),
}

impl std::fmt::Debug for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Digest").finish_non_exhaustive()
    }
}

impl Digest {
    pub fn new(algorithm: CrcAlgorithm) -> Self {
        Self {
            inner: match algorithm {
                CrcAlgorithm::Crc32IsoHdlc => Inner::C32(CRC32.digest()),
                CrcAlgorithm::Crc32Iscsi => Inner::C32(CRC32C.digest()),
                CrcAlgorithm::Crc64Nvme => Inner::C64(CRC64.digest()),
            },
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        match &mut self.inner {
            Inner::C32(d) => d.update(data),
            Inner::C64(d) => d.update(data),
        }
    }

    pub fn finalize(self) -> u64 {
        match self.inner {
            Inner::C32(d) => u64::from(d.finalize()),
            Inner::C64(d) => d.finalize(),
        }
    }
}

pub fn checksum(algorithm: CrcAlgorithm, data: &[u8]) -> u64 {
    let mut digest = Digest::new(algorithm);
    digest.update(data);
    digest.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECK: &[u8] = b"123456789";

    #[test]
    fn iso_hdlc_check() {
        assert_eq!(checksum(CrcAlgorithm::Crc32IsoHdlc, CHECK), 0xcbf4_3926);
    }

    #[test]
    fn iscsi_check() {
        assert_eq!(checksum(CrcAlgorithm::Crc32Iscsi, CHECK), 0xe306_9283);
    }

    #[test]
    fn nvme_check() {
        assert_eq!(checksum(CrcAlgorithm::Crc64Nvme, CHECK), 0xae8b_1486_0a79_9888);
    }
}
