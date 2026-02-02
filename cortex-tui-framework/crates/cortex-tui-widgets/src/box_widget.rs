//! Box container widget.
//!
//! The Box widget is a container that can hold child widgets and optionally
//! display a border around its content.

use crate::border::{
    draw_border, inner_rect, BorderChars, BorderSides, BorderStyle, DrawBorderParams,
    TitleAlignment,
};
use crate::buffer::Buffer;
use crate::event::{Event, EventResult};
use crate::layout::{Dimension, FlexDirection, LayoutStyle};
use crate::types::{Color, Edges, Rect, Style};
use crate::widget::{Widget, WidgetId, WidgetRef};
use std::any::Any;

/// A container widget with optional border and background.
///
/// The Box widget is the primary container for laying out child widgets.
/// It supports:
/// - Various border styles (single, double, rounded, heavy, ascii, custom)
/// - Background color
/// - Padding
/// - Flexbox-style layout for children
/// - Optional title in the border
///
/// # Example
///
/// ```ignore
/// let container = BoxWidget::builder()
///     .border_style(BorderStyle::Rounded)
///     .background(Color::rgb(0.1, 0.1, 0.1))
///     .padding(1)
///     .title("My Container")
///     .build();
/// ```
pub struct BoxWidget {
    /// Unique identifier for this widget.
    id: WidgetId,
    /// Layout style for this widget.
    layout: LayoutStyle,
    /// Child widgets.
    children: Vec<WidgetRef>,
    /// Border style.
    border_style: BorderStyle,
    /// Which sides of the border to draw.
    border_sides: BorderSides,
    /// Border color.
    border_color: Option<Color>,
    /// Background color.
    background: Option<Color>,
    /// Padding inside the border.
    padding: Edges,
    /// Title displayed in the top border.
    title: Option<String>,
    /// Title alignment.
    title_alignment: TitleAlignment,
    /// Title style.
    title_style: Option<Style>,
}

impl std::fmt::Debug for BoxWidget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxWidget")
            .field("id", &self.id)
            .field("layout", &self.layout)
            .field("children", &format!("[{} widgets]", self.children.len()))
            .field("border_style", &self.border_style)
            .field("border_sides", &self.border_sides)
            .field("border_color", &self.border_color)
            .field("background", &self.background)
            .field("padding", &self.padding)
            .field("title", &self.title)
            .field("title_alignment", &self.title_alignment)
            .field("title_style", &self.title_style)
            .finish()
    }
}

impl BoxWidget {
    /// Creates a new empty box widget with default settings.
    pub fn new() -> Self {
        Self {
            id: WidgetId::new(),
            layout: LayoutStyle::default(),
            children: Vec::new(),
            border_style: BorderStyle::None,
            border_sides: BorderSides::ALL,
            border_color: None,
            background: None,
            padding: Edges::ZERO,
            title: None,
            title_alignment: TitleAlignment::Left,
            title_style: None,
        }
    }

    /// Creates a builder for constructing a box widget.
    pub fn builder() -> BoxWidgetBuilder {
        BoxWidgetBuilder::new()
    }

    /// Adds a child widget.
    pub fn add_child(&mut self, child: WidgetRef) {
        self.children.push(child);
    }

    /// Adds multiple children.
    pub fn add_children(&mut self, children: impl IntoIterator<Item = WidgetRef>) {
        self.children.extend(children);
    }

    /// Removes all children.
    pub fn clear_children(&mut self) {
        self.children.clear();
    }

    /// Sets the border style.
    pub fn set_border_style(&mut self, style: BorderStyle) {
        self.border_style = style;
    }

    /// Sets which border sides to draw.
    pub fn set_border_sides(&mut self, sides: BorderSides) {
        self.border_sides = sides;
    }

    /// Sets the border color.
    pub fn set_border_color(&mut self, color: Option<Color>) {
        self.border_color = color;
    }

    /// Sets the background color.
    pub fn set_background(&mut self, color: Option<Color>) {
        self.background = color;
    }

    /// Sets the padding.
    pub fn set_padding(&mut self, padding: Edges) {
        self.padding = padding;
    }

    /// Sets the title.
    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    /// Returns the border style.
    pub fn border_style(&self) -> BorderStyle {
        self.border_style
    }

    /// Returns which border sides are drawn.
    pub fn border_sides(&self) -> BorderSides {
        self.border_sides
    }

    /// Returns the border color.
    pub fn border_color(&self) -> Option<Color> {
        self.border_color
    }

    /// Returns the background color.
    pub fn background(&self) -> Option<Color> {
        self.background
    }

    /// Returns the padding.
    pub fn padding(&self) -> Edges {
        self.padding
    }

    /// Returns the title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Calculates the content area inside the border and padding.
    pub fn content_rect(&self, rect: Rect) -> Rect {
        // Account for border
        let after_border = if self.border_style.is_none() {
            rect
        } else {
            inner_rect(rect, self.border_sides)
        };

        // Account for padding
        after_border.inset_sides(
            self.padding.left,
            self.padding.top,
            self.padding.right,
            self.padding.bottom,
        )
    }
}

impl Default for BoxWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for BoxWidget {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn type_name(&self) -> &'static str {
        "Box"
    }

    fn layout(&self) -> &LayoutStyle {
        &self.layout
    }

    fn layout_mut(&mut self) -> &mut LayoutStyle {
        &mut self.layout
    }

    fn render(&self, buffer: &mut Buffer, rect: Rect) {
        if rect.is_empty() {
            return;
        }

        // Draw background
        if let Some(bg_color) = self.background {
            let bg_style = Style::new().bg(bg_color);
            // cortex_tui_buffer::Buffer::fill takes (rect, character, style)
            buffer.fill(rect, ' ', bg_style);
        }

        // Draw border
        if !self.border_style.is_none() && self.border_sides.any() {
            let params = DrawBorderParams {
                rect,
                style: self.border_style,
                sides: self.border_sides,
                color: self.border_color,
                title: self.title.clone(),
                title_alignment: self.title_alignment,
                title_style: self.title_style,
            };
            draw_border(buffer, &params);
        }

        // Note: Child rendering is typically handled by the layout system,
        // which will call render on each child with the appropriate rect.
        // This render method only handles the box's own visual elements.
    }

    fn handle_event(&mut self, event: &Event) -> EventResult {
        // Box itself doesn't handle events, but forwards to children
        for child in &mut self.children {
            if child.handle_event(event).is_handled() {
                return EventResult::Handled;
            }
        }
        EventResult::Ignored
    }

    fn children(&self) -> &[WidgetRef] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [WidgetRef] {
        &mut self.children
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn measure(&self, _available_width: f32, _available_height: f32) -> (f32, f32) {
        // Calculate intrinsic size based on border and padding
        let border_width = if self.border_style.is_none() {
            0
        } else {
            self.border_sides.left as u16 + self.border_sides.right as u16
        };
        let border_height = if self.border_style.is_none() {
            0
        } else {
            self.border_sides.top as u16 + self.border_sides.bottom as u16
        };

        let padding_width = self.padding.horizontal();
        let padding_height = self.padding.vertical();

        let min_width = border_width + padding_width;
        let min_height = border_height + padding_height;

        // Children size would be calculated by the layout system
        (min_width as f32, min_height as f32)
    }
}

/// Builder for constructing BoxWidget instances.
#[derive(Debug)]
pub struct BoxWidgetBuilder {
    widget: BoxWidget,
}

impl BoxWidgetBuilder {
    /// Creates a new builder.
    pub fn new() -> Self {
        Self {
            widget: BoxWidget::new(),
        }
    }

    /// Sets the border style.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.widget.border_style = style;
        if !style.is_none() && self.widget.border_sides.none() {
            self.widget.border_sides = BorderSides::ALL;
        }
        self
    }

    /// Sets a single-line border.
    pub fn border_single(self) -> Self {
        self.border_style(BorderStyle::Single)
    }

    /// Sets a double-line border.
    pub fn border_double(self) -> Self {
        self.border_style(BorderStyle::Double)
    }

    /// Sets a rounded border.
    pub fn border_rounded(self) -> Self {
        self.border_style(BorderStyle::Rounded)
    }

    /// Sets a heavy border.
    pub fn border_heavy(self) -> Self {
        self.border_style(BorderStyle::Heavy)
    }

    /// Sets an ASCII border.
    pub fn border_ascii(self) -> Self {
        self.border_style(BorderStyle::Ascii)
    }

    /// Sets custom border characters.
    pub fn border_custom(self, chars: BorderChars) -> Self {
        self.border_style(BorderStyle::Custom(chars))
    }

    /// Sets which sides of the border to draw.
    pub fn border_sides(mut self, sides: BorderSides) -> Self {
        self.widget.border_sides = sides;
        self
    }

    /// Sets the border color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.widget.border_color = Some(color);
        self
    }

    /// Sets the background color.
    pub fn background(mut self, color: Color) -> Self {
        self.widget.background = Some(color);
        self
    }

    /// Sets uniform padding on all sides.
    pub fn padding(mut self, padding: u16) -> Self {
        self.widget.padding = Edges::uniform(padding);
        self
    }

    /// Sets padding for each side individually.
    pub fn padding_edges(mut self, top: u16, right: u16, bottom: u16, left: u16) -> Self {
        self.widget.padding = Edges::new(top, right, bottom, left);
        self
    }

    /// Sets symmetric padding (vertical, horizontal).
    pub fn padding_symmetric(mut self, vertical: u16, horizontal: u16) -> Self {
        self.widget.padding = Edges::symmetric(vertical, horizontal);
        self
    }

    /// Sets the title displayed in the top border.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.widget.title = Some(title.into());
        self
    }

    /// Sets the title alignment.
    pub fn title_alignment(mut self, alignment: TitleAlignment) -> Self {
        self.widget.title_alignment = alignment;
        self
    }

    /// Sets the title style.
    pub fn title_style(mut self, style: Style) -> Self {
        self.widget.title_style = Some(style);
        self
    }

    /// Sets the width.
    pub fn width(mut self, width: impl Into<Dimension>) -> Self {
        self.widget.layout.width = width.into();
        self
    }

    /// Sets the height.
    pub fn height(mut self, height: impl Into<Dimension>) -> Self {
        self.widget.layout.height = height.into();
        self
    }

    /// Sets both width and height.
    pub fn size(mut self, width: impl Into<Dimension>, height: impl Into<Dimension>) -> Self {
        self.widget.layout.width = width.into();
        self.widget.layout.height = height.into();
        self
    }

    /// Sets the flex grow factor.
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.widget.layout.flex_grow = grow;
        self
    }

    /// Sets the flex shrink factor.
    pub fn flex_shrink(mut self, shrink: f32) -> Self {
        self.widget.layout.flex_shrink = shrink;
        self
    }

    /// Sets the flex direction for children.
    pub fn flex_direction(mut self, direction: FlexDirection) -> Self {
        self.widget.layout.flex_direction = direction;
        self
    }

    /// Arranges children in a row.
    pub fn row(self) -> Self {
        self.flex_direction(FlexDirection::Row)
    }

    /// Arranges children in a column.
    pub fn column(self) -> Self {
        self.flex_direction(FlexDirection::Column)
    }

    /// Sets the gap between children.
    pub fn gap(mut self, gap: f32) -> Self {
        self.widget.layout.gap = gap;
        self.widget.layout.column_gap = gap;
        self.widget.layout.row_gap = gap;
        self
    }

    /// Adds a child widget.
    pub fn child(mut self, child: impl Widget + 'static) -> Self {
        self.widget.children.push(Box::new(child));
        self
    }

    /// Adds a boxed child widget.
    pub fn child_boxed(mut self, child: WidgetRef) -> Self {
        self.widget.children.push(child);
        self
    }

    /// Adds multiple children.
    pub fn children(mut self, children: impl IntoIterator<Item = WidgetRef>) -> Self {
        self.widget.children.extend(children);
        self
    }

    /// Builds the BoxWidget.
    pub fn build(self) -> BoxWidget {
        self.widget
    }
}

impl Default for BoxWidgetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Creates a simple container box with a border.
pub fn bordered_box(style: BorderStyle) -> BoxWidget {
    BoxWidget::builder().border_style(style).build()
}

/// Creates a container with rounded corners.
pub fn rounded_box() -> BoxWidget {
    bordered_box(BorderStyle::Rounded)
}

/// Creates a container with a single-line border.
pub fn single_box() -> BoxWidget {
    bordered_box(BorderStyle::Single)
}

/// Creates a container with a double-line border.
pub fn double_box() -> BoxWidget {
    bordered_box(BorderStyle::Double)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_box_widget_creation() {
        let widget = BoxWidget::new();
        assert_eq!(widget.type_name(), "Box");
        assert!(widget.children().is_empty());
        assert!(widget.border_style().is_none());
    }

    #[test]
    fn test_box_widget_builder() {
        let widget = BoxWidget::builder()
            .border_rounded()
            .padding(2)
            .title("Test")
            .width(100u16)
            .height(50u16)
            .build();

        assert_eq!(widget.border_style(), BorderStyle::Rounded);
        assert_eq!(widget.padding(), Edges::uniform(2));
        assert_eq!(widget.title(), Some("Test"));
        assert_eq!(widget.layout().width, Dimension::Points(100.0));
    }

    #[test]
    fn test_content_rect() {
        let widget = BoxWidget::builder().border_single().padding(1).build();

        let outer = Rect::new(0, 0, 20, 10);
        let inner = widget.content_rect(outer);

        // Border takes 1 on each side, padding takes 1 on each side
        assert_eq!(inner, Rect::new(2, 2, 16, 6));
    }

    #[test]
    fn test_box_render_background() {
        let widget = BoxWidget::builder()
            .background(Color::rgb(0.5, 0.5, 0.5))
            .build();

        let mut buffer = Buffer::new(10, 10);
        widget.render(&mut buffer, Rect::new(0, 0, 10, 10));

        // Background should be applied
        assert_eq!(buffer.get(5, 5).unwrap().bg, Color::rgb(0.5, 0.5, 0.5));
    }

    #[test]
    fn test_box_render_border() {
        let widget = BoxWidget::builder().border_single().build();

        let mut buffer = Buffer::new(10, 5);
        widget.render(&mut buffer, Rect::new(0, 0, 10, 5));

        assert_eq!(buffer.get(0, 0).unwrap().character, '┌');
        assert_eq!(buffer.get(9, 0).unwrap().character, '┐');
        assert_eq!(buffer.get(0, 4).unwrap().character, '└');
        assert_eq!(buffer.get(9, 4).unwrap().character, '┘');
    }

    #[test]
    fn test_add_children() {
        let mut parent = BoxWidget::new();
        let child = BoxWidget::new();
        parent.add_child(Box::new(child));

        assert_eq!(parent.children().len(), 1);
    }

    #[test]
    fn test_convenience_constructors() {
        let r = rounded_box();
        assert_eq!(r.border_style(), BorderStyle::Rounded);

        let s = single_box();
        assert_eq!(s.border_style(), BorderStyle::Single);

        let d = double_box();
        assert_eq!(d.border_style(), BorderStyle::Double);
    }
}
