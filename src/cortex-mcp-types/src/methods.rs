//! MCP method name constants.

// Lifecycle
/// Initialize method.
pub const INITIALIZE: &str = "initialize";

// Notifications
/// Initialized notification.
pub const INITIALIZED: &str = "notifications/initialized";
/// Progress notification.
pub const PROGRESS: &str = "notifications/progress";
/// Cancelled notification.
pub const CANCELLED: &str = "notifications/cancelled";
/// Roots list changed notification.
pub const ROOTS_LIST_CHANGED: &str = "notifications/roots/list_changed";
/// Tools list changed notification.
pub const TOOLS_LIST_CHANGED: &str = "notifications/tools/list_changed";
/// Resources list changed notification.
pub const RESOURCES_LIST_CHANGED: &str = "notifications/resources/list_changed";
/// Prompts list changed notification.
pub const PROMPTS_LIST_CHANGED: &str = "notifications/prompts/list_changed";
/// Log message notification.
pub const LOG_MESSAGE: &str = "notifications/message";
/// Resource updated notification.
pub const RESOURCE_UPDATED: &str = "notifications/resources/updated";

// Tools
/// List tools method.
pub const TOOLS_LIST: &str = "tools/list";
/// Call tool method.
pub const TOOLS_CALL: &str = "tools/call";

// Resources
/// List resources method.
pub const RESOURCES_LIST: &str = "resources/list";
/// List resource templates method.
pub const RESOURCE_TEMPLATES_LIST: &str = "resources/templates/list";
/// Read resource method.
pub const RESOURCES_READ: &str = "resources/read";
/// Subscribe to resource method.
pub const RESOURCES_SUBSCRIBE: &str = "resources/subscribe";
/// Unsubscribe from resource method.
pub const RESOURCES_UNSUBSCRIBE: &str = "resources/unsubscribe";

// Prompts
/// List prompts method.
pub const PROMPTS_LIST: &str = "prompts/list";
/// Get prompt method.
pub const PROMPTS_GET: &str = "prompts/get";

// Logging
/// Set log level method.
pub const LOGGING_SET_LEVEL: &str = "logging/setLevel";

// Sampling
/// Create sampling message method.
pub const SAMPLING_CREATE_MESSAGE: &str = "sampling/createMessage";

// Roots
/// List roots method.
pub const ROOTS_LIST: &str = "roots/list";

// Ping
/// Ping method.
pub const PING: &str = "ping";
