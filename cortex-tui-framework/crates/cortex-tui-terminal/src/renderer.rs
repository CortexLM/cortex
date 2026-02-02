//! Renderer with double-buffering and frame pacing.

use crate::backend::TerminalBackend;
use cortex_tui_buffer::{Buffer, DoubleBuffer};
use cortex_tui_core::{Color, Result};
use std::time::{Duration, Instant};

/// Frame rate pacer for adaptive rendering.
///
/// Manages frame timing with configurable target and maximum FPS.
/// Supports "live" mode for animations requiring higher frame rates.
#[derive(Debug)]
pub struct FramePacer {
    target_fps: u32,
    max_fps: u32,
    target_frame_time: Duration,
    min_frame_time: Duration,
    last_frame: Instant,
    frame_count: u64,
    live_count: u32,
    fps_sample_time: Instant,
    current_fps: f32,
}

impl Default for FramePacer {
    fn default() -> Self {
        Self::new(30, 60)
    }
}

impl FramePacer {
    /// Creates a new frame pacer with target and maximum FPS.
    ///
    /// - `target_fps`: Normal mode frame rate (e.g., 30 for static UI)
    /// - `max_fps`: Animation mode frame rate (e.g., 60)
    pub fn new(target_fps: u32, max_fps: u32) -> Self {
        let target_fps = target_fps.max(1);
        let max_fps = max_fps.max(target_fps);

        Self {
            target_fps,
            max_fps,
            target_frame_time: Duration::from_secs_f64(1.0 / target_fps as f64),
            min_frame_time: Duration::from_secs_f64(1.0 / max_fps as f64),
            last_frame: Instant::now(),
            frame_count: 0,
            live_count: 0,
            fps_sample_time: Instant::now(),
            current_fps: 0.0,
        }
    }

    /// Requests "live" mode for animations (higher frame rate).
    ///
    /// Uses reference counting - call `drop_live()` when animation ends.
    pub fn request_live(&mut self) {
        self.live_count += 1;
    }

    /// Releases a "live" mode request.
    pub fn drop_live(&mut self) {
        self.live_count = self.live_count.saturating_sub(1);
    }

    /// Returns whether live mode is active.
    #[inline]
    pub fn is_live(&self) -> bool {
        self.live_count > 0
    }

    /// Returns the current effective target FPS.
    pub fn effective_fps(&self) -> u32 {
        if self.is_live() {
            self.max_fps
        } else {
            self.target_fps
        }
    }

    /// Returns the current measured FPS.
    #[inline]
    pub fn current_fps(&self) -> f32 {
        self.current_fps
    }

    /// Returns the total frame count.
    #[inline]
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// Returns the time to wait before the next frame.
    pub fn frame_delay(&self, frame_duration: Duration) -> Duration {
        let target = if self.is_live() {
            self.min_frame_time
        } else {
            self.target_frame_time
        };

        target.saturating_sub(frame_duration)
    }

    /// Advances to the next frame and returns delta time.
    pub fn tick(&mut self) -> Duration {
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame);
        self.last_frame = now;
        self.frame_count += 1;

        // Update FPS measurement every second
        let sample_elapsed = now.duration_since(self.fps_sample_time);
        if sample_elapsed >= Duration::from_secs(1) {
            self.current_fps = self.frame_count as f32 / sample_elapsed.as_secs_f32();
            self.frame_count = 0;
            self.fps_sample_time = now;
        }

        delta
    }

    /// Waits for the appropriate frame time.
    pub fn wait(&self, frame_duration: Duration) {
        let delay = self.frame_delay(frame_duration);
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
    }
}

/// Terminal renderer with double-buffering.
///
/// The renderer manages a double-buffered terminal display, computing
/// differential updates to minimize output and prevent flickering.
pub struct Renderer<B: TerminalBackend> {
    backend: B,
    buffers: DoubleBuffer,
    frame_pacer: FramePacer,
    background_color: Color,
}

impl<B: TerminalBackend> Renderer<B> {
    /// Creates a new renderer with the given backend.
    pub fn new(backend: B) -> Result<Self> {
        let (width, height) = backend.size()?;

        Ok(Self {
            backend,
            buffers: DoubleBuffer::new(width, height),
            frame_pacer: FramePacer::default(),
            background_color: Color::TRANSPARENT,
        })
    }

    /// Creates a renderer with custom frame pacing.
    pub fn with_frame_pacer(backend: B, frame_pacer: FramePacer) -> Result<Self> {
        let (width, height) = backend.size()?;

        Ok(Self {
            backend,
            buffers: DoubleBuffer::new(width, height),
            frame_pacer,
            background_color: Color::TRANSPARENT,
        })
    }

    /// Returns the terminal width.
    #[inline]
    pub fn width(&self) -> u16 {
        self.buffers.width()
    }

    /// Returns the terminal height.
    #[inline]
    pub fn height(&self) -> u16 {
        self.buffers.height()
    }

    /// Returns the terminal size as a tuple.
    #[inline]
    pub fn size(&self) -> (u16, u16) {
        self.buffers.size()
    }

    /// Returns a reference to the backend.
    #[inline]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Returns a mutable reference to the backend.
    #[inline]
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Returns a mutable reference to the frame pacer.
    #[inline]
    pub fn frame_pacer(&mut self) -> &mut FramePacer {
        &mut self.frame_pacer
    }

    /// Returns a mutable reference to the render buffer.
    #[inline]
    pub fn buffer(&mut self) -> &mut Buffer {
        self.buffers.back_mut()
    }

    /// Sets the default background color.
    pub fn set_background(&mut self, color: Color) {
        self.background_color = color;
    }

    /// Forces a full redraw on the next render.
    pub fn force_redraw(&mut self) {
        self.buffers
            .resize(self.buffers.width(), self.buffers.height());
    }

    /// Resizes the renderer to match terminal size.
    pub fn resize(&mut self) -> Result<()> {
        let (width, height) = self.backend.size()?;
        self.buffers.resize(width, height);
        Ok(())
    }

    /// Resizes to specific dimensions.
    pub fn resize_to(&mut self, width: u16, height: u16) {
        self.buffers.resize(width, height);
    }

    /// Clears the render buffer.
    pub fn clear(&mut self) {
        self.buffers.clear_with_bg(self.background_color);
    }

    /// Renders the buffer to the terminal using differential updates.
    ///
    /// This is the core rendering method that:
    /// 1. Computes the diff between current and next buffers
    /// 2. Generates optimal ANSI sequences for changes
    /// 3. Uses synchronized output to prevent tearing
    /// 4. Swaps buffers for the next frame
    pub fn render(&mut self) -> Result<()> {
        // Begin synchronized output
        self.backend.begin_sync_update()?;
        self.backend.hide_cursor()?;

        // Compute the diff
        let diff = self.buffers.diff_default();

        // Track current style to minimize escape sequences
        let mut current_fg: Option<Color> = None;
        let mut current_bg: Option<Color> = None;
        let mut current_attrs = None;

        const COLOR_EPSILON: f32 = 0.00001;

        // Process all cell changes
        for run in &diff.runs {
            for (i, cell) in run.cells.iter().enumerate() {
                let x = run.x + i as u16;
                let y = run.y;

                // Skip continuation cells (handled by wide character)
                if cell.is_continuation() {
                    continue;
                }

                // Check if style needs update
                let fg_match = current_fg.is_some_and(|c| c.approx_eq(&cell.fg, COLOR_EPSILON));
                let bg_match = current_bg.is_some_and(|c| c.approx_eq(&cell.bg, COLOR_EPSILON));
                let attr_match = current_attrs == Some(cell.attributes);

                if !fg_match || !bg_match || !attr_match {
                    // Reset and apply new style
                    self.backend.reset_style()?;
                    self.backend.set_foreground(cell.fg)?;
                    self.backend.set_background(cell.bg)?;
                    self.backend.set_attributes(cell.attributes)?;

                    current_fg = Some(cell.fg);
                    current_bg = Some(cell.bg);
                    current_attrs = Some(cell.attributes);
                }

                // Move cursor
                self.backend.move_cursor(x, y)?;

                // Write the character
                let mut buf = [0u8; 4];
                let s = cell.character.encode_utf8(&mut buf);
                self.backend.write_str(s)?;
            }
        }

        // Reset style at end
        self.backend.reset_style()?;

        // End synchronized output and flush
        self.backend.end_sync_update()?;
        self.backend.flush()?;

        // Swap buffers for next frame
        self.buffers.swap();

        Ok(())
    }

    /// Renders and handles frame pacing.
    ///
    /// Returns the actual frame duration.
    pub fn render_frame(&mut self) -> Result<Duration> {
        let frame_start = Instant::now();

        self.render()?;

        let frame_duration = frame_start.elapsed();
        self.frame_pacer.wait(frame_duration);

        Ok(frame_duration)
    }

    /// Prepares the terminal for rendering.
    pub fn setup(&mut self) -> Result<()> {
        self.backend.enter_raw_mode()?;
        self.backend.enter_alternate_screen()?;
        self.backend.enable_mouse_capture()?;
        self.backend.enable_bracketed_paste()?;
        self.backend.enable_focus_change()?;
        self.backend.hide_cursor()?;
        self.backend.clear()?;
        self.force_redraw();
        Ok(())
    }

    /// Restores the terminal to its original state.
    pub fn teardown(&mut self) -> Result<()> {
        self.backend.show_cursor()?;
        self.backend.disable_focus_change()?;
        self.backend.disable_bracketed_paste()?;
        self.backend.disable_mouse_capture()?;
        self.backend.leave_alternate_screen()?;
        self.backend.exit_raw_mode()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_pacer_creation() {
        let pacer = FramePacer::new(30, 60);
        assert_eq!(pacer.target_fps, 30);
        assert_eq!(pacer.max_fps, 60);
        assert!(!pacer.is_live());
    }

    #[test]
    fn test_frame_pacer_live_mode() {
        let mut pacer = FramePacer::new(30, 60);

        pacer.request_live();
        assert!(pacer.is_live());
        assert_eq!(pacer.effective_fps(), 60);

        pacer.drop_live();
        assert!(!pacer.is_live());
        assert_eq!(pacer.effective_fps(), 30);
    }

    #[test]
    fn test_frame_pacer_refcount() {
        let mut pacer = FramePacer::new(30, 60);

        pacer.request_live();
        pacer.request_live();
        assert!(pacer.is_live());

        pacer.drop_live();
        assert!(pacer.is_live()); // Still live (one request remaining)

        pacer.drop_live();
        assert!(!pacer.is_live());
    }

    #[test]
    fn test_frame_delay() {
        let pacer = FramePacer::new(30, 60);

        // Frame took 10ms, target is ~33ms, should delay ~23ms
        let delay = pacer.frame_delay(Duration::from_millis(10));
        assert!(delay > Duration::from_millis(20));

        // Frame took longer than target, no delay
        let delay = pacer.frame_delay(Duration::from_millis(50));
        assert!(delay.is_zero());
    }
}
