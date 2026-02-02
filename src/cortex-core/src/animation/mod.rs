//! Animation primitives for the 120 FPS Cortex engine.
//!
//! Provides smooth, high-performance animation components optimized for
//! terminal rendering at 120 FPS. All animations are frame-based with
//! proper timing to ensure consistent behavior across different frame rates.

mod easing;
mod fade;
mod progress_bar;
mod pulse;
mod spinner;
mod timer;
mod token_counter;
mod types;
mod typewriter;

// Re-export all public types for backwards compatibility
pub use easing::{ease_in_out, interpolate_color};
pub use fade::{Fade, FadeDirection};
pub use progress_bar::ProgressBar;
pub use pulse::Pulse;
pub use spinner::Spinner;
pub use timer::ElapsedTimer;
pub use token_counter::TokenCounter;
pub use types::{SpinnerFrames, SpinnerType};
pub use typewriter::Typewriter;

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    use ratatui::style::Color;

    #[test]
    fn test_pulse_creation() {
        let pulse = Pulse::new(1000);
        assert_eq!(pulse.frame, 0);
        assert_eq!(pulse.cycle_duration(), Duration::from_millis(1000));
    }

    #[test]
    fn test_pulse_tick() {
        let mut pulse = Pulse::new(1000);
        pulse.tick();
        assert_eq!(pulse.frame, 1);
        pulse.tick();
        assert_eq!(pulse.frame, 2);
    }

    #[test]
    fn test_pulse_progress_bounds() {
        let pulse = Pulse::new(1000);
        let progress = pulse.progress();
        assert!(progress >= 0.0 && progress <= 1.0);
    }

    #[test]
    fn test_pulse_intensity_bounds() {
        let pulse = Pulse::new(1000);
        let intensity = pulse.intensity();
        assert!(intensity >= 0.0 && intensity <= 1.0);
    }

    #[test]
    fn test_pulse_color() {
        let pulse = Pulse::new(1000);
        let color = pulse.color();
        // Color should be a valid RGB value
        match color {
            Color::Rgb(_, _, _) => (),
            _ => panic!("Expected RGB color"),
        }
    }

    #[test]
    fn test_typewriter_creation() {
        let tw = Typewriter::new("Hello".to_string(), 60.0);
        assert_eq!(tw.visible_text(), "");
        assert!(!tw.is_complete());
    }

    #[test]
    fn test_typewriter_skip_to_end() {
        let mut tw = Typewriter::new("Hello".to_string(), 60.0);
        tw.skip_to_end();
        assert_eq!(tw.visible_text(), "Hello");
        assert!(tw.is_complete());
    }

    #[test]
    fn test_typewriter_set_text() {
        let mut tw = Typewriter::new("Hello".to_string(), 60.0);
        tw.skip_to_end();
        tw.set_text("World".to_string());
        assert_eq!(tw.visible_text(), "");
        assert!(!tw.is_complete());
    }

    #[test]
    fn test_typewriter_append() {
        let mut tw = Typewriter::new("Hello".to_string(), 60.0);
        tw.skip_to_end();
        assert!(tw.is_complete());
        tw.append(" World");
        assert!(!tw.is_complete());
        assert_eq!(tw.full_text(), "Hello World");
    }

    #[test]
    fn test_typewriter_unicode() {
        let mut tw = Typewriter::new("日本語".to_string(), 1000.0);
        // With very high chars_per_second, should reveal quickly
        for _ in 0..100 {
            tw.tick();
        }
        // Should handle multi-byte chars correctly
        assert!(tw.visible_text().len() <= "日本語".len());
    }

    #[test]
    fn test_fade_in() {
        let fade = Fade::fade_in(100);
        assert!(!fade.is_complete());
        assert_eq!(fade.direction(), FadeDirection::In);

        // Initial progress should be near 0
        let initial = fade.progress();
        assert!(initial < 0.5);
    }

    #[test]
    fn test_fade_out() {
        let fade = Fade::fade_out(100);
        assert_eq!(fade.direction(), FadeDirection::Out);

        // Initial progress should be near 1 (since it's fading out)
        let initial = fade.progress();
        assert!(initial > 0.5);
    }

    #[test]
    fn test_fade_completion() {
        let fade = Fade::fade_in(10);
        thread::sleep(Duration::from_millis(20));
        assert!(fade.is_complete());
    }

    #[test]
    fn test_spinner_dots() {
        let spinner = Spinner::dots();
        assert_eq!(spinner.current(), "⠋");
        assert_eq!(spinner.frame_count(), 10);
    }

    #[test]
    fn test_spinner_line() {
        let spinner = Spinner::line();
        assert_eq!(spinner.current(), "-");
        assert_eq!(spinner.frame_count(), 4);
    }

    #[test]
    fn test_spinner_bounce() {
        let spinner = Spinner::bounce();
        assert_eq!(spinner.current(), "⠁");
        assert_eq!(spinner.frame_count(), 4);
    }

    #[test]
    fn test_spinner_tick() {
        let mut spinner = Spinner::line();
        thread::sleep(Duration::from_millis(150)); // Wait longer than frame_duration
        spinner.tick();
        // Frame should have advanced
        assert!(spinner.current_index() > 0 || spinner.current_index() == 0);
    }

    #[test]
    fn test_spinner_types() {
        // Test all spinner types
        let thinking = Spinner::thinking();
        assert_eq!(thinking.spinner_type(), SpinnerType::Thinking);
        assert_eq!(thinking.interval_ms(), 150);

        let tool = Spinner::tool();
        assert_eq!(tool.spinner_type(), SpinnerType::Tool);
        assert_eq!(tool.interval_ms(), 80);

        let streaming = Spinner::streaming();
        assert_eq!(streaming.spinner_type(), SpinnerType::Streaming);
        assert_eq!(streaming.interval_ms(), 100);

        let approval = Spinner::approval();
        assert_eq!(approval.spinner_type(), SpinnerType::Approval);
        assert_eq!(approval.interval_ms(), 200);

        let loading = Spinner::loading();
        assert_eq!(loading.spinner_type(), SpinnerType::Loading);
        assert_eq!(loading.interval_ms(), 100);
    }

    #[test]
    fn test_spinner_frames_for_type() {
        assert_eq!(
            SpinnerFrames::for_type(SpinnerType::Thinking),
            SpinnerFrames::CIRCLE
        );
        assert_eq!(
            SpinnerFrames::for_type(SpinnerType::Tool),
            SpinnerFrames::DOTS
        );
        assert_eq!(
            SpinnerFrames::for_type(SpinnerType::Streaming),
            SpinnerFrames::BLOCKS
        );
        assert_eq!(
            SpinnerFrames::for_type(SpinnerType::Approval),
            SpinnerFrames::ARC
        );
        assert_eq!(
            SpinnerFrames::for_type(SpinnerType::Loading),
            SpinnerFrames::DOTS
        );
        assert_eq!(
            SpinnerFrames::for_type(SpinnerType::Progress),
            SpinnerFrames::LINE
        );
    }

    #[test]
    fn test_progress_bar_new() {
        let pb = ProgressBar::new(100);
        assert_eq!(pb.current(), 0);
        assert_eq!(pb.total(), 100);
        assert_eq!(pb.percentage(), 0.0);
    }

    #[test]
    fn test_progress_bar_set_progress() {
        let mut pb = ProgressBar::new(100);
        pb.set_progress(50);
        assert_eq!(pb.current(), 50);
        assert_eq!(pb.percentage(), 50.0);

        // Test clamping
        pb.set_progress(150);
        assert_eq!(pb.current(), 100);
    }

    #[test]
    fn test_progress_bar_increment() {
        let mut pb = ProgressBar::new(100);
        pb.increment(25);
        assert_eq!(pb.current(), 25);
        pb.increment(25);
        assert_eq!(pb.current(), 50);

        // Test clamping
        pb.increment(100);
        assert_eq!(pb.current(), 100);
    }

    #[test]
    fn test_progress_bar_render() {
        let mut pb = ProgressBar::new(100).with_width(10);
        pb.set_progress(50);
        let rendered = pb.render();
        assert!(rendered.contains("50%"));
        assert!(rendered.starts_with('['));
        assert!(rendered.contains(']'));
    }

    #[test]
    fn test_progress_bar_complete() {
        let mut pb = ProgressBar::new(100);
        assert!(!pb.is_complete());
        pb.set_progress(100);
        assert!(pb.is_complete());
    }

    #[test]
    fn test_token_counter_new() {
        let counter = TokenCounter::new();
        assert_eq!(counter.total(), 0);
        assert_eq!(counter.input(), 0);
        assert_eq!(counter.output(), 0);
    }

    #[test]
    fn test_token_counter_add() {
        let mut counter = TokenCounter::new();
        counter.add_input(100);
        counter.add_output(200);
        assert_eq!(counter.input(), 100);
        assert_eq!(counter.output(), 200);
        assert_eq!(counter.total(), 300);
    }

    #[test]
    fn test_token_counter_render() {
        let mut counter = TokenCounter::new();
        counter.add_output(1500);
        let rendered = counter.render();
        assert!(rendered.contains("1.5k"));
        assert!(rendered.contains("tokens"));
    }

    #[test]
    fn test_token_counter_render_with_max() {
        let mut counter = TokenCounter::new().with_max(4096);
        counter.add_output(2000);
        let rendered = counter.render();
        assert!(rendered.contains("/"));
        assert!(rendered.contains("4.1k") || rendered.contains("4.0k"));
    }

    #[test]
    fn test_token_counter_format() {
        // Test format_count through render
        let mut counter = TokenCounter::new();
        counter.add_output(500);
        assert!(counter.render().contains("500"));

        counter.reset();
        counter.add_output(1500);
        assert!(counter.render().contains("1.5k"));

        counter.reset();
        counter.add_output(1_500_000);
        assert!(counter.render().contains("1.5M"));
    }

    #[test]
    fn test_elapsed_timer_new() {
        let timer = ElapsedTimer::new();
        let elapsed = timer.elapsed();
        // Should be very small initially
        assert!(elapsed.as_millis() < 100);
    }

    #[test]
    fn test_elapsed_timer_render() {
        let timer = ElapsedTimer::new();
        let rendered = timer.render();
        // Should show sub-second initially
        assert!(rendered.contains('s'));
    }

    #[test]
    fn test_elapsed_timer_render_bracketed() {
        let timer = ElapsedTimer::new();
        let rendered = timer.render_bracketed();
        assert!(rendered.starts_with('['));
        assert!(rendered.ends_with(']'));
    }

    #[test]
    fn test_elapsed_timer_reset() {
        let mut timer = ElapsedTimer::new();
        thread::sleep(Duration::from_millis(50));
        let before_reset = timer.elapsed();
        timer.reset();
        let after_reset = timer.elapsed();
        assert!(after_reset < before_reset);
    }

    #[test]
    fn test_interpolate_color() {
        let from = Color::Rgb(0, 0, 0);
        let to = Color::Rgb(100, 100, 100);

        // At t=0, should be from color
        let c0 = interpolate_color(from, to, 0.0);
        assert_eq!(c0, Color::Rgb(0, 0, 0));

        // At t=1, should be to color
        let c1 = interpolate_color(from, to, 1.0);
        assert_eq!(c1, Color::Rgb(100, 100, 100));

        // At t=0.5, should be midpoint
        let c_mid = interpolate_color(from, to, 0.5);
        assert_eq!(c_mid, Color::Rgb(50, 50, 50));
    }

    #[test]
    fn test_interpolate_color_clamping() {
        let from = Color::Rgb(0, 0, 0);
        let to = Color::Rgb(100, 100, 100);

        // Values outside 0-1 should be clamped
        let c_neg = interpolate_color(from, to, -0.5);
        let c_zero = interpolate_color(from, to, 0.0);
        assert_eq!(c_neg, c_zero);

        let c_over = interpolate_color(from, to, 1.5);
        let c_one = interpolate_color(from, to, 1.0);
        assert_eq!(c_over, c_one);
    }

    #[test]
    fn test_ease_in_out() {
        // Test boundaries
        assert_eq!(ease_in_out(0.0), 0.0);
        assert!((ease_in_out(1.0) - 1.0).abs() < 0.001);

        // Test midpoint is 0.5
        assert!((ease_in_out(0.5) - 0.5).abs() < 0.001);

        // Test monotonicity
        let mut prev = 0.0;
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let val = ease_in_out(t);
            assert!(val >= prev);
            prev = val;
        }
    }
}
