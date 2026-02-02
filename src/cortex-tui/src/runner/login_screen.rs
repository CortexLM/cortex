//! Login Screen - Full-screen TUI
//!
//! Full-screen login screen using ratatui and alternate screen buffer for reliable
//! rendering across all terminal emulators.

use std::io::stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use tokio::sync::mpsc;

use cortex_login::{SecureAuthData, save_auth_with_fallback};
use cortex_tui_components::spinner::SpinnerStyle;

// ============================================================================
// Constants
// ============================================================================

const API_BASE_URL: &str = "https://api.cortex.foundation";
const AUTH_BASE_URL: &str = "https://auth.cortex.foundation";

// Colors matching the original design
const PRIMARY: Color = Color::Rgb(0x00, 0xFF, 0xA3);
const DIM: Color = Color::Rgb(0x6b, 0x6b, 0x7b);
const CYAN: Color = Color::Cyan;

// ============================================================================
// Login Screen State
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginState {
    SelectMethod,
    WaitingForAuth,
    Success,
    Failed,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginMethod {
    CortexAccount,
    ApiKey,
    Exit,
}

impl LoginMethod {
    fn all() -> &'static [LoginMethod] {
        &[
            LoginMethod::CortexAccount,
            LoginMethod::ApiKey,
            LoginMethod::Exit,
        ]
    }

    fn label(&self) -> &'static str {
        match self {
            LoginMethod::CortexAccount => "Cortex Foundation account",
            LoginMethod::ApiKey => "API Key",
            LoginMethod::Exit => "Exit",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            LoginMethod::CortexAccount => "Pro, Max, Scale, or Enterprise subscription",
            LoginMethod::ApiKey => "For API access without subscription",
            LoginMethod::Exit => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginResult {
    LoggedIn,
    ContinueWithApiKey,
    Exit,
    Failed(String),
}

// ============================================================================
// Async Messages
// ============================================================================

#[derive(Debug)]
enum AsyncMessage {
    DeviceCodeReceived {
        user_code: String,
        device_code: String,
        #[allow(dead_code)]
        verification_uri: String,
    },
    DeviceCodeError(String),
    TokenReceived,
    TokenError(String),
}

// ============================================================================
// Login Screen
// ============================================================================

pub struct LoginScreen {
    state: LoginState,
    selected_method: usize,
    frame_count: u64,
    error_message: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    cortex_home: PathBuf,
    #[allow(dead_code)]
    message: Option<String>,
    async_rx: Option<mpsc::Receiver<AsyncMessage>>,
    copied_notification: Option<Instant>,
}

impl LoginScreen {
    pub fn new(cortex_home: PathBuf, message: Option<String>) -> Self {
        Self {
            state: LoginState::SelectMethod,
            selected_method: 0,
            frame_count: 0,
            error_message: None,
            user_code: None,
            verification_uri: None,
            cortex_home,
            message,
            async_rx: None,
            copied_notification: None,
        }
    }

    pub async fn run(&mut self) -> Result<LoginResult> {
        // Enter alternate screen mode for reliable rendering
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = stdout();
        crossterm::execute!(
            stdout,
            crossterm::terminal::EnterAlternateScreen,
            crossterm::event::EnableMouseCapture,
        )?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let result = self.run_loop(&mut terminal).await;

        // Cleanup - leave alternate screen
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(
            terminal.backend_mut(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
        )?;
        terminal.show_cursor()?;

        result
    }

    async fn run_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<LoginResult> {
        // Create an async event stream - this is crucial for non-blocking event handling
        // that allows the tokio runtime to process async messages concurrently
        let mut event_stream = EventStream::new();

        // Small delay to let terminal settle
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Timer for UI updates (60fps refresh rate)
        let mut render_interval = tokio::time::interval(Duration::from_millis(16));

        loop {
            self.frame_count = self.frame_count.wrapping_add(1);

            // Clear copied notification after 2 seconds
            if let Some(notif_time) = self.copied_notification
                && notif_time.elapsed() > Duration::from_secs(2)
            {
                self.copied_notification = None;
            }

            // Render
            terminal.draw(|f| self.render(f))?;

            // Check state before waiting for events
            match self.state {
                LoginState::Success => {
                    return Ok(LoginResult::LoggedIn);
                }
                LoginState::Exit => {
                    return Ok(LoginResult::Exit);
                }
                LoginState::Failed => {
                    let msg = self.error_message.clone().unwrap_or_default();
                    return Ok(LoginResult::Failed(msg));
                }
                _ => {}
            }

            // Use tokio::select! to concurrently wait for:
            // 1. Terminal events (keyboard input)
            // 2. Async messages from background tasks (token polling)
            // 3. Render timer tick
            // This prevents blocking the async runtime and ensures responsive UI
            tokio::select! {
                // Handle keyboard/terminal events
                maybe_event = event_stream.next() => {
                    if let Some(Ok(Event::Key(key))) = maybe_event
                        && key.kind == crossterm::event::KeyEventKind::Press
                        && let Some(result) = self.handle_key(key)
                    {
                        return Ok(result);
                    }
                }
                
                // Handle async messages from token polling
                msg = async {
                    if let Some(ref mut rx) = self.async_rx {
                        rx.recv().await
                    } else {
                        // No receiver, wait forever (will be cancelled by other branches)
                        std::future::pending::<Option<AsyncMessage>>().await
                    }
                } => {
                    if let Some(msg) = msg {
                        self.handle_async_message(msg);
                    }
                }
                
                // Periodic render tick to keep UI responsive
                _ = render_interval.tick() => {
                    // Just continue to re-render
                }
            }
        }
    }

    fn render(&self, f: &mut ratatui::Frame) {
        let area = f.area();
        f.render_widget(Clear, area);

        match self.state {
            LoginState::SelectMethod => self.render_select_method(f, area),
            LoginState::WaitingForAuth => self.render_waiting(f, area),
            _ => {}
        }
    }

    fn render_select_method(&self, f: &mut ratatui::Frame, area: Rect) {
        let version = env!("CARGO_PKG_VERSION");

        // Center the content
        let content_width = 70.min(area.width.saturating_sub(4));
        let content_height = 14;
        let content_x = (area.width.saturating_sub(content_width)) / 2;
        let content_y = (area.height.saturating_sub(content_height)) / 2;
        let content_area = Rect::new(content_x, content_y, content_width, content_height);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Separator
                Constraint::Length(1), // Welcome message
                Constraint::Length(1), // Description
                Constraint::Length(1), // Empty
                Constraint::Length(1), // Select header
                Constraint::Length(1), // Empty
                Constraint::Length(3), // Method options
                Constraint::Length(1), // Empty
                Constraint::Length(1), // Hints
                Constraint::Length(1), // Error message (if any)
            ])
            .split(content_area);

        // Line 1: Separator
        let separator =
            Paragraph::new("────────────────────────────────────────────────────────────")
                .style(Style::default().fg(DIM));
        f.render_widget(separator, chunks[0]);

        // Line 2: Welcome message
        let welcome = Paragraph::new(Line::from(vec![
            Span::styled(
                "Welcome to Cortex CLI",
                Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" v{}", version), Style::default().fg(DIM)),
        ]));
        f.render_widget(welcome, chunks[1]);

        // Line 3: Description
        let description =
            Paragraph::new("Cortex can be used with your Cortex Foundation account or API key.")
                .style(Style::default().fg(DIM));
        f.render_widget(description, chunks[2]);

        // Line 5: Select header
        let header = Paragraph::new(" Select login method:");
        f.render_widget(header, chunks[4]);

        // Lines 7-9: Method options
        let mut lines: Vec<Line> = Vec::new();
        for (i, method) in LoginMethod::all().iter().enumerate() {
            let is_selected = i == self.selected_method;
            let prefix = if is_selected { " › " } else { "   " };

            let mut spans = vec![
                Span::styled(
                    format!("{}{}. ", prefix, i + 1),
                    Style::default().fg(if is_selected { PRIMARY } else { DIM }),
                ),
                Span::styled(
                    method.label(),
                    Style::default().fg(if is_selected { PRIMARY } else { Color::White }),
                ),
            ];

            let desc = method.description();
            if !desc.is_empty() {
                spans.push(Span::styled(
                    format!(" · {}", desc),
                    Style::default().fg(DIM),
                ));
            }

            lines.push(Line::from(spans));
        }
        let options = Paragraph::new(lines);
        f.render_widget(options, chunks[6]);

        // Line 11: Hints
        let hints = Paragraph::new("↑↓ to select · Enter to confirm · Ctrl+C to exit")
            .style(Style::default().fg(DIM));
        f.render_widget(hints, chunks[8]);

        // Line 12: Error message (if any)
        if let Some(ref error) = self.error_message {
            let error_msg =
                Paragraph::new(format!("Error: {}", error)).style(Style::default().fg(Color::Red));
            f.render_widget(error_msg, chunks[9]);
        }
    }

    fn render_waiting(&self, f: &mut ratatui::Frame, area: Rect) {
        let version = env!("CARGO_PKG_VERSION");
        let breathing = SpinnerStyle::Breathing.frames();
        let spinner = breathing[(self.frame_count % breathing.len() as u64) as usize];

        // Build direct auth URL
        let direct_url = if let Some(ref code) = self.user_code {
            format!("{}/device?code={}", AUTH_BASE_URL, code)
        } else {
            format!("{}/device", AUTH_BASE_URL)
        };

        // Center the content
        let content_width = 70.min(area.width.saturating_sub(4));
        let content_height = 14;
        let content_x = (area.width.saturating_sub(content_width)) / 2;
        let content_y = (area.height.saturating_sub(content_height)) / 2;
        let content_area = Rect::new(content_x, content_y, content_width, content_height);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Welcome message
                Constraint::Length(1), // Empty
                Constraint::Length(1), // Mascot top
                Constraint::Length(1), // Mascot + waiting message
                Constraint::Length(1), // Mascot bottom
                Constraint::Length(1), // Mascot legs
                Constraint::Length(1), // Empty
                Constraint::Length(1), // Browser message
                Constraint::Length(1), // URL
                Constraint::Length(1), // Empty
                Constraint::Length(1), // Hints
            ])
            .split(content_area);

        // Line 1: Welcome message
        let welcome = Paragraph::new(Line::from(vec![
            Span::styled(
                "Welcome to Cortex CLI",
                Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" v{}", version), Style::default().fg(DIM)),
        ]));
        f.render_widget(welcome, chunks[0]);

        // Line 3: Mascot top
        let mascot_top = Paragraph::new(" ▄█▀▀▀▀█▄ ").style(Style::default().fg(PRIMARY));
        f.render_widget(mascot_top, chunks[2]);

        // Line 4: Mascot + waiting message
        let mascot_middle = Paragraph::new(Line::from(vec![
            Span::styled("██ ▌  ▐ ██  ", Style::default().fg(PRIMARY)),
            Span::styled(
                format!("Waiting for browser authentication  {}", spinner),
                Style::default().fg(PRIMARY),
            ),
        ]));
        f.render_widget(mascot_middle, chunks[3]);

        // Line 5: Mascot bottom
        let mascot_bottom = Paragraph::new(" █▄▄▄▄▄▄█ ").style(Style::default().fg(PRIMARY));
        f.render_widget(mascot_bottom, chunks[4]);

        // Line 6: Mascot legs
        let mascot_legs = Paragraph::new("  █    █").style(Style::default().fg(PRIMARY));
        f.render_widget(mascot_legs, chunks[5]);

        // Line 8: Browser message
        let copy_hint = if self.copied_notification.is_some() {
            "(✓ Copied!)"
        } else {
            "(c to copy)"
        };
        let browser_msg = Paragraph::new(Line::from(vec![
            Span::styled(
                "Browser didn't open? Click the URL below ",
                Style::default().fg(DIM),
            ),
            Span::styled(copy_hint, Style::default().fg(DIM)),
        ]));
        f.render_widget(browser_msg, chunks[7]);

        // Line 9: URL
        let url_line = Paragraph::new(&*direct_url).style(Style::default().fg(CYAN));
        f.render_widget(url_line, chunks[8]);

        // Line 11: Hints
        let hints =
            Paragraph::new("Esc to go back · Ctrl+C to exit").style(Style::default().fg(DIM));
        f.render_widget(hints, chunks[10]);
    }

    fn get_direct_url(&self) -> String {
        if let Some(ref code) = self.user_code {
            format!("{}/device?code={}", AUTH_BASE_URL, code)
        } else {
            format!("{}/device", AUTH_BASE_URL)
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<LoginResult> {
        // Ctrl+C quits from anywhere
        if key.code == KeyCode::Char('c')
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            return Some(LoginResult::Exit);
        }

        match self.state {
            LoginState::SelectMethod => self.handle_select_method_key(key),
            LoginState::WaitingForAuth => self.handle_waiting_key(key),
            _ => None,
        }
    }

    fn handle_select_method_key(&mut self, key: KeyEvent) -> Option<LoginResult> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_method > 0 {
                    self.selected_method -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected_method < LoginMethod::all().len() - 1 {
                    self.selected_method += 1;
                }
            }
            KeyCode::Enter => {
                return self.select_method();
            }
            KeyCode::Char('1') => {
                self.selected_method = 0;
                return self.select_method();
            }
            KeyCode::Char('2') => {
                self.selected_method = 1;
                return self.select_method();
            }
            KeyCode::Char('3') => {
                self.selected_method = 2;
                return self.select_method();
            }
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                return Some(LoginResult::Exit);
            }
            _ => {}
        }
        None
    }

    fn select_method(&mut self) -> Option<LoginResult> {
        match LoginMethod::all()[self.selected_method] {
            LoginMethod::CortexAccount => {
                self.start_device_code_flow();
                None
            }
            LoginMethod::ApiKey => Some(LoginResult::ContinueWithApiKey),
            LoginMethod::Exit => Some(LoginResult::Exit),
        }
    }

    fn handle_waiting_key(&mut self, key: KeyEvent) -> Option<LoginResult> {
        match key.code {
            KeyCode::Esc => {
                // Cancel the current auth flow and go back to method selection
                self.state = LoginState::SelectMethod;
                self.error_message = None;
                self.user_code = None;
                self.verification_uri = None;
                // Drop the receiver to signal the async task it can stop
                // (the task will get a send error and terminate)
                self.async_rx = None;
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                // Only handle 'c' for copy if NOT Ctrl+C (Ctrl+C is handled in handle_key)
                // Copy URL to clipboard using the safe clipboard function
                // This properly handles Linux (with wait()) and Windows clipboard behavior
                let url = self.get_direct_url();
                if super::terminal::safe_clipboard_copy(&url) {
                    self.copied_notification = Some(Instant::now());
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                // Also allow 'q' to exit from waiting screen for better UX
                return Some(LoginResult::Exit);
            }
            _ => {}
        }
        None
    }

    fn start_device_code_flow(&mut self) {
        self.state = LoginState::WaitingForAuth;
        self.error_message = None;
        self.user_code = None;
        self.verification_uri = None;

        let tx = self.create_async_channel();
        tokio::spawn(async move {
            request_device_code_async(tx).await;
        });
    }

    fn create_async_channel(&mut self) -> mpsc::Sender<AsyncMessage> {
        let (tx, rx) = mpsc::channel(16);
        self.async_rx = Some(rx);
        tx
    }

    fn handle_async_message(&mut self, msg: AsyncMessage) {
        match msg {
            AsyncMessage::DeviceCodeReceived {
                user_code,
                device_code,
                verification_uri: _,
            } => {
                tracing::info!("Device code received: {}", user_code);
                let auth_url = format!("{}/device", AUTH_BASE_URL);
                self.user_code = Some(user_code.clone());
                self.verification_uri = Some(auth_url.clone());

                // Open browser
                let link_url = format!("{}?code={}", auth_url, user_code);
                tracing::debug!("Opening browser to: {}", link_url);
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("open")
                        .arg(&link_url)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = std::process::Command::new("xdg-open")
                        .arg(&link_url)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
                #[cfg(target_os = "windows")]
                {
                    let _ = std::process::Command::new("cmd")
                        .args(["/C", "start", "", &link_url])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }

                // Start token polling - create new channel for this phase
                tracing::debug!("Starting token polling for device code");
                let cortex_home = self.cortex_home.clone();
                let tx = self.create_async_channel();
                tokio::spawn(async move {
                    poll_for_token_async(cortex_home, device_code, tx).await;
                });
            }
            AsyncMessage::DeviceCodeError(e) => {
                tracing::error!("Device code error: {}", e);
                self.state = LoginState::SelectMethod;
                self.error_message = Some(e);
            }
            AsyncMessage::TokenReceived => {
                tracing::info!("Authentication token received - login successful");
                self.state = LoginState::Success;
            }
            AsyncMessage::TokenError(e) => {
                tracing::error!("Token error: {}", e);
                self.state = LoginState::SelectMethod;
                self.error_message = Some(e);
            }
        }
    }
}

// ============================================================================
// Async Functions
// ============================================================================

async fn request_device_code_async(tx: mpsc::Sender<AsyncMessage>) {
    let client = match cortex_engine::create_default_client() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(AsyncMessage::DeviceCodeError(e.to_string())).await;
            return;
        }
    };

    let response = match client
        .post(format!("{}/auth/device/code", API_BASE_URL))
        .json(&serde_json::json!({
            "device_name": hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "Cortex CLI".to_string()),
            "scopes": ["chat", "models"]
        }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let _ = tx
                .send(AsyncMessage::DeviceCodeError(format!(
                    "Network error: {}",
                    e
                )))
                .await;
            return;
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        let error = if status.as_u16() == 403 {
            "Cannot connect to Cortex API. Service may be unavailable.".to_string()
        } else if status.as_u16() == 429 {
            "Too many login attempts. Please wait.".to_string()
        } else {
            format!("API error ({}): {}", status, body)
        };

        let _ = tx.send(AsyncMessage::DeviceCodeError(error)).await;
        return;
    }

    #[derive(serde::Deserialize)]
    struct DeviceCodeResponse {
        user_code: String,
        device_code: String,
        verification_uri: String,
    }

    match response.json::<DeviceCodeResponse>().await {
        Ok(data) => {
            let _ = tx
                .send(AsyncMessage::DeviceCodeReceived {
                    user_code: data.user_code,
                    device_code: data.device_code,
                    verification_uri: data.verification_uri,
                })
                .await;
        }
        Err(e) => {
            let _ = tx
                .send(AsyncMessage::DeviceCodeError(format!("Parse error: {}", e)))
                .await;
        }
    }
}

async fn poll_for_token_async(
    cortex_home: PathBuf,
    device_code: String,
    tx: mpsc::Sender<AsyncMessage>,
) {
    tracing::debug!("Token polling started");
    
    let client = match cortex_engine::create_default_client() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to create HTTP client: {}", e);
            let _ = tx.send(AsyncMessage::TokenError(e.to_string())).await;
            return;
        }
    };

    let interval = Duration::from_secs(5);
    let max_attempts = 180; // 15 minutes total

    for attempt in 0..max_attempts {
        tokio::time::sleep(interval).await;

        // Check if the receiver was dropped (user cancelled)
        // This is a cheap check that allows us to exit early
        if tx.is_closed() {
            tracing::debug!("Token polling cancelled (receiver dropped)");
            return;
        }

        tracing::trace!("Polling for token (attempt {}/{})", attempt + 1, max_attempts);

        let response = match client
            .post(format!("{}/auth/device/token", API_BASE_URL))
            .json(&serde_json::json!({ "device_code": device_code }))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("Token poll request failed: {}", e);
                continue;
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if status.is_success() {
            tracing::debug!("Token response received (success)");
            
            #[derive(serde::Deserialize)]
            struct TokenResponse {
                access_token: String,
                refresh_token: String,
            }

            if let Ok(token) = serde_json::from_str::<TokenResponse>(&body) {
                let expires_at = chrono::Utc::now().timestamp() + 3600;
                let auth_data = SecureAuthData::with_oauth(
                    token.access_token,
                    Some(token.refresh_token),
                    Some(expires_at),
                );

                match save_auth_with_fallback(&cortex_home, &auth_data) {
                    Ok(mode) => {
                        tracing::info!("Auth credentials saved using {:?} storage", mode);
                        // Send the success message - this is the critical moment
                        if let Err(e) = tx.send(AsyncMessage::TokenReceived).await {
                            tracing::error!("Failed to send TokenReceived message: {}", e);
                        } else {
                            tracing::debug!("TokenReceived message sent successfully");
                        }
                        return;
                    }
                    Err(e) => {
                        tracing::error!("Failed to save auth credentials: {}", e);
                        let _ = tx
                            .send(AsyncMessage::TokenError(format!(
                                "Failed to save credentials: {}",
                                e
                            )))
                            .await;
                        return;
                    }
                }
            } else {
                tracing::warn!("Failed to parse token response");
            }
            continue;
        }

        if let Ok(error) = serde_json::from_str::<serde_json::Value>(&body)
            && let Some(err) = error.get("error").and_then(|e| e.as_str())
        {
            match err {
                "authorization_pending" => {
                    tracing::trace!("Authorization pending...");
                    continue;
                }
                "slow_down" => {
                    tracing::debug!("Server requested slow down");
                    continue;
                }
                "expired_token" => {
                    tracing::warn!("Device code expired");
                    let _ = tx
                        .send(AsyncMessage::TokenError("Device code expired".to_string()))
                        .await;
                    return;
                }
                "access_denied" => {
                    tracing::warn!("Access denied by user");
                    let _ = tx
                        .send(AsyncMessage::TokenError("Access denied".to_string()))
                        .await;
                    return;
                }
                _ => {
                    tracing::debug!("Unknown error response: {}", err);
                }
            }
        }
    }

    tracing::warn!("Token polling timed out after {} attempts", max_attempts);
    let _ = tx
        .send(AsyncMessage::TokenError(
            "Authentication timed out".to_string(),
        ))
        .await;
}
