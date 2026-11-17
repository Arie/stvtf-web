//! An uploaded demo that came packed: gzip, or a zip with one demo in it. What comes out is cut
//! off at the most a demo may be, names are never paths, and nothing is unpacked twice.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail};

/// The most files a zip may list.
const MAX_ENTRIES: usize = 64;
/// A zip's end record without its comment, and the longest comment.
const END_LEN: usize = 22;
const MAX_COMMENT: usize = 0xffff;
/// A central directory entry, and a local header, without their names.
const ENTRY_LEN: usize = 46;
const LOCAL_LEN: usize = 30;

fn u16_at(b: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes([b[at], b[at + 1]]))
}

fn u32_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]))
}

/// Unpacks `part` in place, to at most `max` bytes: whether it was packed.
pub(super) fn unpack(part: &Path, max: u64) -> Result<bool> {
    let mut file = File::open(part).context("No such upload")?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).context("Not a demo")?;
    file.rewind()?;
    let packed: Box<dyn Read> = match magic {
        [0x1f, 0x8b, ..] => Box::new(flate2::read::GzDecoder::new(file)),
        [b'P', b'K', 3, 4] => demo_in_zip(file, max)?,
        _ => return Ok(false),
    };
    let out = part.with_extension("unpacked");
    let copied = File::create(&out).map_err(Into::into).and_then(|mut to| copy_at_most(packed, &mut to, max));
    if let Err(e) = copied.and_then(|()| Ok(std::fs::rename(&out, part)?)) {
        let _ = std::fs::remove_file(&out);
        return Err(e);
    }
    Ok(true)
}

/// Copies `from` to `to`, `max` bytes at most.
fn copy_at_most(from: impl Read, to: &mut File, max: u64) -> Result<()> {
    let copied = std::io::copy(&mut from.take(max + 1), to).ok().context("Could not unpack it")?;
    if copied > max {
        bail!("Over {} MB unpacked", max >> 20);
    }
    Ok(())
}

/// The one demo in the zip `file`, as it unpacks. Refused: none or several, encrypted, another
/// method, over `max`, too many files, zip64.
fn demo_in_zip(mut file: File, max: u64) -> Result<Box<dyn Read>> {
    let len = file.metadata()?.len();
    let tail_len = len.min((END_LEN + MAX_COMMENT) as u64);
    let mut tail = vec![0u8; tail_len as usize];
    file.seek(SeekFrom::Start(len - tail_len))?;
    file.read_exact(&mut tail)?;
    let end = tail.windows(4).rposition(|w| w == [b'P', b'K', 5, 6]).filter(|at| at + END_LEN <= tail.len()).context("Not a zip")?;
    let end = &tail[end..];
    let (entries, dir_len, dir_at) = (u16_at(end, 10), u32_at(end, 12), u32_at(end, 16));
    if entries > MAX_ENTRIES || dir_at.checked_add(dir_len).is_none_or(|to| to > len) || dir_len > (1 << 20) {
        bail!("Too many files in it");
    }
    let mut dir = vec![0u8; dir_len as usize];
    file.seek(SeekFrom::Start(dir_at))?;
    file.read_exact(&mut dir)?;
    let mut demo = None;
    let mut at = 0;
    for _ in 0..entries {
        let e = dir.get(at..at + ENTRY_LEN).filter(|e| e.starts_with(b"PK\x01\x02")).context("Not a zip")?;
        let (name_len, more) = (u16_at(e, 28), u16_at(e, 30) + u16_at(e, 32));
        let name = dir.get(at + ENTRY_LEN..at + ENTRY_LEN + name_len).context("Not a zip")?;
        if name.to_ascii_lowercase().ends_with(b".dem") {
            if demo.is_some() {
                bail!("More than one demo in it");
            }
            demo = Some((u16_at(e, 8), u16_at(e, 10), u32_at(e, 20), u32_at(e, 24), u32_at(e, 42)));
        }
        at += ENTRY_LEN + name_len + more;
    }
    let (flags, method, packed, unpacked, local_at) = demo.context("No demo in it")?;
    if flags & 1 != 0 {
        bail!("It is encrypted");
    }
    if packed == 0xffff_ffff || unpacked == 0xffff_ffff || unpacked > max {
        bail!("Over {} MB unpacked", max >> 20);
    }
    let mut local = [0u8; LOCAL_LEN];
    file.seek(SeekFrom::Start(local_at))?;
    file.read_exact(&mut local).ok().filter(|()| local.starts_with(b"PK\x03\x04")).context("Not a zip")?;
    let data_at = local_at + (LOCAL_LEN + u16_at(&local, 26) + u16_at(&local, 28)) as u64;
    if data_at.checked_add(packed).is_none_or(|to| to > len) {
        bail!("Not a zip");
    }
    file.seek(SeekFrom::Start(data_at))?;
    let data = file.take(packed);
    match method {
        0 => Ok(Box::new(data)),
        8 => Ok(Box::new(flate2::read::DeflateDecoder::new(data))),
        _ => bail!("Packed in a way not taken: zip it the usual way"),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::io::Write;

    use super::*;
    use crate::testing::TempDir;

    /// A zip of `files`: name, contents, method (0 stored, 8 deflated).
    pub(crate) fn zip_of(files: &[(&str, &[u8], u16)]) -> Vec<u8> {
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (name, plain, method) in files {
            let data = if *method == 8 {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(plain).unwrap();
                e.finish().unwrap()
            } else {
                plain.to_vec()
            };
            let at = out.len() as u32;
            let sizes = [(data.len() as u32).to_le_bytes(), (plain.len() as u32).to_le_bytes()].concat();
            let common = [&[20, 0, 0, 0][..], &method.to_le_bytes(), &[0; 8], &sizes, &(name.len() as u16).to_le_bytes(), &[0, 0]].concat();
            out.extend([b"PK\x03\x04", &common[..], name.as_bytes(), &data].concat());
            dir.extend([b"PK\x01\x02\x14\x00", &common[..], &[0; 10], &at.to_le_bytes(), name.as_bytes()].concat());
        }
        let n = (files.len() as u16).to_le_bytes();
        let end =
            [b"PK\x05\x06\0\0\0\0", &n[..], &n, &(dir.len() as u32).to_le_bytes(), &(out.len() as u32).to_le_bytes(), &[0, 0]].concat();
        [out, dir, end].concat()
    }

    fn unpacked(dir: &TempDir, packed: &[u8], max: u64) -> Result<Vec<u8>> {
        let part = dir.join("x.part");
        std::fs::write(&part, packed).unwrap();
        let was = unpack(&part, max)?;
        assert!(was && !part.with_extension("unpacked").exists());
        Ok(std::fs::read(&part)?)
    }

    #[test]
    fn a_zip_or_gzip_unpacks_to_its_demo_and_a_demo_is_left_as_it_is() {
        let dir = TempDir::new("unpack");
        let demo = [b"HL2DEMO\0".as_slice(), &[7; 5000]].concat();
        for method in [0, 8] {
            let zip = zip_of(&[("readme.txt", b"hi", 8), ("Match/Demo.DEM", &demo, method)]);
            assert_eq!(unpacked(&dir, &zip, 1 << 20).unwrap(), demo);
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&demo).unwrap();
        assert_eq!(unpacked(&dir, &gz.finish().unwrap(), 1 << 20).unwrap(), demo);
        let part = dir.join("plain.part");
        std::fs::write(&part, &demo).unwrap();
        assert!(!unpack(&part, 1 << 20).unwrap());
        assert_eq!(std::fs::read(&part).unwrap(), demo);
    }

    #[test]
    fn what_unpacks_to_too_much_or_is_no_single_demo_is_refused() {
        let dir = TempDir::new("unpack-refused");
        let why = |packed: &[u8], max| unpacked(&dir, packed, max).unwrap_err().to_string();
        let big = vec![0u8; 100_000];
        // A bomb, whatever it says of its size.
        let mut bomb = zip_of(&[("a.dem", &big, 8)]);
        assert!(bomb.len() < 1000);
        assert_eq!(why(&bomb, 50_000), "Over 0 MB unpacked");
        let says = bomb.windows(4).rposition(|w| w == 100_000u32.to_le_bytes()).unwrap();
        bomb[says..says + 4].copy_from_slice(&10u32.to_le_bytes());
        assert_eq!(why(&bomb, 50_000), "Over 0 MB unpacked", "it lies about its size");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&big).unwrap();
        assert_eq!(why(&gz.finish().unwrap(), 50_000), "Over 0 MB unpacked");
        assert_eq!(why(&zip_of(&[("a.txt", b"x", 0)]), 1 << 20), "No demo in it");
        assert_eq!(why(&zip_of(&[("a.dem", b"x", 0), ("b.dem", b"y", 0)]), 1 << 20), "More than one demo in it");
        assert_eq!(why(&zip_of(&[("a.dem", b"x", 12)]), 1 << 20), "Packed in a way not taken: zip it the usual way");
        let many: Vec<(String, &[u8], u16)> = (0..=MAX_ENTRIES).map(|n| (format!("{n}.txt"), &b"x"[..], 0)).collect();
        let many: Vec<(&str, &[u8], u16)> = many.iter().map(|(n, d, m)| (n.as_str(), *d, *m)).collect();
        assert_eq!(why(&zip_of(&many), 1 << 20), "Too many files in it");
        // Cut short: no zip.
        let whole = zip_of(&[("a.dem", &big, 8)]);
        assert_eq!(why(&whole[..whole.len() - 30], 1 << 20), "Not a zip");
        let mut encrypted = zip_of(&[("a.dem", b"x", 0)]);
        let flag = encrypted.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap() + 8;
        encrypted[flag] |= 1;
        assert_eq!(why(&encrypted, 1 << 20), "It is encrypted");
    }
}
