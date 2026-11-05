//! One lock for every test that touches a window, the real clipboard or the theme's
//! process-wide state, so those tests take turns however many threads the harness runs.

use std::sync::{Mutex, MutexGuard, PoisonError};

static UI: Mutex<()> = Mutex::new(());

/// Takes the turn; hold the guard for the whole test. A test that panicked while holding it
/// doesn't stop the next one.
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    UI.lock().unwrap_or_else(PoisonError::into_inner)
}
