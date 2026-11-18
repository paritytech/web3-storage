// SPDX-License-Identifier: Apache-2.0

/// Source error boxed from a chain client or storage implementation.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Why a garbage collector chain read or storage call failed.
///
/// Displays as the source error, so logs show the underlying message.
#[derive(Debug, thiserror::Error)]
pub enum GcError {
    /// A chain read failed or returned data of the wrong shape.
    #[error(transparent)]
    Chain(BoxError),

    /// A storage backend call failed.
    #[error(transparent)]
    Storage(BoxError),
}

impl GcError {
    /// A chain read failed.
    pub fn chain(e: impl Into<BoxError>) -> Self {
        GcError::Chain(e.into())
    }

    /// A storage call failed.
    pub fn storage(e: impl Into<BoxError>) -> Self {
        GcError::Storage(e.into())
    }
}
