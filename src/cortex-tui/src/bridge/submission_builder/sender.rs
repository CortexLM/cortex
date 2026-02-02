//! Sender extension trait for SubmissionBuilder.

use async_channel::Sender;
use cortex_protocol::Submission;
use tokio::sync::mpsc::Sender as TokioSender;

use super::SubmissionBuilder;

/// Extension trait for sending submissions through channels.
///
/// This trait provides a convenient way to send submissions without
/// having to manually build and unwrap them.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui::bridge::SubmissionSender;
///
/// async fn send_message(sender: &Sender<Submission>) -> anyhow::Result<()> {
///     sender.send_submission(SubmissionBuilder::user_message("Hello")).await
/// }
/// ```
#[async_trait::async_trait]
pub trait SubmissionSender {
    /// Send a submission built from the given builder.
    ///
    /// Returns an error if the builder has no operation set or if
    /// sending fails.
    async fn send_submission(&self, builder: SubmissionBuilder) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl SubmissionSender for Sender<Submission> {
    async fn send_submission(&self, builder: SubmissionBuilder) -> anyhow::Result<()> {
        if let Some(submission) = builder.build() {
            self.send(submission)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send submission: {}", e))?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SubmissionSender for TokioSender<Submission> {
    async fn send_submission(&self, builder: SubmissionBuilder) -> anyhow::Result<()> {
        if let Some(submission) = builder.build() {
            self.send(submission)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send submission: {}", e))?;
        }
        Ok(())
    }
}
