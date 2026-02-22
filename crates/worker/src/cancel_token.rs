use tokio::sync::watch;

/// Signal state for cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelSignal {
    Running,
    Cancelled,
}

/// Owned token that can cancel an operation.
pub struct CancelToken {
    tx: watch::Sender<CancelSignal>,
}

/// Guard that can be awaited to detect cancellation.
#[derive(Clone)]
pub struct CancelGuard {
    rx: watch::Receiver<CancelSignal>,
}

impl CancelToken {
    /// Create a new cancel token pair.
    pub fn new() -> (Self, CancelGuard) {
        let (tx, rx) = watch::channel(CancelSignal::Running);
        (Self { tx }, CancelGuard { rx })
    }

    /// Cancel the operation. Idempotent.
    pub fn cancel(&self) {
        let _ = self.tx.send(CancelSignal::Cancelled);
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new().0
    }
}

impl CancelGuard {
    /// Wait until the operation is cancelled.
    pub async fn cancelled(&mut self) {
        let mut rx = self.rx.clone();
        while *rx.borrow() == CancelSignal::Running {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }

    /// Check if already cancelled without waiting.
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow() == CancelSignal::Cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cancel_token_basic() {
        let (token, mut guard) = CancelToken::new();
        assert!(!guard.is_cancelled());

        token.cancel();
        assert!(guard.is_cancelled());
    }

    #[tokio::test]
    async fn test_cancel_idempotent() {
        let (token, mut guard) = CancelToken::new();
        token.cancel();
        token.cancel();
        assert!(guard.is_cancelled());
    }

    #[tokio::test]
    async fn test_cancel_guard_waits() {
        let (token, mut guard) = CancelToken::new();

        let handle = tokio::spawn(async move {
            guard.cancelled().await;
            "cancelled"
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        token.cancel();

        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(1),
            handle,
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().unwrap(), "cancelled");
    }

    #[tokio::test]
    async fn test_multiple_guards() {
        let (token, guard1) = CancelToken::new();
        let mut guard2 = guard1.clone();
        let mut guard3 = guard1.clone();

        token.cancel();
        assert!(guard2.is_cancelled());
        assert!(guard3.is_cancelled());
    }
}
