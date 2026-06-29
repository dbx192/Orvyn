use crate::blocking_pool::BlockingPool;
use chrono::{DateTime, Utc};
use db_common::{AppError, ConnectionId, ErrorKind, Result, SessionId, TaskId, TaskInfo, TaskKind};
use std::{collections::BTreeMap, sync::Arc};
use tokio::{sync::RwLock, task::AbortHandle, time::Duration};
use tracing::{info, instrument};

#[derive(Clone)]
pub struct TaskManager {
    tasks: Arc<RwLock<BTreeMap<TaskId, TaskInfo>>>,
    running_tasks: Arc<RwLock<BTreeMap<TaskId, AbortHandle>>>,
    blocking_pool: BlockingPool,
}

impl TaskManager {
    pub fn new(blocking_pool: BlockingPool) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(BTreeMap::new())),
            running_tasks: Arc::new(RwLock::new(BTreeMap::new())),
            blocking_pool,
        }
    }

    #[instrument(skip(self, future), fields(task_id = %task.id, task_kind = ?task.kind))]
    pub async fn spawn<T, F>(&self, task: TaskInfo, future: F) -> Result<TaskId>
    where
        F: std::future::Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let task_id = task.id;
        self.update_task(task_id, |t| {
            t.mark_running();
        })
        .await;

        let tasks_clone = self.tasks.clone();
        let running_tasks_clone = self.running_tasks.clone();

        let handle = tokio::spawn(async move {
            let result = future.await;

            tasks_clone
                .write()
                .await
                .entry(task_id)
                .and_modify(|t| match &result {
                    Ok(_) => t.mark_completed(),
                    Err(e) => t.mark_failed(e.clone()),
                });

            running_tasks_clone.write().await.remove(&task_id);

            result
        });

        self.running_tasks
            .write()
            .await
            .insert(task_id, handle.abort_handle());

        Ok(task_id)
    }

    #[instrument(skip(self, f), fields(task_id = %task.id, task_kind = ?task.kind))]
    pub async fn spawn_blocking<T, F>(&self, task: TaskInfo, f: F) -> Result<TaskId>
    where
        F: FnOnce() -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let task_id = task.id;

        self.update_task(task_id, |t| {
            t.mark_running();
        })
        .await;

        let tasks_clone = self.tasks.clone();
        let running_tasks_clone = self.running_tasks.clone();
        let blocking_pool = self.blocking_pool.clone();

        let handle = tokio::spawn(async move {
            let result = blocking_pool.spawn(f).await;

            tasks_clone
                .write()
                .await
                .entry(task_id)
                .and_modify(|t| match &result {
                    Ok(_) => t.mark_completed(),
                    Err(e) => t.mark_failed(e.clone()),
                });

            running_tasks_clone.write().await.remove(&task_id);

            result
        });

        self.running_tasks
            .write()
            .await
            .insert(task_id, handle.abort_handle());

        Ok(task_id)
    }

    #[instrument(skip(self, f), fields(task_id = %task.id, task_kind = ?task.kind))]
    pub async fn spawn_blocking_with_timeout<T, F>(
        &self,
        task: TaskInfo,
        f: F,
        timeout_ms: u64,
    ) -> Result<TaskId>
    where
        F: FnOnce() -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let task_id = task.id;

        self.update_task(task_id, |t| {
            t.mark_running();
        })
        .await;

        let tasks_clone = self.tasks.clone();
        let running_tasks_clone = self.running_tasks.clone();
        let blocking_pool = self.blocking_pool.clone();

        let handle = tokio::spawn(async move {
            let result = blocking_pool.spawn_with_timeout(f, timeout_ms).await;

            tasks_clone
                .write()
                .await
                .entry(task_id)
                .and_modify(|t| match &result {
                    Ok(_) => t.mark_completed(),
                    Err(e) => {
                        if e.kind == ErrorKind::Timeout {
                            t.mark_timed_out();
                        } else {
                            t.mark_failed(e.clone());
                        }
                    }
                });

            running_tasks_clone.write().await.remove(&task_id);

            result
        });

        self.running_tasks
            .write()
            .await
            .insert(task_id, handle.abort_handle());

        Ok(task_id)
    }

    #[instrument(skip(self), fields(task_id = %id))]
    pub async fn cancel(&self, id: TaskId) -> Result<()> {
        let handle = self.running_tasks.write().await.remove(&id);

        if let Some(handle) = handle {
            handle.abort();

            self.update_task(id, |t| {
                t.mark_cancelled();
            })
            .await;

            info!(task_id = %id, "task cancelled");
            Ok(())
        } else {
            Err(AppError::validation("task not found or already completed"))
        }
    }

    pub async fn get_task(&self, id: TaskId) -> Option<TaskInfo> {
        self.tasks.read().await.get(&id).cloned()
    }

    pub async fn list_tasks(&self) -> Vec<TaskInfo> {
        self.tasks.read().await.values().cloned().collect()
    }

    pub async fn list_tasks_for_connection(&self, connection_id: ConnectionId) -> Vec<TaskInfo> {
        self.tasks
            .read()
            .await
            .values()
            .filter(|t| t.connection_id == connection_id)
            .cloned()
            .collect()
    }

    pub async fn is_running(&self, id: TaskId) -> bool {
        self.running_tasks.read().await.contains_key(&id)
    }

    pub async fn active_task_count(&self) -> usize {
        self.running_tasks.read().await.len()
    }

    pub async fn active_task_count_for_connection(&self, connection_id: ConnectionId) -> usize {
        self.tasks
            .read()
            .await
            .values()
            .filter(|t| t.connection_id == connection_id && t.status.is_running())
            .count()
    }

    pub async fn create_task(&self, connection_id: ConnectionId, kind: TaskKind) -> TaskInfo {
        let task = TaskInfo::new(TaskId::new(), connection_id, kind);
        self.tasks.write().await.insert(task.id, task.clone());
        task
    }

    pub async fn create_task_with_session(
        &self,
        connection_id: ConnectionId,
        session_id: SessionId,
        kind: TaskKind,
    ) -> TaskInfo {
        let mut task = TaskInfo::new(TaskId::new(), connection_id, kind);
        task.session_id = Some(session_id);
        self.tasks.write().await.insert(task.id, task.clone());
        task
    }

    async fn update_task<F>(&self, id: TaskId, f: F)
    where
        F: FnOnce(&mut TaskInfo),
    {
        self.tasks.write().await.entry(id).and_modify(f);
    }

    pub async fn cleanup_completed(&self, older_than: Duration) {
        let cutoff: DateTime<Utc> =
            Utc::now() - chrono::Duration::from_std(older_than).unwrap_or_default();
        let mut tasks = self.tasks.write().await;

        tasks.retain(|_, t| {
            if let Some(completed_at) = t.completed_at {
                completed_at > cutoff
            } else {
                true
            }
        });
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new(BlockingPool::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_common::{ConnectionId, TaskStatus};

    #[tokio::test]
    async fn spawns_and_tracks_task() {
        let manager = TaskManager::default();
        let connection_id = ConnectionId::new();

        let task = manager.create_task(connection_id, TaskKind::Query).await;
        let task_id = task.id;

        assert!(manager.get_task(task_id).await.is_some());
        assert_eq!(
            manager.get_task(task_id).await.unwrap().status,
            TaskStatus::Pending
        );

        let _ = manager.spawn(task, async { Ok(42) }).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let task_info = manager.get_task(task_id).await.unwrap();
        assert_eq!(task_info.status, TaskStatus::Completed);
        assert!(!manager.is_running(task_id).await);
    }

    #[tokio::test]
    async fn cancels_task() {
        let manager = TaskManager::default();
        let connection_id = ConnectionId::new();

        let task = manager.create_task(connection_id, TaskKind::Query).await;
        let task_id = task.id;

        let _ = manager
            .spawn(task, async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(42)
            })
            .await;

        assert!(manager.is_running(task_id).await);

        manager.cancel(task_id).await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let task_info = manager.get_task(task_id).await.unwrap();
        assert_eq!(task_info.status, TaskStatus::Cancelled);
        assert!(!manager.is_running(task_id).await);
    }

    #[tokio::test]
    async fn spawns_blocking_task() {
        let manager = TaskManager::default();
        let connection_id = ConnectionId::new();

        let task = manager.create_task(connection_id, TaskKind::Query).await;
        let task_id = task.id;

        let _ = manager.spawn_blocking(task, || Ok(42)).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let task_info = manager.get_task(task_id).await.unwrap();
        assert_eq!(task_info.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn task_timeout() {
        let manager = TaskManager::default();
        let connection_id = ConnectionId::new();

        let task = manager.create_task(connection_id, TaskKind::Query).await;
        let task_id = task.id;

        let _ = manager
            .spawn_blocking_with_timeout(
                task,
                || {
                    std::thread::sleep(Duration::from_millis(500));
                    Ok(42)
                },
                100,
            )
            .await;

        tokio::time::sleep(Duration::from_millis(150)).await;

        let task_info = manager.get_task(task_id).await.unwrap();
        assert_eq!(task_info.status, TaskStatus::TimedOut);
    }

    #[tokio::test]
    async fn lists_tasks_for_connection() {
        let manager = TaskManager::default();
        let conn1 = ConnectionId::new();
        let conn2 = ConnectionId::new();

        let task1 = manager.create_task(conn1, TaskKind::Query).await;
        let task2 = manager.create_task(conn2, TaskKind::Query).await;
        let task3 = manager.create_task(conn1, TaskKind::MetadataLoad).await;

        let _ = manager.spawn(task1, async { Ok(1) }).await;
        let _ = manager.spawn(task2, async { Ok(2) }).await;
        let _ = manager.spawn(task3, async { Ok(3) }).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let conn1_tasks = manager.list_tasks_for_connection(conn1).await;
        assert_eq!(conn1_tasks.len(), 2);

        let conn2_tasks = manager.list_tasks_for_connection(conn2).await;
        assert_eq!(conn2_tasks.len(), 1);
    }
}
