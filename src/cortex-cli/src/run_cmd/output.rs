//! Output handling utilities: clipboard, notifications, and formatting.

use anyhow::{Context, Result};
use std::io::Write;

/// Copy text to system clipboard.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};
        let mut child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .context("Failed to spawn pbcopy")?;

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::{Command, Stdio};
        // Try xclip first, then xsel
        let result = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(Stdio::piped())
            .spawn();

        let mut child = match result {
            Ok(c) => c,
            Err(_) => Command::new("xsel")
                .args(["--clipboard", "--input"])
                .stdin(Stdio::piped())
                .spawn()
                .context("Failed to spawn clipboard command (tried xclip and xsel)")?,
        };

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::{Command, Stdio};
        let mut child = Command::new("clip")
            .stdin(Stdio::piped())
            .spawn()
            .context("Failed to spawn clip.exe")?;

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("Clipboard not supported on this platform")
    }
}

/// Send a desktop notification.
pub fn send_notification(session_id: &str, success: bool) -> Result<()> {
    let title = if success {
        "Cortex Task Completed"
    } else {
        "Cortex Task Failed"
    };

    let body = format!("Session: {}", &session_id[..8.min(session_id.len())]);

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let script = format!(
            r#"display notification "{}" with title "{}""#,
            body.replace('"', "\\\""),
            title.replace('"', "\\\"")
        );
        let _ = Command::new("osascript").args(["-e", &script]).output();
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let _ = Command::new("notify-send").args([title, &body]).output();
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        // Use PowerShell for Windows notifications
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
            title.replace('"', "`\""),
            body.replace('"', "`\"")
        );
        let _ = Command::new("powershell")
            .args(["-Command", &script])
            .output();
    }

    Ok(())
}
