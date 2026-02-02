#![allow(clippy::too_many_arguments)]
//! # Cortex TUI Components
//!
//! A standardized component library for building consistent terminal UIs in Cortex.
//!
//! This crate provides high-level, reusable components that wrap ratatui primitives,
//! ensuring consistent styling, behavior, and patterns across the entire TUI.
//!
//! ## Philosophy
//!
//! **Never use raw ratatui directly in application code.** Instead, use these components
//! which provide:
//!
//! 1. **Consistent theming** - All components use `cortex-core` styles
//! 2. **Standard behaviors** - Navigation, selection, scrolling work the same everywhere
//! 3. **Accessibility** - Key hints, screen reader support, proper focus management
//! 4. **Reduced boilerplate** - Common patterns are encapsulated in reusable components
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use cortex_tui_components::prelude::*;
//!
//! // Create a dropdown selector
//! let mut selector = Selector::new(vec![
//!     SelectItem::new("option1", "First Option").with_description("Description here"),
//!     SelectItem::new("option2", "Second Option").current(),
//!     SelectItem::new("option3", "Third Option").with_shortcut('3'),
//! ])
//! .with_title("Choose an option")
//! .searchable();
//!
//! // Handle input
//! match selector.handle_key(key_event) {
//!     SelectResult::Selected(idx) => { /* handle selection */ }
//!     SelectResult::Cancelled => { /* handle cancel */ }
//!     SelectResult::Continue => { /* keep displaying */ }
//! }
//!
//! // Render the component
//! frame.render_widget(&selector, area);
//! ```
//!
//! ## Component Categories
//!
//! ### Input Components
//! - [`TextInput`](input::TextInput) - Single-line text input with cursor
//! - [`TextArea`](input::TextArea) - Multi-line text input
//!
//! ### Selection Components  
//! - [`Selector`](selector::Selector) - Generic selection list with search/shortcuts
//! - [`Dropdown`](dropdown::Dropdown) - Compact dropdown menu
//! - [`RadioGroup`](radio::RadioGroup) - Radio button selection
//! - [`CheckboxGroup`](checkbox::CheckboxGroup) - Checkbox multi-selection
//!
//! ### Display Components
//! - [`Card`](card::Card) - Bordered container with title
//! - [`List`](list::List) - Scrollable list of items
//! - [`Toast`](toast::Toast) - Notification messages
//! - [`Spinner`](spinner::LoadingSpinner) - Loading indicators
//!
//! ### Container Components
//! - [`Modal`](modal::Modal) - Overlay dialog container
//! - [`Popup`](popup::Popup) - Inline popup (like autocomplete)
//! - [`Panel`](panel::Panel) - Resizable panel container
//!
//! ### Form Components
//! - [`Form`](form::Form) - Complete form with multiple fields
//! - [`FormField`](form::FormField) - Individual form field
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                      Application Code                               │
//! │  (Uses cortex-tui-components, never raw ratatui)                   │
//! └─────────────────────────────────┬───────────────────────────────────┘
//!                                   │
//! ┌─────────────────────────────────▼───────────────────────────────────┐
//! │                    cortex-tui-components                            │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌───────────┐  │
//! │  │  Selector   │  │   Modal     │  │    Form     │  │  Dropdown │  │
//! │  │  (search,   │  │  (overlay,  │  │  (fields,   │  │  (popup,  │  │
//! │  │  shortcuts) │  │  stacking)  │  │  validation)│  │  scroll)  │  │
//! │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └─────┬─────┘  │
//! │         │                │                │                │        │
//! │  ┌──────▼────────────────▼────────────────▼────────────────▼─────┐ │
//! │  │                    Component Trait                            │ │
//! │  │  render() | handle_key() | key_hints() | focus_state()       │ │
//! │  └──────────────────────────┬────────────────────────────────────┘ │
//! └─────────────────────────────┼───────────────────────────────────────┘
//!                               │
//! ┌─────────────────────────────▼───────────────────────────────────────┐
//! │                        cortex-core                                  │
//! │                   (styles, colors, animations)                      │
//! └─────────────────────────────┬───────────────────────────────────────┘
//!                               │
//! ┌─────────────────────────────▼───────────────────────────────────────┐
//! │                         ratatui                                     │
//! │                  (low-level TUI rendering)                          │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## For AI Agents
//!
//! When implementing new TUI features, **always use this crate**:
//!
//! 1. **Don't import `ratatui::widgets::*` directly** - Use our components instead
//! 2. **Follow the patterns** - Look at existing components for consistent implementation
//! 3. **Use the Component trait** - All interactive elements should implement it
//! 4. **Add key hints** - Every component should report its keyboard shortcuts
//! 5. **Support theming** - Use `cortex_core::style::*` constants, never hardcode colors

pub mod borders;
pub mod card;
pub mod checkbox;
pub mod color_scheme;
pub mod component;
pub mod dropdown;
pub mod focus;
pub mod form;
pub mod input;
pub mod key_hints;
pub mod list;
pub mod mascot;
pub mod modal;
pub mod page_layout;
pub mod panel;
pub mod popup;
pub mod radio;
pub mod scroll;
pub mod selection_list;
pub mod selector;
pub mod spinner;
pub mod text;
pub mod toast;
pub mod welcome_card;

/// Commonly used types and traits for quick imports.
///
/// ```rust,ignore
/// use cortex_tui_components::prelude::*;
/// ```
pub mod prelude {
    pub use crate::borders::{BorderStyle, RoundedBorder};
    pub use crate::card::{Card, CardBuilder};
    pub use crate::checkbox::{CheckboxGroup, CheckboxItem};
    pub use crate::color_scheme::ColorScheme;
    pub use crate::component::{Component, ComponentResult, FocusState};
    pub use crate::dropdown::{Dropdown, DropdownItem, DropdownPosition, DropdownState};
    pub use crate::focus::{FocusDirection, FocusManager};
    pub use crate::form::{Form, FormBuilder, FormField, FormFieldKind, FormResult, FormState};
    pub use crate::input::{InputState, TextInput};
    pub use crate::key_hints::{KeyHint, KeyHintsBar};
    pub use crate::list::{ListItem, ScrollableList};
    pub use crate::mascot::{MASCOT, MASCOT_MINIMAL, MASCOT_MINIMAL_LINES, MascotExpression};
    pub use crate::modal::{Modal, ModalAction, ModalBuilder, ModalResult, ModalStack};
    pub use crate::page_layout::{
        Badge, InfoItem, InfoSection, NavItem, Navbar, PageHeader, PageLayout, PageTab,
    };
    pub use crate::panel::{Panel, PanelPosition};
    pub use crate::popup::{Popup, PopupPosition};
    pub use crate::radio::{RadioGroup, RadioItem};
    pub use crate::scroll::{ScrollState, Scrollable};
    pub use crate::selection_list::{SelectionItem, SelectionList, SelectionResult};
    pub use crate::selector::{SelectItem, SelectResult, Selector, SelectorState};
    pub use crate::spinner::{LoadingSpinner, SpinnerStyle};
    pub use crate::text::{StyledText, TextStyle};
    pub use crate::toast::{Toast, ToastLevel, ToastManager, ToastPosition, ToastWidget};
    pub use crate::welcome_card::{InfoCard, InfoCardPair, ToLines, WelcomeCard};
}

// Re-export cortex-core style for convenience
pub use cortex_core::style;

/// Cortex TUI Components version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
