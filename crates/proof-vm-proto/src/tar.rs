//! Artefact identity on the topic-VM path
//! ([`crate::guest::SisterRequest::artifact_tar`]): **the bytes of the
//! recipe tar, verbatim**, hashed with SHA-256 (HTTP `artifact_uri` or a
//! vsock inject of a `proof-artefact://` upload).
//!
//! One identity, three places, one function — [`verify_artifact`]:
//!
//! 1. **Miner / submit.** `artifact_digest` is the SHA-256 of the exact file
//!    `artifact_uri` serves: an **uncompressed** ustar / GNU / pax tar of
//!    the recipe tree with at least one byte of file content
//!    (`tar -cf recipe.tar recipe/ && sha256sum recipe.tar`).
//! 2. **RLM guest.** Obtains the bytes — HTTP `artifact_uri` on the URI-only
//!    path, or a vsock [`crate::guest::HostToRlm::StageArtifact`] inject for
//!    `proof-artefact://` — runs [`verify_artifact`] on the bytes *as
//!    received* against the job's `artifact_digest`, inspects a copy of the
//!    tree, and forwards **those same bytes** as `artifact_tar`. It never
//!    re-tars: tar metadata and member order change under re-encoding even
//!    when every file is identical, so a re-tarred tree hashes differently
//!    and the host refuses it. A fetch or inject that fails or does not
//!    verify is a failed job (`RlmToHost::Failed`) — never a substitute
//!    archive.
//! 3. **KVM host.** Runs the same [`verify_artifact`] on `artifact_tar`
//!    against the paid job's digest before any sister jail is built; the
//!    sister unpacks the same bytes.
//!
//! A digest match alone does not say the bytes are a miner's work: a guest
//! whose fetch failed and that fell back to an empty tree hashes just as
//! consistently (staging matched exactly such an empty-file digest when the
//! RLM VM could not reach the artefact host). So [`require_content`] also
//! walks the tar: gzip, not a tar, an empty archive, or a tree of empty
//! files is refused with a reason, and the run comes back without a sister
//! attestation (the control plane then answers 503 for a
//! `firecracker_required` topic; nothing is scored).

use std::fmt;

use sha2::{Digest, Sha256};

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
    /// The bytes do not hash to the digest they are presented under: the
    /// archive was re-encoded (re-tarred) or is not the file the miner served.
    Digest {
        /// SHA-256 hex of the bytes as received.
        got: String,
        /// The digest the paid job names.
        want: String,
    },
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
            Self::Digest { got, want } => write!(
                f,
                "artifact_tar hashes to {got}, request says {want}: the guest must forward the bytes \
                 it fetched from artifact_uri verbatim (a re-tarred tree never matches), and the miner's \
                 artifact_digest must be the sha256 of the served file"
            ),
        }
    }
}

impl std::error::Error for TarError {}

/// The artefact identity check every side runs on the same bytes: an
/// uncompressed tar with file content whose SHA-256 is `artifact_digest`
/// (hex, case-insensitive, whitespace-trimmed). Returns the bytes of file
/// content the archive carries.
///
/// The RLM guest calls this on what it fetched from `artifact_uri` before
/// inspecting or forwarding anything; the KVM host calls it on
/// `SisterRequest::artifact_tar` before building a sister jail. Because the
/// guest forwards the fetched bytes verbatim, both calls see the same bytes
/// and the same digest — that is the contract.
///
/// # Errors
///
/// [`TarError::Gzip`] / [`TarError::Malformed`] / [`TarError::NoContent`]
/// for the shape, [`TarError::Digest`] when the bytes are not the file the
/// digest names.
pub fn verify_artifact(bytes: &[u8], artifact_digest: &str) -> Result<u64, TarError> {
    let content = require_content(bytes)?;
    let got = hex::encode(Sha256::digest(bytes));
    let want = artifact_digest.trim();
    if !got.eq_ignore_ascii_case(want) {
        return Err(TarError::Digest {
            got,
            want: want.to_owned(),
        });
    }
    Ok(content)
}

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

    /// The identity is the served file's bytes. The guest that forwards
    /// them verbatim verifies; a guest that re-tars the same tree — other
    /// member order, other mtime — produces different bytes and is refused
    /// by name, as is any digest that is not the sha256 of the bytes.
    #[test]
    fn identity_is_the_served_bytes_and_a_retar_never_matches() {
        let served = archive(&[
            member("recipe/", b'5', b""),
            member("recipe/run.sh", b'0', b"#!/bin/sh\necho hi\n"),
            member("recipe/model.bin", b'0', &[7u8; 700]),
        ]);
        let digest = hex::encode(Sha256::digest(&served));
        assert_eq!(
            verify_artifact(&served, &digest),
            Ok(718),
            "verbatim bytes verify"
        );
        assert_eq!(
            verify_artifact(&served, &format!(" {} ", digest.to_ascii_uppercase())),
            Ok(718),
            "hex case and whitespace do not matter"
        );
        // Same files, re-tarred: members in another order …
        let reordered = archive(&[
            member("recipe/", b'5', b""),
            member("recipe/model.bin", b'0', &[7u8; 700]),
            member("recipe/run.sh", b'0', b"#!/bin/sh\necho hi\n"),
        ]);
        // … or the same order with another mtime on one header.
        let mut restamped = served.clone();
        restamped[136..148].copy_from_slice(b"14700000000\0");
        let sum: u64 = restamped[..512]
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
        restamped[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        for (label, retar) in [("reordered", reordered), ("restamped", restamped)] {
            assert_eq!(
                regular_file_bytes(&retar),
                Ok(718),
                "{label}: identical file content"
            );
            let err = verify_artifact(&retar, &digest).expect_err(label);
            assert!(
                matches!(&err, TarError::Digest { want, .. } if *want == digest),
                "{label}: {err:?}"
            );
            let msg = err.to_string();
            assert!(
                msg.contains("hashes to") && msg.contains("verbatim"),
                "{msg}"
            );
            assert!(msg.contains("re-tarred tree never matches"), "{msg}");
        }
        // Shape errors come first: a hollow archive under a matching digest
        // is the fetch-fallback stub, not a digest problem.
        let hollow = empty_archive();
        assert_eq!(
            verify_artifact(&hollow, &hex::encode(Sha256::digest(&hollow))),
            Err(TarError::NoContent)
        );
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
