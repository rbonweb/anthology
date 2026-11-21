//! The export archive's bytes (MX1, m-20): what `memory_portable.export` writes with
//! `tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT)` inside
//! `gzip.GzipFile(filename="", mode="wb", fileobj=buf, mtime=0)`. The `tar` crate only reads
//! archives here; this writer is custom so the tar stream is byte-identical to CPython 3.12's.
#![allow(dead_code, reason = "skeleton: MX1-A fills the bodies")]

/// The tar stream CPython 3.12 writes for `members`, in the order given, each added as
/// `memory_portable._tarinfo(name, len(data))` (a regular file, mode 0644, uid and gid 0, empty
/// uname and gname, mtime 0) in PAX format: a `././@PaxHeader` extended header only where
/// tarfile writes one, the data padded to 512 bytes, then the end-of-archive blocks and the
/// padding to a 10240-byte record.
pub fn pax_archive(members: &[(&str, &[u8])]) -> Vec<u8> {
    let _ = members;
    todo!("MX1-A")
}

/// `gzip.GzipFile(filename="", mode="wb", mtime=0)` (compresslevel 9) around `data`: the
/// header CPython writes (no file name, mtime 0, XFL 2, OS 255), a deflate stream, CRC32 and
/// ISIZE. The deflate bytes may differ from zlib's (D-5); decompressed they are `data`.
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let _ = data;
    todo!("MX1-A")
}
