//! Application service layer.
//!
//! This crate will own use-case orchestration such as connection management,
//! query sessions, task execution, and import/export workflows.

pub mod blocking_pool;
pub mod connection;
pub mod metadata;
pub mod query;
pub mod session;
pub mod storage;
pub mod task;

pub const fn app_name() -> &'static str {
    "Rust DB Client"
}
