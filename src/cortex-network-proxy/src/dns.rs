//! DNS rebinding protection module.
//!
//! This module provides protection against DNS rebinding attacks by implementing
//! a double-check mechanism: verify DNS resolution before and after connection
//! to ensure the resolved IP doesn't change to a private/non-public address.
//!
//! # Security Model
//!
//! DNS rebinding attacks work by:
//! 1. Initial DNS query returns a public IP (passes security check)
//! 2. TTL expires quickly
//! 3. Second query returns a private IP (127.0.0.1, 10.x.x.x, etc.)
//! 4. Request is sent to the private network
//!
//! We mitigate this by:
//! - Checking the resolved IP before connection
//! - Verifying the actual peer IP after connection
//! - Blocking any connection that resolves to non-public IPs

use std::net::IpAddr;
use tokio::net::TcpStream;

use super::NetworkProxyError;
use super::ip_validation::is_non_public_ip;

/// Result of DNS resolution check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsCheckResult {
    /// All resolved IPs are public and safe.
    Safe(Vec<IpAddr>),

    /// At least one resolved IP is non-public.
    NonPublicIp(IpAddr),

    /// DNS resolution failed.
    ResolutionFailed(String),
}

/// Resolve a hostname and check if any IP is non-public.
///
/// # Arguments
/// * `host` - The hostname to resolve
/// * `port` - The port (used for address format)
///
/// # Returns
/// * `DnsCheckResult::Safe` - All IPs are public
/// * `DnsCheckResult::NonPublicIp` - Found a non-public IP
/// * `DnsCheckResult::ResolutionFailed` - DNS resolution failed
pub async fn check_dns_resolution(host: &str, port: u16) -> DnsCheckResult {
    let addr = format!("{}:{}", host, port);

    match tokio::net::lookup_host(&addr).await {
        Ok(addrs) => {
            let mut safe_ips = Vec::new();

            for socket_addr in addrs {
                let ip = socket_addr.ip();
                if is_non_public_ip(ip) {
                    return DnsCheckResult::NonPublicIp(ip);
                }
                safe_ips.push(ip);
            }

            if safe_ips.is_empty() {
                DnsCheckResult::ResolutionFailed("No addresses returned".to_string())
            } else {
                DnsCheckResult::Safe(safe_ips)
            }
        }
        Err(e) => DnsCheckResult::ResolutionFailed(e.to_string()),
    }
}

/// Check if a hostname resolves to any non-public IP.
///
/// # Arguments
/// * `host` - The hostname to check
/// * `port` - The port (used for address format)
///
/// # Returns
/// `true` if the host resolves to a non-public IP, `false` otherwise.
pub async fn host_resolves_to_non_public(host: &str, port: u16) -> bool {
    matches!(
        check_dns_resolution(host, port).await,
        DnsCheckResult::NonPublicIp(_)
    )
}

/// Safely connect to a host with DNS rebinding protection.
///
/// This function implements a secure connection pattern that protects against
/// DNS rebinding attacks:
///
/// 1. **Pre-connection check**: Resolve DNS and verify all IPs are public
/// 2. **Connect**: Establish TCP connection
/// 3. **Post-connection check**: Verify the actual peer IP is public
///
/// This double-check ensures that even if DNS changes between resolution
/// and connection (time-of-check to time-of-use attack), we catch it.
///
/// # Arguments
/// * `host` - The hostname to connect to
/// * `port` - The port to connect to
///
/// # Returns
/// * `Ok(TcpStream)` - Successfully connected to a public IP
/// * `Err(NetworkProxyError)` - Connection blocked or failed
///
/// # Security Note
///
/// The CVE-2025-59532 style attacks are mitigated by:
/// - Always checking the actual peer address after connection
/// - Not trusting cached DNS results
pub async fn safe_connect(host: &str, port: u16) -> Result<TcpStream, NetworkProxyError> {
    // Pre-connection DNS check
    let dns_result = check_dns_resolution(host, port).await;

    match dns_result {
        DnsCheckResult::NonPublicIp(_ip) => {
            return Err(NetworkProxyError::HostBlocked(
                host.to_string(),
                super::HostBlockReason::NotAllowedLocal,
            ));
        }
        DnsCheckResult::ResolutionFailed(e) => {
            return Err(NetworkProxyError::DnsError(format!(
                "Failed to resolve {}: {}",
                host, e
            )));
        }
        DnsCheckResult::Safe(_) => {
            // Continue to connection
        }
    }

    // Establish connection
    let addr = format!("{}:{}", host, port);
    let stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| NetworkProxyError::Internal(format!("Connection failed: {}", e)))?;

    // Post-connection IP verification (critical for DNS rebinding protection)
    let peer_addr = stream
        .peer_addr()
        .map_err(|e| NetworkProxyError::Internal(format!("Failed to get peer address: {}", e)))?;

    if is_non_public_ip(peer_addr.ip()) {
        // Shutdown the stream before returning error
        // Use drop to close - TcpStream doesn't have explicit shutdown in async
        drop(stream);
        return Err(NetworkProxyError::HostBlocked(
            host.to_string(),
            super::HostBlockReason::NotAllowedLocal,
        ));
    }

    Ok(stream)
}

/// Safely connect with timeout and DNS rebinding protection.
///
/// Same as `safe_connect` but with an explicit timeout.
///
/// # Arguments
/// * `host` - The hostname to connect to
/// * `port` - The port to connect to
/// * `timeout` - Connection timeout duration
///
/// # Returns
/// * `Ok(TcpStream)` - Successfully connected to a public IP
/// * `Err(NetworkProxyError)` - Connection blocked, failed, or timed out
pub async fn safe_connect_with_timeout(
    host: &str,
    port: u16,
    timeout: std::time::Duration,
) -> Result<TcpStream, NetworkProxyError> {
    tokio::time::timeout(timeout, safe_connect(host, port))
        .await
        .map_err(|_| {
            NetworkProxyError::Internal(format!("Connection to {}:{} timed out", host, port))
        })?
}

/// Verify that a connected stream's peer is at a public IP.
///
/// Use this to verify existing connections, e.g., after receiving a connection
/// or when reusing pooled connections.
///
/// # Arguments
/// * `stream` - The connected TCP stream to verify
///
/// # Returns
/// * `Ok(())` - Peer IP is public
/// * `Err(NetworkProxyError)` - Peer IP is non-public or could not be determined
pub fn verify_peer_ip(stream: &TcpStream) -> Result<(), NetworkProxyError> {
    let peer_addr = stream
        .peer_addr()
        .map_err(|e| NetworkProxyError::Internal(format!("Failed to get peer address: {}", e)))?;

    if is_non_public_ip(peer_addr.ip()) {
        return Err(NetworkProxyError::HostBlocked(
            peer_addr.ip().to_string(),
            super::HostBlockReason::NotAllowedLocal,
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_localhost_blocked() {
        // localhost should always resolve to non-public
        assert!(host_resolves_to_non_public("localhost", 80).await);
    }

    #[tokio::test]
    async fn test_check_dns_resolution_localhost() {
        let result = check_dns_resolution("localhost", 80).await;
        assert!(matches!(result, DnsCheckResult::NonPublicIp(_)));
    }

    #[tokio::test]
    async fn test_safe_connect_blocks_localhost() {
        let result = safe_connect("localhost", 80).await;
        assert!(result.is_err());

        if let Err(NetworkProxyError::HostBlocked(_, reason)) = result {
            assert_eq!(reason, super::super::HostBlockReason::NotAllowedLocal);
        }
    }

    #[tokio::test]
    async fn test_check_dns_resolution_invalid_host() {
        let result = check_dns_resolution("this-host-does-not-exist-12345.invalid", 80).await;
        assert!(matches!(result, DnsCheckResult::ResolutionFailed(_)));
    }

    // Note: Testing actual public DNS resolution would require network access
    // and might be flaky in CI environments
}
