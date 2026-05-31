//! 协程基础行为测试。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use dlsm_greenthread::{Coroutine, ResumeOutcome};

#[test]
fn run_to_completion_executes_closure_once() {
    let counter = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&counter);

    let mut co = Coroutine::new(64 * 1024, move || {
        c.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();

    assert_eq!(co.resume(), ResumeOutcome::Done);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn coroutine_yields_then_completes_on_second_resume() {
    let log = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
    let log_c = Arc::clone(&log);

    let mut co = Coroutine::new(64 * 1024, move || {
        log_c.lock().unwrap().push("before");
        dlsm_greenthread::yield_now();
        log_c.lock().unwrap().push("after");
    })
    .unwrap();

    assert_eq!(co.resume(), ResumeOutcome::Yielded);
    assert_eq!(log.lock().unwrap().as_slice(), &["before"]);

    assert_eq!(co.resume(), ResumeOutcome::Done);
    assert_eq!(log.lock().unwrap().as_slice(), &["before", "after"]);
}

#[test]
fn resume_after_done_remains_done() {
    let mut co = Coroutine::new(16 * 1024, || {}).unwrap();
    assert_eq!(co.resume(), ResumeOutcome::Done);
    assert_eq!(co.resume(), ResumeOutcome::Done);
}

#[test]
fn many_yields_round_trip_state() {
    const N: usize = 50;
    let counter = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&counter);

    let mut co = Coroutine::new(64 * 1024, move || {
        for _ in 0..N {
            c.fetch_add(1, Ordering::SeqCst);
            dlsm_greenthread::yield_now();
        }
    })
    .unwrap();

    for i in 1..=N {
        assert_eq!(co.resume(), ResumeOutcome::Yielded);
        assert_eq!(counter.load(Ordering::SeqCst), i);
    }
    assert_eq!(co.resume(), ResumeOutcome::Done);
}
