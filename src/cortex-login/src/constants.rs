//! Constants for the cortex-login module.

/// Client ID for OAuth authentication.
pub const CLIENT_ID: &str = "cortex-cli";

/// Default OAuth issuer URL.
pub const DEFAULT_ISSUER: &str = "https://auth.cortex.foundation";

/// Environment variable for API key.
pub const API_KEY_ENV_VAR: &str = "CORTEX_API_KEY";

/// Service name for keyring storage.
pub const KEYRING_SERVICE: &str = "cortex-cli";

/// New service name for keyring storage
pub const KEYRING_SERVICE_NEW: &str = "cortex-cli";

/// Legacy service name (for migration)
pub const KEYRING_SERVICE_LEGACY: &str = "cortex-cli";

/// Account name for keyring storage.
pub const KEYRING_ACCOUNT: &str = "auth";

/// API base URL for token refresh.
pub const API_BASE_URL: &str = "https://api.cortex.foundation";

/// User-Agent string for HTTP requests
pub const USER_AGENT: &str = concat!("cortex-cli/", env!("CARGO_PKG_VERSION"));
