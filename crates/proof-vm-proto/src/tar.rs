//! Shape check of the artefact tarball a sister is asked to run
//! ([`crate::guest::SisterRequest::artifact_tar`]).
//!
//! The RLM guest fetches the miner's artefact from `artifact_uri`, tars the
//! tree, and ships the bytes over vsock. The host re-hashes them against the
//! paid job's digest — but a digest match alone does not say the bytes are a
//! miner's work: a guest whose fetch failed and that fell back to an empty
//! tree hashes just as consistently (staging matched exactly such an
//! empty-file digest when the RLM VM could not reach the artefact host). So
//! before any jail is built the host also walks the tar with
//! [`require_content`]: it must be an **uncompressed** ustar / GNU / pax
//! archive whose regular files carry at least one byte. Anything else —
//! gzip, not a tar, an empty archive, a tree of empty files — is refused
//! with a reason, and the run comes back without a sister attestation (the
//! control plane then answers 503 for a `firecracker_required` topic;
//! nothing is scored). Guest images implement this contract: never
//! substitute bytes when the fetch fails — answer the job `Failed`.

use std::fmt;

/// 512-byte tar block.
const BLOCK: usize = 512;

/// Why the bytes are not an artefact the host will run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TarError {
    /// gzip magic: the guest must send an uncompressed tar.
    Gzip,
    /// Not a tar archive (bad header checksum / size field / truncated).
    Malformed(&'static str),
    /// A well-formed archive whose regular files carry zero bytes.
    NoContent,
}

impl fmt::Display for TarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gzip => f.write_str(
                "artifact_tar is gzip-compressed; the guest must send an uncompressed tar",
            ),
            Self::Malformed(why) => write!(f, "artifact_tar is not a tar archive ({why})"),
            Self::NoContent => f.write_str(
                "artifact_tar carries no file content: an empty tree is not a miner artefact \
                 (a guest that substitutes bytes when its fetch fails is refused here)",
            ),
        }
    }
}

impl std::error::Error for TarError {}

/// Octal ASCII field (NUL / space terminated), or GNU base-256 when the
/// first byte has its high bit set.
fn numeric(field: &[u8]) -> Option<u64> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        let mut v: u64 = u64::from(field[0] & 0x7f);
        for b in &field[1..] {
            v = v.checked_mul(256)?.checked_add(u64::from(*b))?;
        }
        return Some(v);
    }
    let mut v: u64 = 0;
    let mut seen = false;
    for b in field {
        match b {
            b'0'..=b'7' => {
                v = v.checked_mul(8)?.checked_add(u64::from(b - b'0'))?;
                seen = true;
            }
            b' ' if !seen => {}
            b' ' | 0 => break,
            _ => return None,
        }
    }
    seen.then_some(v)
}

/// Header checksum: every byte summed with the checksum field read as spaces.
fn checksum_ok(header: &[u8]) -> bool {
    let Some(stored) = numeric(&header[148..156]) else {
        return false;
    };
    let sum: u64 = header
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if (148..156).contains(&i) {
                u64::from(b' ')
            } else {
                u64::from(*b)
            }
        })
        .sum();
    sum == stored
}

/// Bytes carried by regular files in `tar`.
///
/// Walks every header (checksum verified), skips meta entries (GNU long
/// name / link, pax extended / global headers) and non-file members, and
/// sums the sizes of regular files. Two zero blocks — or the end of the
/// bytes — end the archive.
///
/// # Errors
///
/// [`TarError::Gzip`], [`TarError::Malformed`]; never [`TarError::NoContent`]
/// (see [`require_content`]).
pub fn regular_file_bytes(tar: &[u8]) -> Result<u64, TarError> {
    if tar.starts_with(&[0x1f, 0x8b]) {
        return Err(TarError::Gzip);
    }
    if tar.len() < BLOCK {
        return Err(TarError::Malformed("shorter than one header block"));
    }
    let mut total: u64 = 0;
    let mut offset = 0usize;
    let mut members = 0usize;
    while offset + BLOCK <= tar.len() {
        let header = &tar[offset..offset + BLOCK];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        if !checksum_ok(header) {
            return Err(TarError::Malformed("header checksum"));
        }
        let size = numeric(&header[124..136]).ok_or(TarError::Malformed("size field"))?;
        let blocks = size.div_ceil(BLOCK as u64);
        let data_len = usize::try_from(blocks.saturating_mul(BLOCK as u64))
            .map_err(|_| TarError::Malformed("size field"))?;
        let next = offset
            .checked_add(BLOCK)
            .and_then(|o| o.checked_add(data_len))
            .ok_or(TarError::Malformed("size field"))?;
        if next > tar.len() {
            return Err(TarError::Malformed("member data runs past the end"));
        }
        members += 1;
        // typeflag: '0' / NUL / '7' are file data; 'L' 'K' 'x' 'g' are
        // metadata for the next member; everything else carries no payload.
        if matches!(header[156], b'0' | 0 | b'7') {
            total = total.saturating_add(size);
        }
        offset = next;
    }
    if members == 0 {
        return Ok(0);
    }
    Ok(total)
}

/// [`regular_file_bytes`], refusing an archive that carries none.
///
/// # Errors
///
/// Every [`TarError`].
pub fn require_content(tar: &[u8]) -> Result<u64, TarError> {
    match regular_file_bytes(tar)? {
        0 => Err(TarError::NoContent),
        n => Ok(n),
    }
}

/// Hand-built ustar archives for tests (`test-fixtures` feature): no `tar`
/// binary, no filesystem.
#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixtures {
    use super::BLOCK;

    /// One ustar member: header block + data padded to whole blocks.
    #[must_use]
    pub fn member(name: &str, typeflag: u8, data: &[u8]) -> Vec<u8> {
        let mut h = vec![0u8; BLOCK];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000644\0");
        h[108..116].copy_from_slice(b"0000000\0");
        h[116..124].copy_from_slice(b"0000000\0");
        let size = format!("{:011o}\0", data.len());
        h[124..136].copy_from_slice(size.as_bytes());
        h[136..148].copy_from_slice(b"00000000000\0");
        h[156] = typeflag;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u64 = h.iter().map(|b| u64::from(*b)).sum();
        let chk = format!("{sum:06o}\0 ");
        h[148..156].copy_from_slice(chk.as_bytes());
        let mut out = h;
        out.extend_from_slice(data);
        let pad = (BLOCK - data.len() % BLOCK) % BLOCK;
        out.extend(std::iter::repeat_n(0u8, pad));
        out
    }

    /// Members followed by the two end-of-archive zero blocks, padded to
    /// the 10 KiB record GNU tar writes.
    #[must_use]
    pub fn archive(members: &[Vec<u8>]) -> Vec<u8> {
        let mut out: Vec<u8> = members.concat();
        out.extend(std::iter::repeat_n(0u8, 2 * BLOCK));
        let pad = (20 * BLOCK - out.len() % (20 * BLOCK)) % (20 * BLOCK);
        out.extend(std::iter::repeat_n(0u8, pad));
        out
    }

    /// `tar cf empty.tar -T /dev/null`: 10240 zero bytes.
    #[must_use]
    pub fn empty_archive() -> Vec<u8> {
        vec![0u8; 20 * BLOCK]
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{archive, empty_archive, member};
    use super::*;

    #[test]
    fn an_empty_archive_or_a_tree_of_empty_files_is_no_artefact() {
        assert_eq!(regular_file_bytes(&empty_archive()), Ok(0));
        assert_eq!(require_content(&empty_archive()), Err(TarError::NoContent));
        let empties = archive(&[
            member("recipe/", b'5', b""),
            member("recipe/run.sh", b'0', b""),
            member("recipe/README", b'0', b""),
        ]);
        assert_eq!(require_content(&empties), Err(TarError::NoContent));
        let msg = TarError::NoContent.to_string();
        assert!(
            msg.contains("no file content") && msg.contains("fetch fails"),
            "{msg}"
        );
    }

    #[test]
    fn real_content_counts_and_metadata_members_do_not() {
        let tree = archive(&[
            member("recipe/", b'5', b""),
            member("recipe/run.sh", b'0', b"#!/bin/sh\necho hi\n"),
            member("recipe/model.bin", 0, &[7u8; 700]),
            member("recipe/link", b'2', b""),
        ]);
        assert_eq!(regular_file_bytes(&tree), Ok(18 + 700));
        assert_eq!(require_content(&tree), Ok(718));
        // GNU long name + pax headers carry data blocks that are not payload.
        let long = "recipe/".to_owned() + &"n".repeat(150);
        let meta = archive(&[
            member("././@LongLink", b'L', long.as_bytes()),
            member(&long[..99], b'0', b"x"),
            member("./PaxHeaders/x", b'x', b"30 mtime=1700000000.123456789\n"),
            member("recipe/other", b'0', b"yz"),
        ]);
        assert_eq!(require_content(&meta), Ok(3));
        // Trailing end-of-archive blocks may be absent (a bare member list).
        let bare = member("a", b'0', b"abc");
        assert_eq!(require_content(&bare), Ok(3));
    }

    #[test]
    fn gzip_garbage_and_truncation_are_refused_by_name() {
        assert_eq!(
            regular_file_bytes(&[0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0]),
            Err(TarError::Gzip)
        );
        assert_eq!(
            regular_file_bytes(b"not a tar"),
            Err(TarError::Malformed("shorter than one header block"))
        );
        let junk = vec![0x41u8; 1024];
        assert_eq!(
            regular_file_bytes(&junk),
            Err(TarError::Malformed("header checksum"))
        );
        let mut truncated = member("recipe/big", b'0', &[1u8; 2000]);
        truncated.truncate(BLOCK + 512);
        assert_eq!(
            regular_file_bytes(&truncated),
            Err(TarError::Malformed("member data runs past the end"))
        );
        let mut bad_size = member("recipe/x", b'0', b"abc");
        bad_size[124..136].copy_from_slice(b"zzzzzzzzzzz\0");
        // The checksum no longer matches either; whichever fires, it is Malformed.
        assert!(matches!(
            regular_file_bytes(&bad_size),
            Err(TarError::Malformed(_))
        ));
        assert_eq!(numeric(b"0000644\0"), Some(0o644));
        assert_eq!(numeric(b"   17 "), Some(0o17));
        assert_eq!(numeric(&[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0]), Some(256));
        assert_eq!(numeric(b"8"), None);
        assert_eq!(numeric(b"        "), None);
    }
}
