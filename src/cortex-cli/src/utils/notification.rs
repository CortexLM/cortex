//! Desktop notification utilities for the Cortex CLI.
//!
//! Provides cross-platform desktop notification functionality.

use anyhow::Result;

/// Notification urgency level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotificationUrgency {
    /// Low urgency (informational).
    Low,
    /// Normal urgency (default).
    #[default]
    Normal,
    /// Critical urgency (errors, failures).
    Critical,
}

/// Send a desktop notification.
///
/// Uses platform-specific notification mechanisms:
/// - macOS: AppleScript `display notification`
/// - Linux: `notify-send` command
/// - Windows: PowerShell Toast notifications
///
/// # Arguments
/// * `title` - The notification title
/// * `body` - The notification body text
/// * `urgency` - The urgency level (affects visual styling on some platforms)
///
/// # Returns
/// `Ok(())` on success (or silent failure if notifications unavailable).
pub fn send_notification(title: &str, body: &str, urgency: NotificationUrgency) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        send_notification_macos(title, body)?;
    }

    #[cfg(target_os = "linux")]
    {
        send_notification_linux(title, body, urgency)?;
    }

    #[cfg(target_os = "windows")]
    {
        send_notification_windows(title, body)?;
    }

    Ok(())
}

/// Send a task completion notification.
///
/// Convenience function for notifying about task completion status.
///
/// # Arguments
/// * `session_id` - The session identifier (will be truncated for display)
/// * `success` - Whether the task completed successfully
pub fn send_task_notification(session_id: &str, success: bool) -> Result<()> {
    let title = if success {
        "Cortex Task Completed"
    } else {
        "Cortex Task Failed"
    };

    // Use safe UTF-8 slicing - find the last valid char boundary at or before position 8
    let short_id = session_id
        .char_indices()
        .take_while(|(idx, _)| *idx < 8)
        .map(|(idx, ch)| idx + ch.len_utf8())
        .last()
        .and_then(|end| session_id.get(..end))
        .unwrap_or(session_id);
    let body = format!("Session: {}", short_id);

    let urgency = if success {
        NotificationUrgency::Normal
    } else {
        NotificationUrgency::Critical
    };

    send_notification(title, &body, urgency)
}

#[cfg(target_os = "macos")]
fn send_notification_macos(title: &str, body: &str) -> Result<()> {
    use std::process::Command;

    // Escape special characters for AppleScript
    let title_escaped = title.replace('"', "\\\"");
    let body_escaped = body.replace('"', "\\\"");

    let script = format!(
        r#"display notification "{}" with title "{}""#,
        body_escaped, title_escaped
    );

    // Fire and forget - don't fail if notification fails
    let _ = Command::new("osascript").args(["-e", &script]).output();

    Ok(())
}

#[cfg(target_os = "linux")]
fn send_notification_linux(title: &str, body: &str, urgency: NotificationUrgency) -> Result<()> {
    use std::process::Command;

    let urgency_str = match urgency {
        NotificationUrgency::Low => "low",
        NotificationUrgency::Normal => "normal",
        NotificationUrgency::Critical => "critical",
    };

    // Fire and forget
    let _ = Command::new("notify-send")
        .args(["--urgency", urgency_str, title, body])
        .output();

    Ok(())
}

#[cfg(target_os = "windows")]
fn send_notification_windows(title: &str, body: &str) -> Result<()> {
    use std::process::Command;

    // Escape special characters for PowerShell
    let title_escaped = title.replace('"', "`\"");
    let body_escaped = body.replace('"', "`\"");

    let script = format!(
        r#"
        [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
        $template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
        $textNodes = $template.GetElementsByTagName("text")
        $textNodes.Item(0).AppendChild($template.CreateTextNode("{}")) | Out-Null
        $textNodes.Item(1).AppendChild($template.CreateTextNode("{}")) | Out-Null
        $toast = [Windows.UI.Notifications.ToastNotification]::new($template)
        [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("Cortex").Show($toast)
        "#,
        title_escaped, body_escaped
    );

    // Fire and forget
    let _ = Command::new("powershell")
        .args(["-Command", &script])
        .output();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_urgency_default() {
        let urgency = NotificationUrgency::default();
        assert_eq!(urgency, NotificationUrgency::Normal);
    }
}
