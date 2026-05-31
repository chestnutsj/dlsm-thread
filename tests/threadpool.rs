//! M:N 协程线程池测试。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dlsm_greenthread::{yield_now, ThreadPool};

#[test]
fn pool_runs_all_spawned_tasks() {
    let pool = ThreadPool::new(4);
    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..100 {
        let c = Arc::clone(&counter);
        pool.spawn(move || {
            c.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    }
    pool.wait_idle();
    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

#[test]
fn pool_uses_multiple_worker_threads() {
    let n = 4;
    let pool = ThreadPool::new(n);
    let ids = Arc::new(Mutex::new(HashSet::new()));
    let ready = Arc::new(AtomicUsize::new(0));

    for _ in 0..n {
        let ids = Arc::clone(&ids);
        let ready = Arc::clone(&ready);
        pool.spawn(move || {
            ids.lock().unwrap().insert(thread::current().id());
            ready.fetch_add(1, Ordering::SeqCst);
            // 占住本 worker 直到所有 worker 都到达 → 强制 n 个任务并发在 n 个线程上。
            let deadline = Instant::now() + Duration::from_secs(10);
            while ready.load(Ordering::SeqCst) < n && Instant::now() < deadline {
                thread::yield_now();
            }
        })
        .unwrap();
    }
    pool.wait_idle();
    assert_eq!(
        ids.lock().unwrap().len(),
        n,
        "应有 n 个不同 worker 线程并发执行"
    );
}

#[test]
fn pooled_coroutine_yields_and_completes() {
    // yield 后协程回队列、可被另一 worker 接手（跨线程迁移），仍正确完成。
    let pool = ThreadPool::new(2);
    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..10 {
        let c = Arc::clone(&counter);
        pool.spawn(move || {
            c.fetch_add(1, Ordering::SeqCst);
            yield_now();
            c.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    }
    pool.wait_idle();
    assert_eq!(counter.load(Ordering::SeqCst), 20);
}

#[test]
fn spawn_from_within_pool_via_spawner() {
    let pool = ThreadPool::new(2);
    let counter = Arc::new(AtomicUsize::new(0));
    let spawner = pool.spawner();

    let c = Arc::clone(&counter);
    let sp = spawner.clone();
    pool.spawn(move || {
        c.fetch_add(1, Ordering::SeqCst);
        let c2 = Arc::clone(&c);
        sp.spawn(move || {
            c2.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    })
    .unwrap();

    pool.wait_idle();
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn drop_shuts_down_workers_without_hang() {
    let pool = ThreadPool::new(2);
    pool.spawn(|| {}).unwrap();
    pool.wait_idle();
    drop(pool); // 应收回 worker 线程、不挂起
}
