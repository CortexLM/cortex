//! Cortex App Server - HTTP API server binary.

use std::process::ExitCode;

use clap::Parser;
use tokio::signal;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use cortex_app_server::{ServerConfig, run_with_shutdown};

/// Cortex API Server
#[derive(Parser)]
#[command(name = "cortex-server")]
#[command(about = "HTTP API server for Cortex Agent")]
#[command(version)]
struct Args {
    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,

    /// Listen address
    #[arg(short, long, default_value = "0.0.0.0:55554")]
    listen: String,

    /// Log level
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Enable JSON logging
    #[arg(long)]
    json_logs: bool,

    /// Enable mDNS/Bonjour service discovery.
    /// When enabled, the server will advertise itself on the local network.
    #[arg(long)]
    mdns: bool,

    /// Custom mDNS service name (defaults to "cortex-{port}").
    #[arg(long)]
    mdns_name: Option<String>,
}

fn setup_logging(level: &str, json: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    let subscriber = tracing_subscriber::registry().with(filter);

    if json {
        subscriber
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        subscriber
            .with(tracing_subscriber::fmt::layer().pretty())
            .init();
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    setup_logging(&args.log_level, args.json_logs);

    let config = if let Some(config_path) = args.config {
        match ServerConfig::load(&config_path) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to load config from {}: {}", config_path, e);
                return ExitCode::FAILURE;
            }
        }
    } else {
        let mut config = match ServerConfig::from_env() {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to load config from environment: {}", e);
                return ExitCode::FAILURE;
            }
        };
        config.listen_addr = args.listen;

        // Apply mDNS settings from CLI args
        if args.mdns {
            config.mdns.enabled = true;
        }
        if let Some(name) = args.mdns_name {
            config.mdns.service_name = Some(name);
        }

        config
    };

    info!("Starting Cortex server on {}", config.listen_addr);
    info!("Graceful shutdown timeout: {}s", config.shutdown_timeout);
    info!("Press Ctrl+C to stop");

    let shutdown_timeout = config.shutdown_timeout;

    // Create shutdown signal
    let shutdown = async move {
        let ctrl_c = async {
            signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {
                info!("Received Ctrl+C, initiating graceful shutdown (timeout: {}s)...", shutdown_timeout);
            }
            _ = terminate => {
                info!("Received SIGTERM, initiating graceful shutdown (timeout: {}s)...", shutdown_timeout);
            }
        }

        // Give in-flight requests time to complete
        // The axum graceful shutdown will handle waiting for connections
        info!(
            "Waiting up to {}s for in-flight requests to complete...",
            shutdown_timeout
        );
    };

    if let Err(e) = run_with_shutdown(config, shutdown).await {
        error!("Server error: {}", e);
        return ExitCode::FAILURE;
    }

    info!("Server stopped");
    ExitCode::SUCCESS
}
