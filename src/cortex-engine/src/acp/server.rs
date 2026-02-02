//! ACP Server implementation.
//!
//! Provides both stdio and HTTP transports for the ACP protocol.
//! The stdio transport is used for local IDE integration (like Zed),
//! while HTTP enables remote connections and web-based clients.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info};

use crate::acp::handler::{AcpHandler, AcpNotificationEvent};
use crate::acp::protocol::{AcpError, AcpNotification, AcpRequest, AcpRequestId, AcpResponse};
use crate::config::Config;

/// ACP Server supporting both stdio and HTTP transports.
#[allow(dead_code)]
pub struct AcpServer {
    /// Request handler.
    handler: Arc<AcpHandler>,
    /// Configuration.
    config: Config,
}

impl AcpServer {
    /// Create a new ACP server.
    pub fn new(config: Config) -> Self {
        let handler = Arc::new(AcpHandler::new(config.clone()));
        Self { handler, config }
    }

    /// Run the server with stdio transport.
    ///
    /// This reads JSON-RPC requests from stdin and writes responses to stdout.
    /// Notifications are also written to stdout.
    pub async fn run_stdio(&self) -> Result<()> {
        info!("Starting ACP server on stdio transport");

        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        // Spawn notification forwarder
        let notification_rx = self.handler.subscribe();
        tokio::spawn(Self::forward_notifications_to_stdio(notification_rx));

        while reader.read_line(&mut line).await? > 0 {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                line.clear();
                continue;
            }

            debug!("Received request: {}", trimmed);

            let request: AcpRequest = match serde_json::from_str(trimmed) {
                Ok(req) => req,
                Err(e) => {
                    let err_response = AcpResponse::error(
                        AcpRequestId::Number(0),
                        AcpError::parse_error(e.to_string()),
                    );
                    Self::write_to_stdout(&err_response).await?;
                    line.clear();
                    continue;
                }
            };

            let response = self
                .handler
                .process_request(
                    request.id.clone(),
                    &request.method,
                    request.params.unwrap_or(Value::Null),
                )
                .await;

            Self::write_to_stdout(&response).await?;
            line.clear();
        }

        Ok(())
    }

    /// Forward notifications to stdout.
    async fn forward_notifications_to_stdio(
        mut rx: tokio::sync::broadcast::Receiver<AcpNotificationEvent>,
    ) {
        while let Ok(event) = rx.recv().await {
            let notification = AcpNotification::new(&event.method).with_params(event.params);
            if let Err(e) = Self::write_to_stdout(&notification).await {
                error!("Error writing notification: {}", e);
            }
        }
    }

    /// Write a serializable value to stdout as JSON.
    async fn write_to_stdout<T: Serialize>(value: &T) -> Result<()> {
        let mut json = serde_json::to_vec(value)?;
        json.push(b'\n');
        let mut stdout = tokio::io::stdout();
        stdout.write_all(&json).await?;
        stdout.flush().await?;
        Ok(())
    }

    /// Run the server with HTTP transport.
    ///
    /// This creates an HTTP server that accepts JSON-RPC requests
    /// and streams notifications via Server-Sent Events (SSE).
    pub async fn run_http(&self, addr: SocketAddr) -> Result<()> {
        info!("Starting ACP server on http://{}", addr);

        // Create a simple HTTP server using tokio's TCP listener
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let handler = self.handler.clone();

        loop {
            let (stream, peer_addr) = listener.accept().await?;
            debug!("New connection from {}", peer_addr);

            let handler = handler.clone();
            tokio::spawn(async move {
                if let Err(e) = Self::handle_http_connection(stream, handler).await {
                    error!("HTTP connection error: {}", e);
                }
            });
        }
    }

    /// Handle an HTTP connection.
    async fn handle_http_connection(
        mut stream: tokio::net::TcpStream,
        handler: Arc<AcpHandler>,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;

        let mut buffer = vec![0u8; 8192];
        let n = stream.read(&mut buffer).await?;

        if n == 0 {
            return Ok(());
        }

        let request_str = String::from_utf8_lossy(&buffer[..n]);
        let lines: Vec<&str> = request_str.lines().collect();

        // Parse HTTP request
        let first_line = lines.first().unwrap_or(&"");
        let parts: Vec<&str> = first_line.split_whitespace().collect();

        if parts.len() < 3 {
            Self::send_http_error(&mut stream, 400, "Bad Request").await?;
            return Ok(());
        }

        let method = parts[0];
        let path = parts[1];

        match (method, path) {
            ("POST", "/rpc") | ("POST", "/acp/rpc") | ("POST", "/") => {
                // Find the body (after empty line)
                let body_start = request_str
                    .find("\r\n\r\n")
                    .or_else(|| request_str.find("\n\n"));
                let body = body_start
                    .map(|i| {
                        let skip = if request_str[i..].starts_with("\r\n\r\n") {
                            4
                        } else {
                            2
                        };
                        &request_str[i + skip..]
                    })
                    .unwrap_or("");

                let request: AcpRequest = match serde_json::from_str(body.trim()) {
                    Ok(req) => req,
                    Err(e) => {
                        let err_response = AcpResponse::error(
                            AcpRequestId::Number(0),
                            AcpError::parse_error(e.to_string()),
                        );
                        Self::send_http_json(&mut stream, 200, &err_response).await?;
                        return Ok(());
                    }
                };

                let response = handler
                    .process_request(
                        request.id.clone(),
                        &request.method,
                        request.params.unwrap_or(Value::Null),
                    )
                    .await;

                Self::send_http_json(&mut stream, 200, &response).await?;
            }
            ("GET", "/events") | ("GET", "/acp/events") => {
                // Server-Sent Events stream
                Self::handle_sse_stream(&mut stream, handler).await?;
            }
            ("GET", "/health") => {
                let health = serde_json::json!({
                    "status": "ok",
                    "version": env!("CARGO_PKG_VERSION"),
                });
                Self::send_http_json(&mut stream, 200, &health).await?;
            }
            ("OPTIONS", _) => {
                // CORS preflight
                Self::send_http_cors(&mut stream).await?;
            }
            _ => {
                Self::send_http_error(&mut stream, 404, "Not Found").await?;
            }
        }

        Ok(())
    }

    /// Handle SSE stream.
    async fn handle_sse_stream(
        stream: &mut tokio::net::TcpStream,
        handler: Arc<AcpHandler>,
    ) -> Result<()> {
        // Send SSE headers
        let headers = "HTTP/1.1 200 OK\r\n\
            Content-Type: text/event-stream\r\n\
            Cache-Control: no-cache\r\n\
            Connection: keep-alive\r\n\
            Access-Control-Allow-Origin: *\r\n\
            \r\n";
        stream.write_all(headers.as_bytes()).await?;

        let mut rx = handler.subscribe();

        // Keep connection alive and forward events
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            let data = serde_json::to_string(&serde_json::json!({
                                "method": event.method,
                                "params": event.params,
                            }))?;
                            let sse_msg = format!("data: {}\n\n", data);
                            if stream.write_all(sse_msg.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            // Catch up
                            continue;
                        }
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    // Send keepalive
                    if stream.write_all(b": keepalive\n\n").await.is_err() {
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Send HTTP JSON response.
    async fn send_http_json<T: Serialize>(
        stream: &mut tokio::net::TcpStream,
        status: u16,
        body: &T,
    ) -> Result<()> {
        let json = serde_json::to_string(body)?;
        let status_text = match status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Unknown",
        };
        let response = format!(
            "HTTP/1.1 {} {}\r\n\
            Content-Type: application/json\r\n\
            Content-Length: {}\r\n\
            Access-Control-Allow-Origin: *\r\n\
            Access-Control-Allow-Methods: POST, GET, OPTIONS\r\n\
            Access-Control-Allow-Headers: Content-Type\r\n\
            \r\n\
            {}",
            status,
            status_text,
            json.len(),
            json
        );
        stream.write_all(response.as_bytes()).await?;
        Ok(())
    }

    /// Send HTTP error response.
    async fn send_http_error(
        stream: &mut tokio::net::TcpStream,
        status: u16,
        message: &str,
    ) -> Result<()> {
        let body = serde_json::json!({ "error": message });
        Self::send_http_json(stream, status, &body).await
    }

    /// Send CORS preflight response.
    async fn send_http_cors(stream: &mut tokio::net::TcpStream) -> Result<()> {
        let response = "HTTP/1.1 204 No Content\r\n\
            Access-Control-Allow-Origin: *\r\n\
            Access-Control-Allow-Methods: POST, GET, OPTIONS\r\n\
            Access-Control-Allow-Headers: Content-Type\r\n\
            Access-Control-Max-Age: 86400\r\n\
            \r\n";
        stream.write_all(response.as_bytes()).await?;
        Ok(())
    }

    /// Legacy method for backward compatibility.
    pub async fn run(&self) -> Result<()> {
        self.run_stdio().await
    }
}
