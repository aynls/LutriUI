//! In-flight Tauri command cancellation.
//! Dropping the selected future aborts the reqwest body; late replies are ignored in the UI.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::provider::ProviderError;

#[derive(Default)]
pub struct InflightCancels {
    inner: Mutex<HashMap<String, CancellationToken>>,
}

impl InflightCancels {
    pub fn register(&self, id: &str) -> CancellationToken {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = guard.get(id) {
            return existing.clone();
        }
        let token = CancellationToken::new();
        if !id.is_empty() {
            guard.insert(id.to_string(), token.clone());
        }
        token
    }

    /// Cancel an id even if the command has not registered yet, so a fast
    /// click cannot start HTTP after the user already asked to stop.
    pub fn cancel(&self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(token) = guard.remove(id) {
            token.cancel();
            true
        } else {
            let token = CancellationToken::new();
            token.cancel();
            guard.insert(id.to_string(), token);
            false
        }
    }

    pub fn finish(&self, id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }
}

pub fn cancelled(code: &str, message: impl Into<String>) -> ProviderError {
    ProviderError::coded(code, message)
}

pub async fn run_cancellable<T, Fut>(
    token: CancellationToken,
    cancelled: ProviderError,
    work: Fut,
) -> Result<T, ProviderError>
where
    Fut: Future<Output = Result<T, ProviderError>>,
{
    tokio::select! {
        biased;
        _ = token.cancelled() => Err(cancelled),
        result = work => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn cancel_before_register_yields_cancelled_token() {
        let inflight = InflightCancels::default();
        assert!(!inflight.cancel("opt-1"));
        let token = inflight.register("opt-1");
        assert!(token.is_cancelled());
        inflight.finish("opt-1");
        let later = inflight.register("opt-1");
        assert!(!later.is_cancelled());
    }

    #[tokio::test]
    async fn biased_cancel_discards_a_ready_result() {
        let token = CancellationToken::new();
        token.cancel();
        let err = run_cancellable(
            token,
            cancelled("backend_generation_cancelled", "Cancelled"),
            async { Ok::<_, ProviderError>("late".to_string()) },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "backend_generation_cancelled");
    }

    #[tokio::test]
    async fn cancel_drops_in_flight_work() {
        let token = CancellationToken::new();
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let flag = started.clone();
        let done = finished.clone();
        let work = async move {
            flag.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(30)).await;
            done.store(true, Ordering::SeqCst);
            Ok::<_, ProviderError>("done".to_string())
        };
        let handle = tokio::spawn(run_cancellable(
            token.clone(),
            cancelled("backend_llm_cancelled", "Cancelled"),
            work,
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        token.cancel();
        let err = handle.await.unwrap().unwrap_err();
        assert_eq!(err.code, "backend_llm_cancelled");
        assert!(started.load(Ordering::SeqCst));
        assert!(!finished.load(Ordering::SeqCst));
    }
}
