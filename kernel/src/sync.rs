use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
        SpinLockGuard { lock: self }
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

pub struct Mutex<T> {
    inner: SpinLock<T>,
}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: SpinLock::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        self.inner.lock()
    }
}

pub struct Semaphore {
    count: AtomicIsize,
}

impl Semaphore {
    pub const fn new(initial: isize) -> Self {
        Self {
            count: AtomicIsize::new(initial),
        }
    }

    pub fn try_down(&self) -> bool {
        loop {
            let current = self.count.load(Ordering::Acquire);
            if current <= 0 {
                return false;
            }
            if self
                .count
                .compare_exchange(current, current - 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub fn down(&self) {
        while !self.try_down() {
            crate::sched::yield_now();
            spin_loop();
        }
    }

    pub fn up(&self) {
        self.count.fetch_add(1, Ordering::Release);
    }

    pub fn value(&self) -> isize {
        self.count.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinlock_mutates_value() {
        let lock = SpinLock::new(1usize);
        {
            let mut guard = lock.lock();
            *guard += 4;
        }
        assert_eq!(*lock.lock(), 5);
    }

    #[test]
    fn mutex_mutates_value() {
        let lock = Mutex::new(7usize);
        {
            let mut guard = lock.lock();
            *guard = 11;
        }
        assert_eq!(*lock.lock(), 11);
    }

    #[test]
    fn semaphore_try_down_and_up() {
        let sem = Semaphore::new(1);
        assert!(sem.try_down());
        assert_eq!(sem.value(), 0);
        assert!(!sem.try_down());
        sem.up();
        assert_eq!(sem.value(), 1);
    }
}
