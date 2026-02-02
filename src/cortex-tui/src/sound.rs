//! Sound notification system for the Cortex TUI.
//!
//! Provides audio notifications for key events like response completion,
//! tool approval requests, and spec plan approval.
//!
//! Audio playback is handled in a dedicated thread since rodio's OutputStream
//! is not Send/Sync. We use a channel-based approach to send sound requests
//! from any thread to the dedicated audio thread.

use std::io::{Cursor, Write};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::thread;

/// Type of sound notification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundType {
    /// Response/streaming completed
    ResponseComplete,
    /// Tool requires approval
    ApprovalRequired,
}

/// Channel sender for sound requests (Send + Sync)
/// Using sync_channel with capacity of 16 to prevent unbounded growth
static SOUND_SENDER: OnceLock<mpsc::SyncSender<SoundType>> = OnceLock::new();

/// Embedded WAV data for response complete sound
const COMPLETE_WAV: &[u8] = include_bytes!("sounds/complete.wav");
/// Embedded WAV data for approval required sound
const APPROVAL_WAV: &[u8] = include_bytes!("sounds/approval.wav");

/// Initialize the global sound system.
/// Spawns a dedicated audio thread that owns the OutputStream.
/// Should be called once at application startup.
pub fn init() {
    // Only initialize once
    if SOUND_SENDER.get().is_some() {
        return;
    }

    // Use bounded channel to prevent memory exhaustion from rapid triggers
    let (tx, rx) = mpsc::sync_channel::<SoundType>(16);

    // Store the sender globally
    if SOUND_SENDER.set(tx).is_err() {
        // Another thread beat us to initialization
        return;
    }

    // Spawn a dedicated audio thread with a descriptive name
    thread::Builder::new()
        .name("cortex-audio".to_string())
        .spawn(move || {
            // Try to create audio output
            let output = match rodio::OutputStream::try_default() {
                Ok((stream, handle)) => Some((stream, handle)),
                Err(e) => {
                    tracing::debug!("Failed to initialize audio output: {}", e);
                    None
                }
            };

            // Process sound requests
            while let Ok(sound_type) = rx.recv() {
                if let Some((ref _stream, ref handle)) = output {
                    let data: &'static [u8] = match sound_type {
                        SoundType::ResponseComplete => COMPLETE_WAV,
                        SoundType::ApprovalRequired => APPROVAL_WAV,
                    };

                    if let Err(e) = play_wav_internal(handle, data) {
                        tracing::debug!("Failed to play sound: {}", e);
                    }
                }
            }
        })
        .expect("Failed to spawn audio thread");
}

/// Internal function to play WAV data using a stream handle
fn play_wav_internal(
    handle: &rodio::OutputStreamHandle,
    data: &'static [u8],
) -> Result<(), String> {
    let cursor = Cursor::new(data);
    let source = rodio::Decoder::new(cursor).map_err(|e| format!("Decoder error: {}", e))?;
    let sink = rodio::Sink::try_new(handle).map_err(|e| format!("Sink error: {}", e))?;
    sink.append(source);
    sink.detach();
    Ok(())
}

/// Emit terminal bell as fallback, ensuring immediate output
fn emit_terminal_bell() {
    print!("\x07");
    // Flush stdout to ensure bell is emitted immediately (not buffered)
    let _ = std::io::stdout().flush();
}

/// Play a notification sound.
///
/// If `enabled` is false or audio is unavailable, this function does nothing.
/// Falls back to terminal bell if the sound system is not initialized.
/// This function is non-blocking - sound plays in background thread.
pub fn play(sound_type: SoundType, enabled: bool) {
    if !enabled {
        return;
    }

    // Try to send sound request to audio thread
    if let Some(sender) = SOUND_SENDER.get() {
        // Use try_send to avoid blocking if channel is full
        if sender.try_send(sound_type).is_err() {
            // Channel full or audio thread terminated, fall back to bell
            emit_terminal_bell();
        }
    } else {
        // Sound system not initialized, fall back to terminal bell
        emit_terminal_bell();
    }
}

/// Play notification for response completion
pub fn play_response_complete(enabled: bool) {
    play(SoundType::ResponseComplete, enabled);
}

/// Play notification for approval required
pub fn play_approval_required(enabled: bool) {
    play(SoundType::ApprovalRequired, enabled);
}

/// Check if the sound system has been initialized.
/// Useful for testing and diagnostics.
pub fn is_initialized() -> bool {
    SOUND_SENDER.get().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sound_type_equality() {
        assert_eq!(SoundType::ResponseComplete, SoundType::ResponseComplete);
        assert_eq!(SoundType::ApprovalRequired, SoundType::ApprovalRequired);
        assert_ne!(SoundType::ResponseComplete, SoundType::ApprovalRequired);
    }

    #[test]
    fn test_sound_type_debug() {
        let complete = SoundType::ResponseComplete;
        let approval = SoundType::ApprovalRequired;
        assert_eq!(format!("{:?}", complete), "ResponseComplete");
        assert_eq!(format!("{:?}", approval), "ApprovalRequired");
    }

    #[test]
    fn test_sound_type_clone() {
        let original = SoundType::ResponseComplete;
        let cloned = original;
        assert_eq!(original, cloned);
    }

    #[test]
    fn test_play_when_disabled() {
        // Should not panic when sound is disabled
        play(SoundType::ResponseComplete, false);
        play(SoundType::ApprovalRequired, false);
    }

    #[test]
    fn test_play_response_complete_disabled() {
        // Should not panic when sound is disabled
        play_response_complete(false);
    }

    #[test]
    fn test_play_approval_required_disabled() {
        // Should not panic when sound is disabled
        play_approval_required(false);
    }

    #[test]
    fn test_embedded_wav_data_not_empty() {
        // Verify that the embedded WAV files are not empty
        assert!(!COMPLETE_WAV.is_empty(), "complete.wav should not be empty");
        assert!(!APPROVAL_WAV.is_empty(), "approval.wav should not be empty");
    }

    #[test]
    fn test_embedded_wav_data_has_riff_header() {
        // WAV files should start with "RIFF" magic bytes
        assert!(
            COMPLETE_WAV.starts_with(b"RIFF"),
            "complete.wav should have RIFF header"
        );
        assert!(
            APPROVAL_WAV.starts_with(b"RIFF"),
            "approval.wav should have RIFF header"
        );
    }
}
