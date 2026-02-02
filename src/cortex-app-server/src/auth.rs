//! Authentication and authorization.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::AuthConfig;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// JWT claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID).
    pub sub: String,
    /// Expiration time (Unix timestamp).
    pub exp: u64,
    /// Issued at (Unix timestamp).
    pub iat: u64,
    /// Issuer.
    pub iss: String,
    /// Audience.
    #[serde(default)]
    pub aud: Vec<String>,
    /// User roles.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Additional metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Claims {
    /// Create new claims for a user.
    pub fn new(user_id: impl Into<String>, expiry_seconds: u64) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            sub: user_id.into(),
            exp: now + expiry_seconds,
            iat: now,
            iss: "Cortex".to_string(),
            aud: vec!["cortex-api".to_string()],
            roles: vec![],
            metadata: HashMap::new(),
        }
    }

    /// Add a role to the claims.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.roles.push(role.into());
        self
    }

    /// Add metadata to the claims.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Check if the token is expired.
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.exp < now
    }

    /// Check if the user has a specific role.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Check if the user has any of the specified roles.
    pub fn has_any_role(&self, roles: &[&str]) -> bool {
        roles.iter().any(|r| self.has_role(r))
    }
}

/// Authentication service.
pub struct AuthService {
    /// Configuration.
    config: AuthConfig,
    /// Encoding key for JWT.
    encoding_key: Option<EncodingKey>,
    /// Decoding key for JWT.
    decoding_key: Option<DecodingKey>,
    /// API key hash cache.
    _api_key_hashes: RwLock<HashMap<String, String>>,
    /// Revoked tokens.
    revoked_tokens: RwLock<HashMap<String, u64>>,
}

impl std::fmt::Debug for AuthService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthService")
            .field("config", &self.config)
            .field("encoding_key", &self.encoding_key.is_some())
            .field("decoding_key", &self.decoding_key.is_some())
            .finish()
    }
}

impl AuthService {
    /// Create a new authentication service.
    pub fn new(config: AuthConfig) -> Self {
        let (encoding_key, decoding_key) = config
            .jwt_secret
            .as_ref()
            .map(|secret| {
                (
                    EncodingKey::from_secret(secret.as_bytes()),
                    DecodingKey::from_secret(secret.as_bytes()),
                )
            })
            .map(|(e, d)| (Some(e), Some(d)))
            .unwrap_or((None, None));

        Self {
            config,
            encoding_key,
            decoding_key,
            _api_key_hashes: RwLock::new(HashMap::new()),
            revoked_tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Generate a JWT token for a user.
    pub fn generate_token(&self, user_id: &str) -> AppResult<String> {
        let encoding_key = self
            .encoding_key
            .as_ref()
            .ok_or_else(|| AppError::Internal("JWT secret not configured".to_string()))?;

        let claims = Claims::new(user_id, self.config.jwt_expiry);

        encode(&Header::default(), &claims, encoding_key)
            .map_err(|e| AppError::Internal(format!("Failed to generate token: {e}")))
    }

    /// Validate a JWT token.
    pub fn validate_token(&self, token: &str) -> AppResult<Claims> {
        let decoding_key = self
            .decoding_key
            .as_ref()
            .ok_or_else(|| AppError::Internal("JWT secret not configured".to_string()))?;

        let validation = Validation::default();

        let token_data = decode::<Claims>(token, decoding_key, &validation)
            .map_err(|e| AppError::Authentication(format!("Invalid token: {e}")))?;

        if token_data.claims.is_expired() {
            return Err(AppError::Authentication("Token expired".to_string()));
        }

        Ok(token_data.claims)
    }

    /// Check if a token is revoked.
    pub async fn is_token_revoked(&self, token: &str) -> bool {
        let revoked = self.revoked_tokens.read().await;
        revoked.contains_key(token)
    }

    /// Revoke a token.
    pub async fn revoke_token(&self, token: &str, expiry: u64) {
        let mut revoked = self.revoked_tokens.write().await;
        revoked.insert(token.to_string(), expiry);
    }

    /// Clean up expired revoked tokens.
    pub async fn cleanup_revoked_tokens(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut revoked = self.revoked_tokens.write().await;
        revoked.retain(|_, expiry| *expiry > now);
    }

    /// Validate an API key.
    pub fn validate_api_key(&self, api_key: &str) -> bool {
        self.config.api_keys.iter().any(|k| k == api_key)
    }

    /// Hash an API key for storage.
    pub fn hash_api_key(api_key: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        api_key.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

/// Authentication result.
#[derive(Debug, Clone)]
pub enum AuthResult {
    /// Authenticated with JWT.
    Jwt(Claims),
    /// Authenticated with API key.
    ApiKey(String),
    /// Anonymous access.
    Anonymous,
}

impl AuthResult {
    /// Get the user ID if authenticated.
    pub fn user_id(&self) -> Option<&str> {
        match self {
            Self::Jwt(claims) => Some(&claims.sub),
            Self::ApiKey(key) => Some(key),
            Self::Anonymous => None,
        }
    }

    /// Check if the user is authenticated.
    pub fn is_authenticated(&self) -> bool {
        !matches!(self, Self::Anonymous)
    }
}

/// Extract authorization from request headers.
pub fn extract_auth_header(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(std::string::ToString::to_string)
}

/// Parse Bearer token from Authorization header.
pub fn parse_bearer_token(auth_header: &str) -> Option<&str> {
    auth_header
        .strip_prefix("Bearer ")
        .or_else(|| auth_header.strip_prefix("bearer "))
}

/// Parse API key from Authorization header.
pub fn parse_api_key(auth_header: &str) -> Option<&str> {
    auth_header
        .strip_prefix("ApiKey ")
        .or_else(|| auth_header.strip_prefix("apikey "))
}

/// Authentication middleware with proper JWT validation.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip auth if disabled
    if !state.config.auth.enabled {
        return Ok(next.run(request).await);
    }

    // Check if path is in anonymous list
    let path = request.uri().path();
    if state
        .config
        .auth
        .anonymous_endpoints
        .iter()
        .any(|p| path.starts_with(p))
    {
        return Ok(next.run(request).await);
    }

    // Try to authenticate
    let auth_header = extract_auth_header(request.headers());

    let auth_result = match auth_header.as_deref() {
        Some(header) if header.starts_with("Bearer ") || header.starts_with("bearer ") => {
            let token = parse_bearer_token(header).ok_or(StatusCode::UNAUTHORIZED)?;

            // Validate JWT token properly
            let decoding_key = state
                .config
                .auth
                .jwt_secret
                .as_ref()
                .map(|secret| DecodingKey::from_secret(secret.as_bytes()))
                .ok_or_else(|| {
                    tracing::error!("JWT secret not configured but auth is enabled");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;

            // Configure validation
            let mut validation = Validation::default();
            validation.set_issuer(&["Cortex"]);
            validation.set_audience(&["cortex-api"]);
            validation.validate_exp = true;
            validation.validate_nbf = false;

            // Decode and validate the token
            let token_data = decode::<Claims>(token, &decoding_key, &validation).map_err(|e| {
                tracing::warn!("JWT validation failed: {}", e);
                StatusCode::UNAUTHORIZED
            })?;

            let claims = token_data.claims;

            // Check if token is expired (extra safety check)
            if claims.is_expired() {
                tracing::warn!("JWT token expired for user: {}", claims.sub);
                return Err(StatusCode::UNAUTHORIZED);
            }

            AuthResult::Jwt(claims)
        }
        Some(header) if header.starts_with("ApiKey ") || header.starts_with("apikey ") => {
            let api_key = parse_api_key(header).ok_or(StatusCode::UNAUTHORIZED)?;

            // Validate API key against configured keys
            // Use constant-time comparison to prevent timing attacks
            let is_valid = state.config.auth.api_keys.iter().any(|configured_key| {
                constant_time_compare(api_key.as_bytes(), configured_key.as_bytes())
            });

            if is_valid {
                AuthResult::ApiKey(api_key.to_string())
            } else {
                tracing::warn!("Invalid API key attempted");
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        _ => {
            tracing::debug!("No valid authorization header provided");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Add auth result to request extensions
    request.extensions_mut().insert(auth_result);

    Ok(next.run(request).await)
}

/// Constant-time string comparison to prevent timing attacks.
fn constant_time_compare(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

/// Role-based access control.
pub struct RoleGuard {
    required_roles: Vec<String>,
    require_all: bool,
}

impl RoleGuard {
    /// Create a new role guard requiring any of the specified roles.
    pub fn any_of(roles: &[&str]) -> Self {
        Self {
            required_roles: roles.iter().map(std::string::ToString::to_string).collect(),
            require_all: false,
        }
    }

    /// Create a new role guard requiring all of the specified roles.
    pub fn all_of(roles: &[&str]) -> Self {
        Self {
            required_roles: roles.iter().map(std::string::ToString::to_string).collect(),
            require_all: true,
        }
    }

    /// Check if the claims satisfy the role requirements.
    pub fn check(&self, claims: &Claims) -> bool {
        if self.require_all {
            self.required_roles.iter().all(|r| claims.has_role(r))
        } else {
            self.required_roles.iter().any(|r| claims.has_role(r))
        }
    }
}

/// User information extracted from authentication.
#[derive(Debug, Clone, Serialize)]
pub struct User {
    /// User ID.
    pub id: String,
    /// User email (if available).
    pub email: Option<String>,
    /// User name (if available).
    pub name: Option<String>,
    /// User roles.
    pub roles: Vec<String>,
    /// Authentication method.
    pub auth_method: String,
}

impl From<Claims> for User {
    fn from(claims: Claims) -> Self {
        Self {
            id: claims.sub,
            email: claims
                .metadata
                .get("email")
                .and_then(|v| v.as_str().map(String::from)),
            name: claims
                .metadata
                .get("name")
                .and_then(|v| v.as_str().map(String::from)),
            roles: claims.roles,
            auth_method: "jwt".to_string(),
        }
    }
}

/// API key information.
#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyInfo {
    /// Key ID (hash of the key).
    pub id: String,
    /// Key name/label.
    pub name: Option<String>,
    /// Creation time.
    pub created_at: u64,
    /// Last used time.
    pub last_used: Option<u64>,
    /// Scopes/permissions.
    pub scopes: Vec<String>,
    /// Rate limit tier.
    pub rate_limit_tier: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claims_creation() {
        let claims = Claims::new("user123", 3600);
        assert_eq!(claims.sub, "user123");
        assert!(!claims.is_expired());
    }

    #[test]
    fn test_claims_roles() {
        let claims = Claims::new("user123", 3600)
            .with_role("admin")
            .with_role("user");

        assert!(claims.has_role("admin"));
        assert!(claims.has_role("user"));
        assert!(!claims.has_role("superadmin"));
    }

    #[test]
    fn test_parse_bearer_token() {
        assert_eq!(parse_bearer_token("Bearer abc123"), Some("abc123"));
        assert_eq!(parse_bearer_token("bearer abc123"), Some("abc123"));
        assert_eq!(parse_bearer_token("ApiKey abc123"), None);
    }

    #[test]
    fn test_parse_api_key() {
        assert_eq!(parse_api_key("ApiKey abc123"), Some("abc123"));
        assert_eq!(parse_api_key("apikey abc123"), Some("abc123"));
        assert_eq!(parse_api_key("Bearer abc123"), None);
    }

    #[test]
    fn test_role_guard() {
        let claims = Claims::new("user123", 3600)
            .with_role("admin")
            .with_role("user");

        let any_guard = RoleGuard::any_of(&["admin", "superadmin"]);
        assert!(any_guard.check(&claims));

        let all_guard = RoleGuard::all_of(&["admin", "user"]);
        assert!(all_guard.check(&claims));

        let missing_guard = RoleGuard::all_of(&["admin", "superadmin"]);
        assert!(!missing_guard.check(&claims));
    }
}
