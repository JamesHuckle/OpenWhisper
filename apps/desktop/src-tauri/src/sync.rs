//! The one way the app locks a `std::sync::Mutex`.
//!
//! A lock is poisoned when a thread panics while holding it. The data inside
//! is still whole for everything the app keeps in a mutex, so the app carries
//! on with it rather than failing every later call.
//!
//! This file needs no Tauri, so its tests also run in `core-tests`.

use std::sync::{Mutex, MutexGuard};

/// Locks `mutex`, carrying on with its data if a panic poisoned it.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::lock;
    use std::sync::Mutex;

    #[test]
    fn a_poisoned_lock_still_gives_its_data() {
        let mutex = Mutex::new(vec![1]);
        let _ = std::panic::catch_unwind(|| {
            let mut guard = mutex.lock().unwrap();
            guard.push(2);
            panic!("poison the lock");
        });
        assert!(mutex.is_poisoned());

        lock(&mutex).push(3);

        assert_eq!(*lock(&mutex), [1, 2, 3]);
    }
}
