//! Locks that outlive a thread that panicked with them.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks `m` even when a thread panicked while holding it: nothing here leaves its data
/// half-changed, so one failed request must not fail every later one.
pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
