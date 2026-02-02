//! Application event loop and lifecycle management.

use crate::backend::TerminalBackend;
use crate::renderer::{FramePacer, Renderer};
use crate::CrosstermBackend;
use cortex_tui_buffer::Buffer;
use cortex_tui_core::Result;
use cortex_tui_input::Event;
use crossterm::event::{poll, read};
use std::time::{Duration, Instant};

/// Application state for the event loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppState {
    /// Application is running normally.
    Running,
    /// Application should exit gracefully.
    Stopping,
    /// Application has stopped.
    Stopped,
}

/// Main application struct managing the terminal UI lifecycle.
///
/// Provides an event loop that handles input, rendering, and graceful shutdown.
///
/// # Example
///
/// ```no_run
/// use cortex_tui_terminal::{Application, CrosstermBackend, Style};
///
/// fn main() -> cortex_tui_core::Result<()> {
///     let backend = CrosstermBackend::new()?;
///     let mut app = Application::new(backend)?;
///     
///     app.run(|buffer, _dt| {
///         buffer.draw_str(0, 0, "Press 'q' to quit", Style::default());
///         true
///     })?;
///     
///     Ok(())
/// }
/// ```
pub struct Application<B: TerminalBackend = CrosstermBackend> {
    renderer: Renderer<B>,
    state: AppState,
    event_timeout: Duration,
}

impl Application<CrosstermBackend> {
    /// Creates a new application with the default crossterm backend.
    pub fn default_backend() -> Result<Self> {
        let backend = CrosstermBackend::new()?;
        Self::new(backend)
    }
}

impl<B: TerminalBackend> Application<B> {
    /// Creates a new application with the specified backend.
    pub fn new(backend: B) -> Result<Self> {
        let renderer = Renderer::new(backend)?;

        Ok(Self {
            renderer,
            state: AppState::Stopped,
            event_timeout: Duration::from_millis(1),
        })
    }

    /// Creates an application with a custom frame pacer.
    pub fn with_frame_pacer(backend: B, frame_pacer: FramePacer) -> Result<Self> {
        let renderer = Renderer::with_frame_pacer(backend, frame_pacer)?;

        Ok(Self {
            renderer,
            state: AppState::Stopped,
            event_timeout: Duration::from_millis(1),
        })
    }

    /// Returns the current application state.
    #[inline]
    pub fn state(&self) -> AppState {
        self.state
    }

    /// Returns a reference to the renderer.
    #[inline]
    pub fn renderer(&self) -> &Renderer<B> {
        &self.renderer
    }

    /// Returns a mutable reference to the renderer.
    #[inline]
    pub fn renderer_mut(&mut self) -> &mut Renderer<B> {
        &mut self.renderer
    }

    /// Returns the terminal width.
    #[inline]
    pub fn width(&self) -> u16 {
        self.renderer.width()
    }

    /// Returns the terminal height.
    #[inline]
    pub fn height(&self) -> u16 {
        self.renderer.height()
    }

    /// Sets the event polling timeout.
    pub fn set_event_timeout(&mut self, timeout: Duration) {
        self.event_timeout = timeout;
    }

    /// Requests the application to stop gracefully.
    pub fn quit(&mut self) {
        self.state = AppState::Stopping;
    }

    /// Polls for available input events without blocking.
    fn poll_events(&self) -> Result<Vec<Event>> {
        let mut events = Vec::new();

        while poll(self.event_timeout).map_err(cortex_tui_core::Error::Io)? {
            let event = read().map_err(cortex_tui_core::Error::Io)?;
            events.push(event.into());
        }

        Ok(events)
    }

    /// Handles a resize event by updating the renderer.
    fn handle_resize(&mut self, width: u16, height: u16) -> Result<()> {
        self.renderer.resize_to(width, height);
        self.renderer.force_redraw();
        Ok(())
    }

    /// Runs the application with a simple render callback.
    ///
    /// The callback receives the buffer and delta time, returning `true` to continue
    /// or `false` to exit.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cortex_tui_terminal::{Application, CrosstermBackend, Style};
    /// # fn main() -> cortex_tui_core::Result<()> {
    /// let backend = CrosstermBackend::new()?;
    /// let mut app = Application::new(backend)?;
    ///
    /// app.run(|buffer, dt| {
    ///     buffer.draw_str(0, 0, "Hello!", Style::default());
    ///     true // Continue running
    /// })?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn run<F>(&mut self, mut render_fn: F) -> Result<()>
    where
        F: FnMut(&mut Buffer, Duration) -> bool,
    {
        self.run_with_events(|buffer, dt, _events| render_fn(buffer, dt))
    }

    /// Runs the application with event handling.
    ///
    /// The callback receives the buffer, delta time, and any pending events.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cortex_tui_terminal::{Application, CrosstermBackend, Event, KeyCode, Style};
    /// # fn main() -> cortex_tui_core::Result<()> {
    /// let backend = CrosstermBackend::new()?;
    /// let mut app = Application::new(backend)?;
    ///
    /// app.run_with_events(|buffer, dt, events| {
    ///     for event in events {
    ///         if let Event::Key(key) = event {
    ///             if key.code == KeyCode::Char('q') {
    ///                 return false; // Exit
    ///             }
    ///         }
    ///     }
    ///     buffer.draw_str(0, 0, "Press 'q' to quit", Style::default());
    ///     true
    /// })?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn run_with_events<F>(&mut self, mut callback: F) -> Result<()>
    where
        F: FnMut(&mut Buffer, Duration, &[Event]) -> bool,
    {
        // Setup terminal
        self.renderer.setup()?;
        self.state = AppState::Running;

        let mut last_frame = Instant::now();

        // Main event loop
        let result = self.event_loop(&mut callback, &mut last_frame);

        // Always teardown, even on error
        let teardown_result = self.renderer.teardown();
        self.state = AppState::Stopped;

        // Return first error if any
        result?;
        teardown_result
    }

    /// The main event loop implementation.
    fn event_loop<F>(&mut self, callback: &mut F, last_frame: &mut Instant) -> Result<()>
    where
        F: FnMut(&mut Buffer, Duration, &[Event]) -> bool,
    {
        while self.state == AppState::Running {
            let frame_start = Instant::now();
            let delta_time = frame_start.duration_since(*last_frame);
            *last_frame = frame_start;

            // Poll for events
            let events = self.poll_events()?;

            // Handle resize events
            for event in &events {
                if let Event::Resize(width, height) = event {
                    self.handle_resize(*width, *height)?;
                }
            }

            // Clear buffer for new frame
            self.renderer.clear();

            // Call user callback
            let continue_running = callback(self.renderer.buffer(), delta_time, &events);

            if !continue_running {
                self.state = AppState::Stopping;
                break;
            }

            // Render the frame
            let frame_duration = frame_start.elapsed();
            self.renderer.render()?;

            // Frame pacing
            self.renderer.frame_pacer().wait(frame_duration);
            self.renderer.frame_pacer().tick();
        }

        Ok(())
    }

    /// Runs a single frame without setting up/tearing down the terminal.
    ///
    /// Useful for manual control over the terminal lifecycle.
    pub fn render_frame<F>(&mut self, mut render_fn: F) -> Result<Duration>
    where
        F: FnMut(&mut Buffer),
    {
        self.renderer.clear();
        render_fn(self.renderer.buffer());
        self.renderer.render_frame()
    }
}

/// Builder for creating customized applications.
pub struct ApplicationBuilder<B: TerminalBackend> {
    backend: B,
    frame_pacer: Option<FramePacer>,
    event_timeout: Duration,
}

impl<B: TerminalBackend> ApplicationBuilder<B> {
    /// Creates a new application builder with the specified backend.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            frame_pacer: None,
            event_timeout: Duration::from_millis(1),
        }
    }

    /// Sets the target FPS for normal rendering.
    pub fn target_fps(mut self, fps: u32) -> Self {
        let max_fps = self
            .frame_pacer
            .as_ref()
            .map(|p| p.effective_fps())
            .unwrap_or(60);
        self.frame_pacer = Some(FramePacer::new(fps, max_fps.max(fps)));
        self
    }

    /// Sets the maximum FPS for animations.
    pub fn max_fps(mut self, fps: u32) -> Self {
        let target_fps = self
            .frame_pacer
            .as_ref()
            .map(|p| p.effective_fps())
            .unwrap_or(30);
        self.frame_pacer = Some(FramePacer::new(target_fps.min(fps), fps));
        self
    }

    /// Sets a custom frame pacer.
    pub fn frame_pacer(mut self, pacer: FramePacer) -> Self {
        self.frame_pacer = Some(pacer);
        self
    }

    /// Sets the event polling timeout.
    pub fn event_timeout(mut self, timeout: Duration) -> Self {
        self.event_timeout = timeout;
        self
    }

    /// Builds the application.
    pub fn build(self) -> Result<Application<B>> {
        let mut app = if let Some(pacer) = self.frame_pacer {
            Application::with_frame_pacer(self.backend, pacer)?
        } else {
            Application::new(self.backend)?
        };

        app.set_event_timeout(self.event_timeout);
        Ok(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_state() {
        assert_ne!(AppState::Running, AppState::Stopping);
        assert_ne!(AppState::Stopping, AppState::Stopped);
    }

    #[test]
    fn test_builder_methods() {
        // Just verify builder methods compile - we can't test without a real terminal
        let _builder_chain = || {
            ApplicationBuilder::new(CrosstermBackend::new().unwrap())
                .target_fps(30)
                .max_fps(60)
                .event_timeout(Duration::from_millis(5));
        };
    }
}
