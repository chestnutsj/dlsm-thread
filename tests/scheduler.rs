//! 单线程协程调度器测试。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dlsm_greenthread::{spawn, yield_now, Scheduler};

#[test]
fn spawn_and_run_returns_value() {
    let sched = Scheduler::new();
    let h = sched.spawn(|| 6 * 7).unwrap();
    sched.run_until_idle();
    assert_eq!(h.join(), Some(42));
}

#[test]
fn join_before_run_is_none() {
    let sched = Scheduler::new();
    let h = sched.spawn(|| 1).unwrap();
    assert!(!h.is_finished());
    assert_eq!(h.join(), None::<i32>);
}

#[test]
fn multiple_coroutines_all_complete() {
    let sched = Scheduler::new();
    let counter = Rc::new(Cell::new(0usize));
    for _ in 0..5 {
        let c = Rc::clone(&counter);
        sched.spawn(move || c.set(c.get() + 1)).unwrap();
    }
    sched.run_until_idle();
    assert_eq!(counter.get(), 5);
}

#[test]
fn yield_round_robins_between_coroutines() {
    let log = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let sched = Scheduler::new();

    let la = Rc::clone(&log);
    sched
        .spawn(move || {
            la.borrow_mut().push("a1");
            yield_now();
            la.borrow_mut().push("a2");
        })
        .unwrap();

    let lb = Rc::clone(&log);
    sched
        .spawn(move || {
            lb.borrow_mut().push("b1");
            yield_now();
            lb.borrow_mut().push("b2");
        })
        .unwrap();

    sched.run_until_idle();
    assert_eq!(*log.borrow(), vec!["a1", "b1", "a2", "b2"]);
}

#[test]
fn spawn_from_within_coroutine() {
    let sched = Scheduler::new();
    let counter = Rc::new(Cell::new(0usize));

    let c = Rc::clone(&counter);
    sched
        .spawn(move || {
            c.set(c.get() + 1);
            // 在协程内派生子协程：经线程局部当前调度器入队
            let c2 = Rc::clone(&c);
            spawn(move || c2.set(c2.get() + 1)).unwrap();
        })
        .unwrap();

    sched.run_until_idle();
    assert_eq!(counter.get(), 2);
}
