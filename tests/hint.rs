//! 协程调度归类 Hint 测试。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use dlsm_greenthread::{Coroutine, Hint, Scheduler, ThreadPool};

#[test]
fn coroutine_carries_hint() {
    let normal = Coroutine::new(16 * 1024, || {}).unwrap();
    assert_eq!(normal.hint(), Hint::Normal);

    let compute = Coroutine::new_with_hint(16 * 1024, Hint::Compute, || {}).unwrap();
    assert_eq!(compute.hint(), Hint::Compute);
}

#[test]
fn scheduler_spawn_with_compute_hint_runs() {
    let sched = Scheduler::new();
    let h = sched.spawn_with(Hint::Compute, || 9 * 9).unwrap();
    sched.run_until_idle();
    assert_eq!(h.join(), Some(81));
}

#[test]
fn pool_spawn_with_compute_hint_runs() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let pool = ThreadPool::new(2);
    let counter = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&counter);
    pool.spawn_with(Hint::Compute, move || {
        c.fetch_add(7, Ordering::SeqCst);
    })
    .unwrap();
    pool.wait_idle();
    assert_eq!(counter.load(Ordering::SeqCst), 7);
}
