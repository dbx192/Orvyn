use crate::{id::SessionId, AppError, ConnectionId, TaskId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Cancelled,
    Failed,
    TimedOut,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Failed | Self::TimedOut
        )
    }

    pub fn is_running(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: TaskId,
    pub connection_id: ConnectionId,
    pub session_id: Option<SessionId>,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub elapsed_ms: u128,
    pub error: Option<AppError>,
    pub progress: Option<TaskProgress>,
}

impl TaskInfo {
    pub fn new(id: TaskId, connection_id: ConnectionId, kind: TaskKind) -> Self {
        Self {
            id,
            connection_id,
            session_id: None,
            kind,
            status: TaskStatus::Pending,
            started_at: Utc::now(),
            completed_at: None,
            elapsed_ms: 0,
            error: None,
            progress: None,
        }
    }

    pub fn mark_running(&mut self) {
        self.status = TaskStatus::Running;
        self.started_at = Utc::now();
    }

    pub fn mark_completed(&mut self) {
        self.status = TaskStatus::Completed;
        self.completed_at = Some(Utc::now());
    }

    pub fn mark_cancelled(&mut self) {
        self.status = TaskStatus::Cancelled;
        self.completed_at = Some(Utc::now());
    }

    pub fn mark_failed(&mut self, error: AppError) {
        self.status = TaskStatus::Failed;
        self.completed_at = Some(Utc::now());
        self.error = Some(error);
    }

    pub fn mark_timed_out(&mut self) {
        self.status = TaskStatus::TimedOut;
        self.completed_at = Some(Utc::now());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    ConnectionTest,
    Connect,
    Disconnect,
    Query,
    MetadataLoad,
    RedisScan,
    Import,
    Export,
    TableEdit,
    TransactionCommit,
    TransactionRollback,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskProgress {
    pub current: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}

impl TaskProgress {
    pub fn percentage(&self) -> Option<f64> {
        self.total
            .map(|total| (self.current as f64 / total as f64) * 100.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Disconnected,
    Connecting,
    Connected,
    Broken,
    Reconnecting,
}

impl SessionState {
    pub fn is_connected(self) -> bool {
        matches!(self, Self::Connected)
    }

    pub fn is_connecting(self) -> bool {
        matches!(self, Self::Connecting | Self::Reconnecting)
    }

    pub fn needs_reconnect(self) -> bool {
        matches!(self, Self::Broken)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub connection_id: ConnectionId,
    pub state: SessionState,
    pub last_connected_at: Option<DateTime<Utc>>,
    pub last_disconnected_at: Option<DateTime<Utc>>,
    pub error: Option<AppError>,
    pub reconnect_attempts: u32,
    pub max_reconnect_attempts: u32,
    pub reconnect_delay: Duration,
    pub active_tasks: usize,
}

impl SessionInfo {
    pub fn new(connection_id: ConnectionId) -> Self {
        Self {
            id: SessionId::new(),
            connection_id,
            state: SessionState::Disconnected,
            last_connected_at: None,
            last_disconnected_at: None,
            error: None,
            reconnect_attempts: 0,
            max_reconnect_attempts: 5,
            reconnect_delay: Duration::from_secs(5),
            active_tasks: 0,
        }
    }

    pub fn mark_connecting(&mut self) {
        self.state = SessionState::Connecting;
    }

    pub fn mark_connected(&mut self) {
        self.state = SessionState::Connected;
        self.last_connected_at = Some(Utc::now());
        self.reconnect_attempts = 0;
        self.error = None;
    }

    pub fn mark_disconnected(&mut self) {
        self.state = SessionState::Disconnected;
        self.last_disconnected_at = Some(Utc::now());
        self.reconnect_attempts = 0;
    }

    pub fn mark_broken(&mut self, error: AppError) {
        self.state = SessionState::Broken;
        self.last_disconnected_at = Some(Utc::now());
        self.error = Some(error);
    }

    pub fn mark_reconnecting(&mut self) {
        self.state = SessionState::Reconnecting;
        self.reconnect_attempts += 1;
    }

    pub fn can_reconnect(&self) -> bool {
        self.reconnect_attempts < self.max_reconnect_attempts
    }
}
