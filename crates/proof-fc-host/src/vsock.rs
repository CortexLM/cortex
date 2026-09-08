//! Host side of Firecracker vsock.
//!
//! Host → guest: connect to `<jail root>/v.sock`, send `CONNECT <port>\n`,
//! read `OK <port>\n`, then speak framed JSON. Guest → host: Firecracker
//! connects to `<jail root>/v.sock_<port>`, so the host listens there.

use std::path::{Path, PathBuf};
use std::time::Duration;

use proof_vm_agent::HvError;
use proof_vm_proto::guest::{read_frame, write_frame};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::jail::VSOCK_IN_JAIL;

/// `<jail root>/v.sock`.
#[must_use]
pub fn uds_path(jail_root: &Path) -> PathBuf {
    jail_root.join(VSOCK_IN_JAIL)
}

/// `<jail root>/v.sock_<port>` — where Firecracker delivers guest-initiated
/// connections to host `port`.
#[must_use]
pub fn listener_path(jail_root: &Path, port: u32) -> PathBuf {
    jail_root.join(format!("{VSOCK_IN_JAIL}_{port}"))
}

/// One framed channel to a guest port.
#[derive(Debug)]
pub struct GuestChannel {
    stream: BufReader<UnixStream>,
}

impl GuestChannel {
    /// Connect to guest `port` through the VM's vsock UDS.
    ///
    /// # Errors
    ///
    /// [`HvError::Guest`] when the guest is not listening or the handshake
    /// is not `OK`.
    pub async fn connect(jail_root: &Path, port: u32) -> Result<Self, HvError> {
        let path = uds_path(jail_root);
        let mut stream = UnixStream::connect(&path)
            .await
            .map_err(|e| HvError::Guest(format!("vsock {}: {e}", path.display())))?;
        stream
            .write_all(format!("CONNECT {port}\n").as_bytes())
            .await
            .map_err(|e| HvError::Guest(format!("vsock connect {port}: {e}")))?;
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream
            .read_line(&mut line)
            .await
            .map_err(|e| HvError::Guest(format!("vsock handshake {port}: {e}")))?;
        if !line.starts_with("OK ") {
            return Err(HvError::Guest(format!(
                "vsock port {port}: guest not listening ({})",
                line.trim()
            )));
        }
        Ok(Self { stream })
    }

    /// Keep retrying `connect` until the guest answers or `budget` runs out.
    ///
    /// # Errors
    ///
    /// [`HvError::Guest`] with the last failure.
    pub async fn connect_within(
        jail_root: &Path,
        port: u32,
        budget: Duration,
    ) -> Result<Self, HvError> {
        let deadline = tokio::time::Instant::now() + budget;
        let mut last = HvError::Guest(format!("vsock port {port}: never came up"));
        while tokio::time::Instant::now() < deadline {
            match Self::connect(jail_root, port).await {
                Ok(c) => return Ok(c),
                Err(e) => last = e,
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(last)
    }

    /// Wrap an accepted guest-initiated stream.
    #[must_use]
    pub fn from_stream(stream: UnixStream) -> Self {
        Self {
            stream: BufReader::new(stream),
        }
    }

    /// Send one frame.
    ///
    /// # Errors
    ///
    /// [`HvError::Guest`].
    pub async fn send<T: Serialize>(&mut self, value: &T) -> Result<(), HvError> {
        write_frame(self.stream.get_mut(), value)
            .await
            .map_err(|e| HvError::Guest(format!("send: {e}")))
    }

    /// Receive one frame.
    ///
    /// # Errors
    ///
    /// [`HvError::Guest`].
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T, HvError> {
        read_frame(&mut self.stream)
            .await
            .map_err(|e| HvError::Guest(format!("recv: {e}")))
    }

    /// Receive one frame within `budget`.
    ///
    /// # Errors
    ///
    /// [`HvError::Deadline`] on timeout, else [`HvError::Guest`].
    pub async fn recv_within<T: DeserializeOwned>(
        &mut self,
        budget: Duration,
    ) -> Result<T, HvError> {
        tokio::time::timeout(budget, self.recv())
            .await
            .map_err(|_| HvError::Deadline(budget.as_secs()))?
    }
}

/// Listen for guest-initiated connections to host `port` (bind before boot).
///
/// # Errors
///
/// [`HvError::Backend`].
pub fn listen(jail_root: &Path, port: u32) -> Result<UnixListener, HvError> {
    let path = listener_path(jail_root, port);
    let _ = std::fs::remove_file(&path);
    UnixListener::bind(&path)
        .map_err(|e| HvError::Backend(format!("listen {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proof_vm_proto::guest::{HostToRlm, RlmToHost};
    use proof_vm_proto::API_VERSION;

    fn root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("proof-fc-vsock-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    /// A stand-in for Firecracker's UDS end: answers the CONNECT handshake
    /// and then speaks the guest protocol. Bound before it is spawned so the
    /// host side cannot race it on a current-thread runtime.
    async fn fake_firecracker(listener: UnixListener, ok: bool) {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await.expect("connect line");
        assert_eq!(line, "CONNECT 5000\n");
        if !ok {
            return;
        }
        stream
            .get_mut()
            .write_all(b"OK 1073741824\n")
            .await
            .expect("ok");
        let hello: HostToRlm = read_frame(&mut stream).await.expect("hello");
        assert!(matches!(hello, HostToRlm::Hello { .. }));
        write_frame(
            stream.get_mut(),
            &RlmToHost::Ready {
                agent: "fake-guest".into(),
                api_version: API_VERSION,
            },
        )
        .await
        .expect("ready");
    }

    #[tokio::test]
    async fn the_handshake_then_frames_and_a_silent_guest_refuses() {
        let r = root("ok");
        let bound = UnixListener::bind(uds_path(&r)).expect("bind");
        let server = tokio::spawn(fake_firecracker(bound, true));
        let mut ch = GuestChannel::connect_within(&r, 5000, Duration::from_secs(5))
            .await
            .expect("connect");
        ch.send(&HostToRlm::Hello {
            api_version: API_VERSION,
            topic_id: "topic-a".into(),
            vm_id: "topic-a-0001".into(),
        })
        .await
        .expect("send");
        let ready: RlmToHost = ch.recv_within(Duration::from_secs(5)).await.expect("ready");
        assert!(matches!(ready, RlmToHost::Ready { .. }));
        server.await.expect("server");

        let r2 = root("silent");
        let bound = UnixListener::bind(uds_path(&r2)).expect("bind");
        let server = tokio::spawn(fake_firecracker(bound, false));
        let err = GuestChannel::connect(&r2, 5000).await.expect_err("no OK");
        assert!(matches!(err, HvError::Guest(_)), "{err}");
        server.await.expect("server");

        let r3 = root("absent");
        let err = GuestChannel::connect_within(&r3, 5000, Duration::from_millis(700))
            .await
            .expect_err("no socket");
        assert!(matches!(err, HvError::Guest(_)), "{err}");
        assert_eq!(listener_path(&r3, 5001), r3.join("v.sock_5001"));
        let l = listen(&r3, 5001).expect("listen");
        drop(l);
        let _ = listen(&r3, 5001).expect("rebinding replaces a stale socket");
    }
}
