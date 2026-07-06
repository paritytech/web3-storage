// SPDX-License-Identifier: Apache-2.0

//! Background checkpoint loop control types.

use crate::checkpoint::result::CheckpointResult;
use crate::error::ClientError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use storage_primitives::BucketId;
use tokio::sync::mpsc;

/// Message for controlling the background checkpoint loop.
#[derive(Debug)]
pub enum CheckpointLoopCommand {
    /// Submit a checkpoint immediately.
    SubmitNow,
    /// Mark that changes have occurred for a bucket.
    MarkDirty(BucketId),
    /// Pause the checkpoint loop.
    Pause,
    /// Resume the checkpoint loop.
    Resume,
    /// Stop the checkpoint loop.
    Stop,
}

/// Status of a bucket in the checkpoint loop.
#[derive(Clone, Debug, Default)]
pub struct BucketCheckpointStatus {
    /// Whether the bucket has pending changes.
    pub dirty: bool,
    /// Last successful checkpoint time.
    pub last_checkpoint: Option<Instant>,
    /// Last checkpoint result.
    pub last_result: Option<CheckpointResult>,
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
}

/// Handle for controlling a running checkpoint loop.
pub struct CheckpointLoopHandle {
    /// Channel for sending commands to the loop.
    command_tx: mpsc::Sender<CheckpointLoopCommand>,
    /// Flag indicating if the loop is running.
    running: Arc<AtomicBool>,
    /// Handle to the background task.
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl CheckpointLoopHandle {
    /// Create a new handle.
    pub(crate) fn new(
        command_tx: mpsc::Sender<CheckpointLoopCommand>,
        running: Arc<AtomicBool>,
        task_handle: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            command_tx,
            running,
            task_handle: Some(task_handle),
        }
    }

    /// Check if the loop is still running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Request immediate checkpoint submission.
    pub async fn submit_now(&self) -> Result<(), ClientError> {
        self.command_tx
            .send(CheckpointLoopCommand::SubmitNow)
            .await
            .map_err(|_| ClientError::Chain("Checkpoint loop not running".to_string()))
    }

    /// Mark a bucket as dirty (has pending changes).
    pub async fn mark_dirty(&self, bucket_id: BucketId) -> Result<(), ClientError> {
        self.command_tx
            .send(CheckpointLoopCommand::MarkDirty(bucket_id))
            .await
            .map_err(|_| ClientError::Chain("Checkpoint loop not running".to_string()))
    }

    /// Pause the checkpoint loop.
    pub async fn pause(&self) -> Result<(), ClientError> {
        self.command_tx
            .send(CheckpointLoopCommand::Pause)
            .await
            .map_err(|_| ClientError::Chain("Checkpoint loop not running".to_string()))
    }

    /// Resume the checkpoint loop.
    pub async fn resume(&self) -> Result<(), ClientError> {
        self.command_tx
            .send(CheckpointLoopCommand::Resume)
            .await
            .map_err(|_| ClientError::Chain("Checkpoint loop not running".to_string()))
    }

    /// Stop the checkpoint loop.
    pub async fn stop(&mut self) -> Result<(), ClientError> {
        let _ = self.command_tx.send(CheckpointLoopCommand::Stop).await;
        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
        Ok(())
    }
}

/// Callback type for checkpoint completion events.
pub type CheckpointCallback = Arc<dyn Fn(BucketId, &CheckpointResult) + Send + Sync>;
