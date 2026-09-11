//! Miner submit body: JSON or multipart, with an optional artefact part.

use std::collections::BTreeMap;

use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Request};
use axum::http::{header, HeaderMap, StatusCode};
use serde_json::{Map, Value};

use proof_store::MAX_ARTEFACT_BYTES;
use proof_vm_proto::tar::{verify_artifact, TarError};
use sha2::{Digest, Sha256};

use super::{err, ErrResp, SubmitBody};

/// Multipart overhead budget on top of the 5 MiB artefact cap.
pub const SUBMIT_BODY_LIMIT: usize = MAX_ARTEFACT_BYTES + 512 * 1024;

/// Parsed submit: JSON fields plus optional uploaded tar bytes.
pub struct ParsedSubmit {
    /// Signed JSON fields (and unsigned `env`).
    pub body: SubmitBody,
    /// Uncompressed tar bytes from the `artifact` part, when present.
    pub artifact: Option<Vec<u8>>,
}

/// JSON (`application/json`) or multipart (`multipart/form-data`) submit.
pub async fn parse_submit(headers: &HeaderMap, req: Request) -> Result<ParsedSubmit, ErrResp> {
    let ctype = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");
    if ctype
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("multipart/form-data")
    {
        parse_multipart(req).await
    } else {
        parse_json(req).await
    }
}

async fn parse_json(req: Request) -> Result<ParsedSubmit, ErrResp> {
    let bytes = read_body(req).await?;
    let body = serde_json::from_slice(&bytes)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid submit JSON"))?;
    Ok(ParsedSubmit {
        body,
        artifact: None,
    })
}

async fn parse_multipart(req: Request) -> Result<ParsedSubmit, ErrResp> {
    let mut mp = Multipart::from_request(req, &())
        .await
        .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid multipart"))?;
    let mut json_part: Option<String> = None;
    let mut fields = BTreeMap::new();
    let mut artifact: Option<Vec<u8>> = None;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid multipart"))?
    {
        let name = field.name().unwrap_or("").to_owned();
        if name == "artifact" {
            let data = field
                .bytes()
                .await
                .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid artifact part"))?;
            if data.len() > MAX_ARTEFACT_BYTES {
                return Err(err(StatusCode::BAD_REQUEST, "artifact exceeds 5 MiB"));
            }
            artifact = Some(data.to_vec());
            continue;
        }
        let text = field
            .text()
            .await
            .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid multipart field"))?;
        if name == "json" {
            json_part = Some(text);
        } else if !name.is_empty() {
            fields.insert(name, text);
        }
    }
    let body = body_from_parts(json_part.as_deref(), &fields)?;
    Ok(ParsedSubmit { body, artifact })
}

async fn read_body(req: Request) -> Result<Vec<u8>, ErrResp> {
    let bytes = to_bytes(req.into_body(), SUBMIT_BODY_LIMIT)
        .await
        .map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                "request body exceeds 5 MiB artefact cap",
            )
        })?;
    Ok(bytes.to_vec())
}

fn body_from_parts(
    json: Option<&str>,
    fields: &BTreeMap<String, String>,
) -> Result<SubmitBody, ErrResp> {
    let mut map = Map::new();
    if let Some(raw) = json {
        let v: Value = serde_json::from_str(raw)
            .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid submit JSON"))?;
        if let Value::Object(o) = v {
            map = o;
        } else {
            return Err(err(StatusCode::BAD_REQUEST, "invalid submit JSON"));
        }
    }
    for (k, val) in fields {
        map.insert(k.clone(), field_value(k, val));
    }
    serde_json::from_value(Value::Object(map))
        .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid submit fields"))
}

fn field_value(name: &str, val: &str) -> Value {
    match name {
        "manifest" | "env" | "declared_flops" => {
            serde_json::from_str(val).unwrap_or_else(|_| Value::String(val.to_owned()))
        }
        _ => Value::String(val.to_owned()),
    }
}

/// SHA-256 hex of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Intake gates on uploaded artefact bytes: empty, oversize, digest-of-nothing,
/// then [`verify_artifact`] — uncompressed tar with file content whose SHA-256
/// is the signed `artifact_digest`. Gzip / non-tar / content-less / mismatch
/// are **400** with no row, before auth spends the nonce.
pub fn accept_uploaded(
    bytes: &[u8],
    claimed: &str,
    is_nothing: impl Fn(&str) -> bool,
) -> Result<(), ErrResp> {
    if bytes.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "artifact is empty"));
    }
    if bytes.len() > MAX_ARTEFACT_BYTES {
        return Err(err(StatusCode::BAD_REQUEST, "artifact exceeds 5 MiB"));
    }
    let got = sha256_hex(bytes);
    if is_nothing(&got) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "artifact_digest is the sha256 of empty input (or of an empty tar archive): hash the recipe bytes you upload (or ship at artifact_uri)",
        ));
    }
    match verify_artifact(bytes, claimed) {
        Ok(_) => Ok(()),
        Err(TarError::Gzip) => Err(err(
            StatusCode::BAD_REQUEST,
            "artifact is gzip-compressed; upload an uncompressed tar",
        )),
        Err(TarError::Malformed(_)) => Err(err(
            StatusCode::BAD_REQUEST,
            "artifact is not a tar archive",
        )),
        Err(TarError::NoContent) => Err(err(
            StatusCode::BAD_REQUEST,
            "artifact carries no file content",
        )),
        Err(TarError::Digest { .. }) => Err(err(
            StatusCode::BAD_REQUEST,
            "artifact_digest does not match uploaded bytes",
        )),
    }
}
