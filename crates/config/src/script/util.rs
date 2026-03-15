use std::sync::{Mutex, MutexGuard};

/// Lock a mutex, recovering the inner value on poisoning.
pub fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
