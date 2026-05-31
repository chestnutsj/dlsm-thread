//! 协程感知的 MCS 等待策略：等待锁时让出调度器而非空转。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dlsm_greenthread::{CoroutinePark, ThreadPool};
use dlsm_sync::{McsLock, McsNode};

/// 在多个 worker 上跑大量协程，各自经 `McsLock<CoroutinePark>` 互斥地递增同一计数器。
/// 等待锁时 `CoroutinePark` 调用 `yield_now` 让出，最终结果必须精确。
#[test]
fn coroutines_contend_on_mcs_lock_with_yield_park() {
    struct Cell(UnsafeCell<u64>);
    // SAFETY: 仅在持 MCS 锁时访问内部值。
    unsafe impl Sync for Cell {}

    const COROUTINES: usize = 16;
    const ITERS: u64 = 200;

    let pool = ThreadPool::new(4);
    let lock: Arc<McsLock<CoroutinePark>> = Arc::new(McsLock::with_park());
    let cell = Arc::new(Cell(UnsafeCell::new(0u64)));
    let done = Arc::new(AtomicU64::new(0));

    for _ in 0..COROUTINES {
        let lock = Arc::clone(&lock);
        let cell = Arc::clone(&cell);
        let done = Arc::clone(&done);
        pool.spawn(move || {
            let mut node = McsNode::new();
            for _ in 0..ITERS {
                let _g = lock.lock(&mut node);
                // SAFETY: 持锁期间唯一写者
                unsafe {
                    *cell.0.get() += 1;
                }
            }
            done.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    }

    pool.wait_idle();

    assert_eq!(done.load(Ordering::SeqCst), COROUTINES as u64);
    // SAFETY: 所有协程已完成
    let total = unsafe { *cell.0.get() };
    assert_eq!(total, COROUTINES as u64 * ITERS);
}
