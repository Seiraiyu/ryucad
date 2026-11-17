//! A lock that serves in arrival order (#849, design 698): each `lock`
//! takes a ticket and waits for its turn, so a script's write is never
//! overtaken by reads that arrived after it, as it can be under
//! `std::sync::Mutex` (an SRWLock on Windows, not first in first out).
//! `try_lock` takes the lock only when nobody holds it or waits for it.
//! A panic while holding it passes the turn on (the guard's drop runs
//! while unwinding), and the value is handed on as it is, as the daemon's
//! `lock` did with a poisoned mutex: handlers run behind a panic boundary.

use std::ops::{Deref, DerefMut};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

pub(crate) struct Fifo<T> {
    queue: Mutex<Queue>,
    turn: Condvar,
    data: Mutex<T>,
}

#[derive(Default)]
struct Queue {
    /// The next ticket to give.
    next: u64,
    /// The ticket whose turn it is.
    serving: u64,
}

pub(crate) struct FifoGuard<'a, T> {
    fifo: &'a Fifo<T>,
    inner: Option<MutexGuard<'a, T>>,
}

impl<T> Fifo<T> {
    pub(crate) fn new(value: T) -> Fifo<T> {
        Fifo {
            queue: Mutex::new(Queue::default()),
            turn: Condvar::new(),
            data: Mutex::new(value),
        }
    }

    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for every earlier `lock` to be done, then holds the value.
    pub(crate) fn lock(&self) -> FifoGuard<'_, T> {
        let mut q = self.queue();
        let ticket = q.next;
        q.next += 1;
        while q.serving != ticket {
            q = self.turn.wait(q).unwrap_or_else(PoisonError::into_inner);
        }
        drop(q);
        self.enter()
    }

    /// The value, if nobody holds it or waits for it.
    pub(crate) fn try_lock(&self) -> Option<FifoGuard<'_, T>> {
        let mut q = self.queue();
        if q.serving != q.next {
            return None;
        }
        q.next += 1;
        drop(q);
        Some(self.enter())
    }

    /// Holders and waiters now (tests).
    #[cfg(test)]
    pub(crate) fn queued(&self) -> u64 {
        let q = self.queue();
        q.next - q.serving
    }

    /// Our turn: nobody else holds `data`, so this never waits.
    fn enter(&self) -> FifoGuard<'_, T> {
        let inner = self.data.lock().unwrap_or_else(PoisonError::into_inner);
        FifoGuard {
            fifo: self,
            inner: Some(inner),
        }
    }
}

impl<T> Drop for FifoGuard<'_, T> {
    fn drop(&mut self) {
        drop(self.inner.take());
        self.fifo.queue().serving += 1;
        self.fifo.turn.notify_all();
    }
}

impl<T> Deref for FifoGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.inner.as_ref().expect("held until dropped")
    }
}

impl<T> DerefMut for FifoGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.inner.as_mut().expect("held until dropped")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn turns_go_in_arrival_order() {
        let fifo = Arc::new(Fifo::new(Vec::new()));
        let first = fifo.lock();
        let mut threads = Vec::new();
        for n in 0..8u64 {
            let f = Arc::clone(&fifo);
            threads.push(std::thread::spawn(move || f.lock().push(n)));
            // Each has its ticket before the next starts.
            while fifo.queued() < n + 2 {
                std::thread::yield_now();
            }
        }
        drop(first);
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(*fifo.lock(), (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn try_lock_takes_only_a_free_lock() {
        let fifo = Arc::new(Fifo::new(0));
        assert!(fifo.try_lock().is_some());
        let held = fifo.lock();
        assert!(fifo.try_lock().is_none());
        // Someone waiting counts too: status never jumps the queue.
        let f = Arc::clone(&fifo);
        let waiter = std::thread::spawn(move || *f.lock() += 1);
        while fifo.queued() < 2 {
            std::thread::yield_now();
        }
        drop(held);
        waiter.join().unwrap();
        assert_eq!(*fifo.try_lock().unwrap(), 1);
    }

    #[test]
    fn a_panic_while_holding_passes_the_turn_on() {
        let fifo = Arc::new(Fifo::new(1));
        let f = Arc::clone(&fifo);
        let r = std::thread::spawn(move || {
            let mut g = f.lock();
            *g = 2;
            panic!("in a handler");
        })
        .join();
        assert!(r.is_err());
        assert_eq!(*fifo.lock(), 2);
        assert_eq!(fifo.queued(), 0);
    }
}
