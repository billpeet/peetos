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
            // A TLB-shootdown initiator may be waiting on this CPU.
            crate::smp::service_tlb_shootdown();
            spin_loop();
        }
        SpinLockGuard { lock: self }
    }

    /// Take the lock only if it is free. For paths that must not wait, such
    /// as fatal-fault reporting, which may run while the faulting code holds
    /// the lock on this CPU.
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard { lock: self })
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

/// Spinlock that disables interrupts on the local CPU while held.
///
/// Use it for state touched from interrupt handlers or from paths that run
/// without the big kernel lock (scheduler, PMM, heap). Holding it with
/// interrupts off means an interrupt on this CPU can never spin on a lock
/// its own CPU already holds. While waiting it services TLB-shootdown
/// requests, because a shootdown initiator may be spinning on this CPU.
///
/// Never hold one across a context switch through [`IrqSpinLockGuard`]. The
/// scheduler uses the raw [`IrqSpinLock::lock_raw`] / [`IrqSpinLock::unlock_raw`]
/// pair for its hand-off across the switch.
pub struct IrqSpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for IrqSpinLock<T> {}
unsafe impl<T: Send> Send for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    fn spin_acquire(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                crate::smp::service_tlb_shootdown();
                spin_loop();
            }
        }
    }

    pub fn lock(&self) -> IrqSpinLockGuard<'_, T> {
        let was_enabled = arch_x86_64::interrupts::are_enabled();
        arch_x86_64::interrupts::disable();
        self.spin_acquire();
        IrqSpinLockGuard {
            lock: self,
            restore_interrupts: was_enabled,
        }
    }

    /// Acquire without a guard. Interrupts must already be disabled and stay
    /// disabled until [`unlock_raw`](Self::unlock_raw).
    pub fn lock_raw(&self) {
        self.spin_acquire();
    }

    /// Release a lock taken with [`lock_raw`](Self::lock_raw).
    pub fn unlock_raw(&self) {
        self.locked.store(false, Ordering::Release);
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    /// Access the protected value. The caller must hold the lock (raw form).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        &mut *self.value.get()
    }
}

pub struct IrqSpinLockGuard<'a, T> {
    lock: &'a IrqSpinLock<T>,
    restore_interrupts: bool,
}

impl<T> Deref for IrqSpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for IrqSpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for IrqSpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        if self.restore_interrupts {
            arch_x86_64::interrupts::enable();
        }
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
    fn irq_spinlock_masks_and_restores_interrupts() {
        arch_x86_64::interrupts::enable();
        let lock = IrqSpinLock::new(3u32);
        {
            let mut guard = lock.lock();
            assert!(!arch_x86_64::interrupts::are_enabled());
            assert!(lock.is_locked());
            *guard += 1;
        }
        assert!(arch_x86_64::interrupts::are_enabled());
        assert!(!lock.is_locked());
        assert_eq!(*lock.lock(), 4);

        arch_x86_64::interrupts::disable();
        drop(lock.lock());
        assert!(!arch_x86_64::interrupts::are_enabled());
        arch_x86_64::interrupts::enable();
    }

    #[test]
    fn irq_spinlock_raw_pair() {
        let lock = IrqSpinLock::new(0u8);
        lock.lock_raw();
        assert!(lock.is_locked());
        unsafe { *lock.get_mut_unchecked() = 9 };
        lock.unlock_raw();
        assert!(!lock.is_locked());
        assert_eq!(*lock.lock(), 9);
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

    #[test]
    fn spinlock_try_lock_fails_while_held() {
        let lock = SpinLock::new(5u32);
        {
            let mut held = lock.lock();
            *held = 6;
            assert!(lock.try_lock().is_none());
        }
        let again = lock.try_lock().expect("free after the guard drops");
        assert_eq!(*again, 6);
    }
}
