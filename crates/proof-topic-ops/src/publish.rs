//! Publishing a signed document through the existing admin route.
//!
//! `proof-admin topic install` and `topic seal` both end here: the document is
//! already validated and signed, and this is the one HTTP call that makes it
//! reachable. The route is `POST /v1/admin/proof/topics`
//! (`proof_topic_bundle::PUBLISH_PATH`), and the bearer is read from a file —
//! never printed, never logged, never an argument.
//!
//! # Why the bearer is a file
//!
//! A token on a command line is a token in the shell history and in `ps`. The
//! file is read here and held in this struct; [`PublishTarget::redacted`] is
//! what an operator sees when the call is reported.

use std::path::Path;

/// Where the admin publish call goes, and the bearer it uses.
pub struct PublishTarget {
    base_url: String,
    token: String,
}

impl PublishTarget {
    /// Resolve the URL and bearer, refusing a half-configured pair.
    pub fn resolve(
        admin_url: Option<&str>,
        admin_token_file: Option<&Path>,
    ) -> Result<Self, crate::OpsError> {
        let Some(base_url) = admin_url.map(str::trim).filter(|u| !u.is_empty()) else {
            return Err(crate::OpsError::usage(
                "a real install publishes through the admin route, so it needs the master's \
                 base URL: pass --admin-url (or set PROOF_ADMIN_URL), e.g. \
                 --admin-url http://127.0.0.1:8100 for the challenge service directly, or the \
                 gateway's address. `--dry-run` needs none."
                    .to_owned(),
            ));
        };
        let Some(path) = admin_token_file else {
            return Err(crate::OpsError::usage(
                "a real install needs the operator bearer for /v1/admin/*: pass \
                 --admin-token-file (or set PROOF_ADMIN_TOKEN_FILE). The file is read and never \
                 logged or printed. `--dry-run` needs none."
                    .to_owned(),
            ));
        };
        let token = std::fs::read_to_string(path)
            .map_err(|e| crate::OpsError::error(format!("read {}: {e}", path.display())))?;
        // A tokens file holds one bearer per line; the first non-comment line
        // is the one this call uses.
        let token = token
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_owned);
        let Some(token) = token else {
            return Err(crate::OpsError::error(format!(
                "{} holds no bearer (every line is blank or a comment)",
                path.display()
            )));
        };
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// How this target is printed: the URL, never the bearer.
    #[must_use]
    pub fn redacted(&self) -> String {
        format!("{} (bearer read, never printed)", self.base_url)
    }

    /// Publish the document through the existing admin route.
    pub async fn publish(&self, doc: &proof_task::TopicDocument) -> Result<(), String> {
        let url = format!("{}{}", self.base_url, proof_topic_bundle::PUBLISH_PATH);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_mins(1))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let response = client
            .post(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .body(
                serde_json::to_string(doc)
                    .map_err(|e| format!("serialize the signed document: {e}"))?,
            )
            .send()
            .await
            .map_err(|e| format!("POST {url}: {e}"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(format!(
            "POST {url} answered {status}: {}",
            body.trim().chars().take(400).collect::<String>()
        ))
    }
}
