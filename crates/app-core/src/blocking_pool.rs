use db_common::{AppError, ErrorKind, Result};
use std::sync::Arc;
use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};

#[derive(Clone)]
pub struct BlockingPool {
    runtime_handle: Handle,
    semaphore: Arc<Semaphore>,
    max_concurrent: usize,
}

impl BlockingPool {
    pub fn new(max_concurrent: usize, _thread_name: &str) -> Self {
        Self {
            runtime_handle: Handle::current(),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            max_concurrent,
        }
    }

    pub async fn spawn<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce() -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let _permit = self.acquire_permit().await?;
        let runtime_handle = self.runtime_handle.clone();

        let handle: JoinHandle<Result<R>> = runtime_handle.spawn_blocking(f);

        handle.await.map_err(|e| {
            if e.is_cancelled() {
                AppError::new(ErrorKind::Cancelled, "blocking task was cancelled")
            } else {
                AppError::new(ErrorKind::Internal, "blocking task failed")
                    .with_detail(e.to_string())
            }
        })?
    }

    pub async fn spawn_with_timeout<F, R>(&self, f: F, timeout_ms: u64) -> Result<R>
    where
        F: FnOnce() -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let _permit = self.acquire_permit().await?;
        let runtime_handle = self.runtime_handle.clone();

        let handle: JoinHandle<Result<R>> = runtime_handle.spawn_blocking(f);

        let timeout = std::time::Duration::from_millis(timeout_ms);
        tokio::time::timeout(timeout, handle)
            .await
            .map_err(|_| AppError::new(ErrorKind::Timeout, "blocking task timed out"))?
            .map_err(|e| {
                if e.is_cancelled() {
                    AppError::new(ErrorKind::Cancelled, "blocking task was cancelled")
                } else {
                    AppError::new(ErrorKind::Internal, "blocking task failed")
                        .with_detail(e.to_string())
                }
            })?
    }

    async fn acquire_permit(&self) -> Result<OwnedSemaphorePermit> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::new(ErrorKind::Internal, "blocking pool semaphore closed"))
    }

    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }
}

impl Default for BlockingPool {
    fn default() -> Self {
        Self::new(4, "blocking-worker")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn executes_blocking_task() {
        let pool = BlockingPool::new(2, "test");

        let result = pool.spawn(|| Ok(42)).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn handles_blocking_error() {
        let pool = BlockingPool::new(2, "test");

        let result: Result<()> = pool.spawn(|| Err(AppError::validation("test error"))).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().message, "test error");
    }

    #[tokio::test]
    async fn respects_concurrent_limit() {
        let pool = BlockingPool::new(1, "test");

        let start = std::time::Instant::now();
        let pool_for_first = pool.clone();
        let pool_for_second = pool.clone();

        let handle1 = tokio::spawn(async move {
            pool_for_first.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                Ok(1)
            })
            .await
        });

        let handle2 = tokio::spawn(async move {
            pool_for_second.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                Ok(2)
            })
            .await
        });

        let (r1, r2) = tokio::join!(handle1, handle2);
        assert_eq!(r1.unwrap().unwrap(), 1);
        assert_eq!(r2.unwrap().unwrap(), 2);

        let elapsed = start.elapsed();
        assert!(elapsed >= std::time::Duration::from_millis(200));
    }

    #[tokio::test]
    async fn timeout_works() {
        let pool = BlockingPool::new(2, "test");

        let result = pool
            .spawn_with_timeout(
                || {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    Ok(42)
                },
                100,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().kind == ErrorKind::Timeout);
    }
}
