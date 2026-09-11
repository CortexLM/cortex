//! The miner artefact, fetched **inside** the VM and judged on bytes: the
//! file served at `artifact_uri` must hash to the request's
//! `artifact_digest` and be an uncompressed tar with content
//! (`proof_vm_proto::tar::verify_artifact`). Anything else is a failed job —
//! never a substitute tree, never a re-tar.

use std::path::{Path, PathBuf};
use std::time::Duration;

use proof_rlm::{is_staged_artifact_uri, CustomRunRequest};
use proof_vm_proto::tar::verify_artifact;

use crate::staging::{unpack_tar, StagedArtifact};

/// Largest artefact the guest fetches (matches the host's relay cap).
pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
/// Wall-clock for one fetch.
pub const FETCH_TIMEOUT: Duration = Duration::from_mins(5);
/// The verbatim bytes are kept beside the tree for adaptors that forward them.
pub const ARTIFACT_TAR_NAME: &str = "artifact.tar";
/// Unpacked tree directory name under the job's work dir.
pub const ARTIFACT_DIR_NAME: &str = "artifact";

fn scheme_ok(uri: &str, allow_plain_http: bool) -> Result<(), String> {
    if is_staged_artifact_uri(uri) {
        return Err(format!(
            "artifact_uri {uri:?} is a staged-vault locator; the host must inject the bytes over vsock (never HTTP-fetch this scheme)"
        ));
    }
    if uri.starts_with("https://") || (allow_plain_http && uri.starts_with("http://")) {
        Ok(())
    } else {
        Err(format!(
            "artifact_uri {uri:?} must be https:// (plain http only when the guest allows it)"
        ))
    }
}

/// Fetch, verify, and unpack the request's artefact under `work`. `Ok(None)`
/// when the request carries no locator (a baseline with no artefact).
///
/// A `proof-artefact://` locator uses `injected` (host vsock) and never
/// HTTP. A miner-hosted `https://` URI still GETs. Injected bytes that do
/// not match the job digest, or a staged locator with no inject, fail closed.
pub async fn fetch_artifact(
    request: &CustomRunRequest,
    work: &Path,
    allow_plain_http: bool,
    injected: Option<&StagedArtifact>,
) -> Result<Option<PathBuf>, String> {
    let Some(uri) = request
        .artifact_uri
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    else {
        return Ok(None);
    };
    let bytes = if is_staged_artifact_uri(uri) {
        let art = crate::staging::artifact_for(injected, &request.artifact_digest)?;
        art.bytes
    } else {
        scheme_ok(uri, allow_plain_http)?;
        download(uri).await?
    };
    verify_artifact(&bytes, &request.artifact_digest)
        .map_err(|e| format!("artifact fetch from {uri}: {e}; refusing to run a substitute"))?;
    std::fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;
    std::fs::write(work.join(ARTIFACT_TAR_NAME), &bytes)
        .map_err(|e| format!("keep artifact bytes: {e}"))?;
    let dir = work.join(ARTIFACT_DIR_NAME);
    unpack_tar(&bytes, &dir)?;
    Ok(Some(dir))
}

/// GET `uri` under [`MAX_ARTIFACT_BYTES`] and [`FETCH_TIMEOUT`]; the bytes
/// as served. See [`download_capped`].
pub async fn download(uri: &str) -> Result<Vec<u8>, String> {
    download_capped(uri, MAX_ARTIFACT_BYTES).await
}

/// GET `uri`, **streaming** the body and stopping the moment more than
/// `max` bytes have arrived — the body is never buffered whole before the
/// cap is applied, so a server that omits `Content-Length` (or lies about
/// it) and streams an arbitrarily long body cannot exhaust the guest's
/// memory: the fetch aborts at `max`, the connection is dropped, and the
/// job fails. An honest `Content-Length` over the cap is refused before the
/// first body byte.
pub async fn download_capped(uri: &str, max: usize) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let mut resp = client
        .get(uri)
        .send()
        .await
        .map_err(|e| format!("artifact fetch failed: {uri}: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!(
            "artifact fetch failed: {uri}: HTTP {}",
            resp.status()
        ));
    }
    let too_large =
        |got: usize| format!("artifact at {uri} is larger than {max} bytes (aborted after {got})");
    let declared = resp.content_length();
    if declared.is_some_and(|n| n > max as u64) {
        return Err(too_large(0));
    }
    // Reserve at most what the cap allows, whatever the header claims.
    let reserve = declared
        .and_then(|n| usize::try_from(n).ok())
        .map_or(0, |n| n.min(max));
    let mut bytes = Vec::with_capacity(reserve);
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("artifact fetch failed: {uri}: {}", e.without_url()))?
    {
        if bytes.len().saturating_add(chunk.len()) > max {
            return Err(too_large(bytes.len().saturating_add(chunk.len())));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn only_https_by_default_and_no_locator_is_no_artifact() {
        scheme_ok("https://artefacts.example.test/recipe.tar", false).expect("https");
        assert!(scheme_ok("http://10.0.0.5/recipe.tar", false).is_err());
        scheme_ok("http://10.0.0.5/recipe.tar", true).expect("staging opt-in");
        assert!(scheme_ok("ftp://x/recipe.tar", true).is_err());
        assert!(scheme_ok("file:///etc/passwd", true).is_err());
        assert!(
            scheme_ok(&format!("proof-artefact://{}", "ab".repeat(32)), true)
                .expect_err("staged scheme is never fetched")
                .contains("must inject"),
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        let mut req = proof_rlm::fixtures::request();
        req.artifact_uri = None;
        let work =
            std::env::temp_dir().join(format!("proof-vm-guest-fetch-{}", std::process::id()));
        assert_eq!(
            rt.block_on(fetch_artifact(&req, &work, false, None))
                .expect("no locator"),
            None
        );
        req.artifact_uri = Some("http://127.0.0.1:9/recipe.tar".into());
        let err = rt
            .block_on(fetch_artifact(&req, &work, false, None))
            .expect_err("plain http refused before any request");
        assert!(err.contains("must be https://"), "{err}");
        assert_eq!(MAX_ARTIFACT_BYTES, 64 * 1024 * 1024);

        let tar = proof_vm_proto::tar::fixtures::archive(&[proof_vm_proto::tar::fixtures::member(
            "recipe/run.sh",
            b'0',
            b"echo hi\n",
        )]);
        let digest = hex::encode(sha2::Sha256::digest(&tar));
        let uri = format!("proof-artefact://{digest}");
        req.artifact_digest = digest.clone();
        req.artifact_uri = Some(uri);
        let err = rt
            .block_on(fetch_artifact(&req, &work, true, None))
            .expect_err("staged locator without inject");
        assert!(err.contains("needs a host inject"), "{err}");
        let injected = crate::staging::stage_artifact(
            &digest,
            &proof_vm_proto::guest::StagedFile::new("artifact.tar", &tar),
        )
        .expect("inject");
        let dir = rt
            .block_on(fetch_artifact(&req, &work, true, Some(&injected)))
            .expect("injected")
            .expect("unpacked");
        assert!(dir.join("recipe/run.sh").is_file());
        let _ = std::fs::remove_dir_all(&work);
    }

    /// Serve one chunked-encoding response with no `Content-Length`: `body`
    /// chunks of `chunk` bytes, then either a terminating chunk (`finite`)
    /// or more chunks forever, until the client hangs up. Returns the URL
    /// and a counter of bytes the server managed to write.
    async fn serve_chunked(
        chunk: usize,
        finite: Option<usize>,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let written = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = written.clone();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf).await;
            let head = "HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            if s.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            let payload = vec![b'x'; chunk];
            let frame = format!("{chunk:x}\r\n");
            let mut sent = 0usize;
            loop {
                if finite.is_some_and(|n| sent >= n) {
                    let _ = s.write_all(b"0\r\n\r\n").await;
                    let _ = s.shutdown().await;
                    return;
                }
                if s.write_all(frame.as_bytes()).await.is_err()
                    || s.write_all(&payload).await.is_err()
                    || s.write_all(b"\r\n").await.is_err()
                {
                    return;
                }
                sent += chunk;
                counter.fetch_add(chunk, Ordering::SeqCst);
            }
        });
        (format!("http://{addr}/recipe.tar"), written)
    }

    /// A body with no `Content-Length` is read **incrementally**: a server
    /// that streams past the cap is cut off at the cap (promptly — the old
    /// `bytes()` path would have buffered until the 5-minute timeout or the
    /// host ran out of memory), while a chunked body under the cap arrives
    /// whole. The whole-body cap is what the guest fetches under.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn streaming_bodies_are_cut_at_the_cap_not_buffered_first() {
        let cap = 256 * 1024;
        let (endless, _written) = serve_chunked(16 * 1024, None).await;
        let started = std::time::Instant::now();
        let err = download_capped(&endless, cap)
            .await
            .expect_err("an endless body never fits");
        assert!(
            err.contains(&format!("larger than {cap} bytes")),
            "names the cap: {err}"
        );
        assert!(err.contains("aborted after"), "{err}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "cut while streaming, not after a timeout: {:?}",
            started.elapsed()
        );

        let (small, _) = serve_chunked(1_000, Some(3_000)).await;
        let bytes = download_capped(&small, cap).await.expect("under the cap");
        assert_eq!(bytes.len(), 3_000);
        assert!(bytes.iter().all(|b| *b == b'x'));
        let (exact, _) = serve_chunked(1_024, Some(cap)).await;
        assert_eq!(
            download_capped(&exact, cap)
                .await
                .expect("at the cap")
                .len(),
            cap
        );
        let (over, _) = serve_chunked(1_024, Some(cap + 1_024)).await;
        assert!(download_capped(&over, cap).await.is_err());
    }
}
