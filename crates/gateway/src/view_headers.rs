//! Viewer response-header floor for `/challenge/*/v1/view/*`.
//!
//! Defense in depth at the last serving layer: CSP sandbox for non-PNG
//! documents, CORP cross-origin for public PNG screenshots.

/// Default `frame-ancestors` allowlist for the viewer CSP: the public site,
/// Vercel preview deploys (staging frontend), and local dev servers.
#[must_use]
pub fn default_frame_ancestors() -> &'static str {
    "'self' https://joinbase.ai https://*.vercel.app http://localhost:*"
}

/// Viewer response headers for **non-PNG** `/v1/view/*` responses.
#[must_use]
pub fn viewer_headers(frame_ancestors: &str) -> Vec<(&'static str, String)> {
    let csp = format!(
        "sandbox; default-src 'none'; img-src data: https:; style-src 'unsafe-inline' https:; \
         font-src data: https:; base-uri 'none'; form-action 'none'; frame-ancestors {frame_ancestors}"
    );
    vec![
        ("Content-Security-Policy", csp),
        ("X-Content-Type-Options", "nosniff".into()),
        ("Referrer-Policy", "no-referrer".into()),
        ("Cross-Origin-Resource-Policy", "same-origin".into()),
        ("Cross-Origin-Opener-Policy", "same-origin".into()),
        (
            "Permissions-Policy",
            "accelerometer=(), camera=(), geolocation=(), gyroscope=(), magnetometer=(), \
             microphone=(), payment=(), usb=()"
                .into(),
        ),
        ("Cache-Control", "private, no-store".into()),
    ]
}

/// Headers for public PNG screenshots (`index.png`).
///
/// `Cross-Origin-Resource-Policy: cross-origin` lets marketing/admin UIs load
/// the image with a direct absolute URL to the gateway.
#[must_use]
pub fn screenshot_headers() -> Vec<(&'static str, String)> {
    vec![
        ("X-Content-Type-Options", "nosniff".into()),
        ("Referrer-Policy", "no-referrer".into()),
        ("Cross-Origin-Resource-Policy", "cross-origin".into()),
        ("Cache-Control", "private, no-store".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_is_cross_origin() {
        let h = screenshot_headers();
        assert!(h
            .iter()
            .any(|(k, v)| *k == "Cross-Origin-Resource-Policy" && v == "cross-origin"));
    }

    #[test]
    fn viewer_sandboxes_scripts() {
        let h = viewer_headers(default_frame_ancestors());
        let csp = h
            .iter()
            .find(|(k, _)| *k == "Content-Security-Policy")
            .map_or("", |(_, v)| v.as_str());
        assert!(csp.starts_with("sandbox;"));
        assert!(!csp.contains("allow-scripts"));
    }
}
