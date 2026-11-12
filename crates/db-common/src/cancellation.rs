//! Cooperative cancellation with scoped hooks for blocking database engines.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

type Hook = Box<dyn Fn() + Send + Sync>;
struct State {
    cancelled: bool,
    next: u64,
    hooks: BTreeMap<u64, Hook>,
}
struct Inner {
    state: Mutex<State>,
    signal: tokio::sync::watch::Sender<bool>,
}
#[derive(Clone)]
pub struct QueryCancellation(Arc<Inner>);
impl Default for QueryCancellation {
    fn default() -> Self {
        let (signal, _) = tokio::sync::watch::channel(false);
        Self(Arc::new(Inner {
            state: Mutex::new(State {
                cancelled: false,
                next: 0,
                hooks: BTreeMap::new(),
            }),
            signal,
        }))
    }
}
impl QueryCancellation {
    pub fn cancel(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.cancelled {
            return;
        }
        state.cancelled = true;
        self.0.signal.send_replace(true);
        for hook in state.hooks.values() {
            hook();
        }
    }
    pub fn is_cancelled(&self) -> bool {
        *self.0.signal.borrow()
    }
    pub async fn cancelled(&self) {
        let mut receiver = self.0.signal.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    }
    /// Hooks must be short and synchronous (e.g. sqlite3_interrupt).
    /// Dropping the registration removes the hook BEFORE returning a connection to its pool.
    pub fn register(&self, hook: impl Fn() + Send + Sync + 'static) -> Registration {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let key = state.next;
        state.next += 1;
        if state.cancelled {
            hook();
        } else {
            state.hooks.insert(key, Box::new(hook));
        }
        Registration {
            cancellation: self.clone(),
            key,
        }
    }
    pub fn error(&self) -> crate::AppError {
        crate::AppError::new(crate::ErrorKind::Cancelled, "Query cancelled")
    }
}
pub struct Registration {
    cancellation: QueryCancellation,
    key: u64,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.cancellation
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hooks
            .remove(&self.key);
    }
}
