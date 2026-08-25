//! Application service layer.
//!
//! This crate will own use-case orchestration such as connection management,
//! query sessions, task execution, and import/export workflows.

pub mod blocking_pool;
pub mod connection;
pub mod metadata;
pub mod query;
pub mod redis_service;
pub mod registry;
pub mod search_service;
pub mod session;
pub mod storage;
pub mod task;

pub use registry::{BrandConnectionTester, DriverRegistry, RestrictedConnectionTester};

pub const fn app_name() -> &'static str {
    "Orvyn"
}
