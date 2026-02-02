//! Cortex TUI Widgets
//!
//! Cortex-specific UI components built on cortex-core widgets.
//!
//! ## Available Widgets
//!
//! - [`StatusIndicator`](status_indicator::StatusIndicator) - Minimalist spinner + shimmer status indicator
//! - [`Toast`](toast::Toast) - Toast notification messages
//! - [`ToastManager`](toast::ToastManager) - Toast notification management
//! - [`ToastWidget`](toast::ToastWidget) - Toast rendering widget
//! - [`CommandPalette`](command_palette::CommandPalette) - VS Code-style command palette
//! - [`HelpBrowser`](help_browser::HelpBrowser) - Help documentation browser
//! - [`AutocompletePopup`](autocomplete::AutocompletePopup) - VS Code-style autocomplete popup
//! - [`SelectionList`](selection_list::SelectionList) - Generic selection list with search and shortcuts
//! - [`ApprovalOverlay`](approval_overlay::ApprovalOverlay) - Approval overlay for agent actions
//! - [`FormModal`](form::FormModal) - Generic form modal for user input
//! - [`ActionBar`](action_bar::ActionBar) - Reusable action bar for modal footers
//! - [`BacktrackOverlay`](backtrack_overlay::BacktrackOverlay) - Session rewind overlay

pub mod action_bar;
pub mod approval_overlay;
pub mod autocomplete;
pub mod backtrack_overlay;
pub mod command_palette;
pub mod form;
pub mod help_browser;
pub mod key_hints;
pub mod mention_popup;
pub mod model_picker;
// pub mod provider_picker; // REMOVED (single Cortex provider)
pub mod scrollable_dropdown;
pub mod selection_list;
pub mod status_indicator;
pub mod task_progress;
pub mod toast;

// Re-exports
pub use action_bar::{ActionBar, ActionItem, ActionStyle, NavHint};
pub use approval_overlay::{
    ApprovalDecision, ApprovalOverlay, ApprovalRequest, ChangeType, FileChange,
};
pub use autocomplete::{AutocompletePopup, MentionType, filter_mentions};
pub use backtrack_overlay::BacktrackOverlay;
pub use command_palette::{CommandPalette, CommandPaletteState, PaletteItem, RecentType};
pub use form::{FieldKind, FormField, FormModal, FormModalColors, FormState};
pub use help_browser::{
    HelpBrowser, HelpBrowserState, HelpContent, HelpFocus, HelpSection, get_help_sections,
};
pub use key_hints::{HintContext, KeyHints};
pub use mention_popup::MentionPopup;
pub use model_picker::{ModelItem, ModelPicker, ModelPickerState};
// pub use provider_picker::{PickerFocus, ProviderItem, ProviderPicker, ProviderPickerState}; // REMOVED (single Cortex provider)
pub use scrollable_dropdown::{
    DropdownItem, DropdownPosition, ScrollableDropdown, ScrollbarStyle, calculate_scroll_offset,
    select_next, select_prev,
};
pub use selection_list::{SelectionItem, SelectionList, SelectionResult};
pub use status_indicator::{StatusIndicator, fmt_elapsed_compact};
pub use task_progress::{
    CompactProgressIndicator, CurrentTool, ParallelTaskProgressWidget, ProgressCollector,
    ProgressState, TaskProgress, TaskProgressWidget, TaskStatus, ToolCallSummary,
};
pub use toast::{Toast, ToastLevel, ToastManager, ToastPosition, ToastWidget};
