//! `replay` part `hmac`: the local key and HMAC-keyed case ids (RP1, design §13.7, N-I2).
//!
//! - HMAC-SHA256 (RFC 2104) is built on `sha2`, so no new crate (lane notes).
//! - The key is 32 bytes from `/dev/urandom`, created once as `<replay>/hmac.key`, mode 0600,
//!   with `O_EXCL | O_NOFOLLOW`. It never leaves the replay folder: only ids derived from it
//!   reach the committed report.
//! - A case id is the first 16 hex digits of `HMAC-SHA256(key, canonical envelope)`, so a short
//!   command cannot be recovered from the report by guessing without the key.

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use sha2::{Digest, Sha256};

use anthology_core::fsx;

/// The key file's name inside the replay folder (`.gitignore`d in any directory).
pub const KEY_FILE: &str = "hmac.key";
/// The key length in bytes.
pub const KEY_LEN: usize = 32;
/// The length of a case id in hex digits.
pub const ID_HEX: usize = 16;

const BLOCK: usize = 64;

/// The local replay key.
#[derive(Clone)]
pub struct Key([u8; KEY_LEN]);

impl Key {
    /// A key from its bytes (tests).
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Key {
        Key(bytes)
    }

    /// The case id of `canonical`: `HMAC-SHA256(key, canonical)[:16]` in lowercase hex.
    pub fn case_id(&self, canonical: &[u8]) -> String {
        let mac = hmac_sha256(&self.0, canonical);
        let mut id = hex(&mac);
        id.truncate(ID_HEX);
        id
    }
}

impl std::fmt::Debug for Key {
    // Never print the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}

/// HMAC-SHA256 of `msg` under `key` (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= block[i];
        opad[i] ^= block[i];
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 15)] as char);
    }
    out
}

/// The key in `dir`, created first when it is missing (`replay collect`).
pub fn load_or_create(dir: &Path) -> Result<Key, String> {
    let path = dir.join(KEY_FILE);
    if std::fs::symlink_metadata(&path).is_err() {
        let mut bytes = [0u8; KEY_LEN];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
        match fsx::create_excl(&path, &bytes, 0o600, true) {
            Ok(()) => return Ok(Key(bytes)),
            // Another collect made it first: use that one.
            Err(e) if path.exists() => drop(e),
            Err(e) => return Err(format!("cannot create {}: {}", path.display(), e.py_str())),
        }
    }
    load(dir)
}

/// The key in `dir`: a regular file (not a symlink) of exactly 32 bytes that only its owner
/// can read or write.
pub fn load(dir: &Path) -> Result<Key, String> {
    let path = dir.join(KEY_FILE);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| {
        format!(
            "no replay key at {}; run `anthology-dev replay collect` first",
            path.display()
        )
    })?;
    if !meta.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "{} is readable by others (mode {:o}); it must be 0600",
            path.display(),
            meta.mode() & 0o777
        ));
    }
    let bytes = fsx::read_regular(&path, Some(KEY_LEN as u64 + 1))
        .map_err(|e| format!("cannot read {}: {}", path.display(), e.py_str()))?;
    let key: [u8; KEY_LEN] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("{} is not {KEY_LEN} bytes", path.display()))?;
    Ok(Key(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// RFC 4231 test cases 1, 2, 3, 4, 6 and 7 (5 is a truncated MAC).
    #[test]
    fn hmac_matches_rfc_4231() {
        let long_key = vec![0xaa; 131];
        let cases: [(Vec<u8>, Vec<u8>, &str); 6] = [
            (
                vec![0x0b; 20],
                b"Hi There".to_vec(),
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            ),
            (
                b"Jefe".to_vec(),
                b"what do ya want for nothing?".to_vec(),
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                vec![0xaa; 20],
                vec![0xdd; 50],
                "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
            ),
            (
                unhex("0102030405060708090a0b0c0d0e0f10111213141516171819"),
                vec![0xcd; 50],
                "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
            ),
            (
                long_key.clone(),
                b"Test Using Larger Than Block-Size Key - Hash Key First".to_vec(),
                "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
            ),
            (
                long_key,
                b"This is a test using a larger than block-size key and a larger than \
                  block-size data. The key needs to be hashed before being used by the HMAC \
                  algorithm."
                    .to_vec(),
                "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
            ),
        ];
        for (key, msg, want) in cases {
            assert_eq!(hex(&hmac_sha256(&key, &msg)), want);
        }
    }

    #[test]
    fn a_case_id_is_16_hex_digits_and_depends_on_the_key() {
        let a = Key::from_bytes([1; KEY_LEN]);
        let b = Key::from_bytes([2; KEY_LEN]);
        let id = a.case_id(b"ls");
        assert_eq!(id.len(), ID_HEX);
        assert!(
            id.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(id, a.case_id(b"ls"));
        assert_ne!(id, b.case_id(b"ls"));
        assert_ne!(id, a.case_id(b"ls "));
        assert_eq!(format!("{a:?}"), "Key(..)");
    }

    #[test]
    fn the_key_is_created_once_with_mode_0600() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = load_or_create(dir.path()).expect("create");
        let meta = std::fs::metadata(dir.path().join(KEY_FILE)).expect("meta");
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(meta.len(), KEY_LEN as u64);
        let again = load_or_create(dir.path()).expect("load");
        assert_eq!(first.case_id(b"x"), again.case_id(b"x"));
        assert_eq!(
            load(dir.path()).expect("load").case_id(b"x"),
            first.case_id(b"x")
        );
        // 32 random bytes are never all zero in practice.
        assert_ne!(first.0, [0u8; KEY_LEN]);
    }

    #[test]
    fn a_bad_key_file_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = load(dir.path()).expect_err("missing");
        assert!(
            err.contains("run `anthology-dev replay collect` first"),
            "{err}"
        );
        let path = dir.path().join(KEY_FILE);
        std::fs::write(&path, [0u8; 31]).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert!(
            load(dir.path())
                .expect_err("short")
                .contains("is not 32 bytes")
        );
        std::fs::write(&path, [0u8; 32]).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(
            load(dir.path())
                .expect_err("mode")
                .contains("it must be 0600")
        );
        std::fs::remove_file(&path).expect("rm");
        let target = dir.path().join("elsewhere");
        std::fs::write(&target, [0u8; 32]).expect("write");
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        assert!(
            load(dir.path())
                .expect_err("symlink")
                .contains("is not a regular file")
        );
        // load_or_create does not replace a symlink either.
        assert!(load_or_create(dir.path()).is_err());
    }
}
