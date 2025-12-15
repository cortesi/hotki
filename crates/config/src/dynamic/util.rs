use std::sync::{Mutex, MutexGuard};

/// Lock a mutex, recovering the inner value on poisoning.
pub(super) fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
