//! 单线程协程调度器。
//!
//! 在一个 OS 线程上以 FIFO 轮转方式驱动多个协程：[`Scheduler::run_until_idle`] 反复取出就绪
//! 协程 `resume`，让出（`yield_now`）的重新入队，返回的丢弃。结果经 [`JoinHandle`] 取回。
//!
//! 运行期间当前调度器记录在线程局部，使协程内部可调用自由函数 [`spawn`] 派生新协程。
//! 后续可在此之上扩 M:N 线程池与跨线程偷工。

use core::cell::{Cell, RefCell};
use core::ptr::NonNull;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::coroutine::{Coroutine, Hint, ResumeOutcome};
use crate::stack::StackError;

/// 协程默认栈大小（128 KiB）。
const DEFAULT_STACK_SIZE: usize = 128 * 1024;

thread_local! {
    /// 当前正在 `run_until_idle` 的调度器；供自由函数 [`spawn`] 定位。
    static CURRENT_SCHED: Cell<Option<NonNull<Scheduler>>> = const { Cell::new(None) };
}

/// 协程返回值的取回句柄。
///
/// 协程跑完后其返回值存入共享槽；`run_until_idle` 结束后用 [`JoinHandle::join`] 取出。
#[derive(Debug)]
pub struct JoinHandle<T> {
    slot: Rc<RefCell<Option<T>>>,
}

impl<T> JoinHandle<T> {
    /// 协程是否已结束（结果已就绪）。
    #[inline]
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.slot.borrow().is_some()
    }

    /// 取回返回值；尚未结束返回 `None`。通常在 [`Scheduler::run_until_idle`] 之后调用。
    #[must_use]
    pub fn join(self) -> Option<T> {
        self.slot.borrow_mut().take()
    }
}

/// 单线程协程调度器。
#[derive(Default)]
pub struct Scheduler {
    ready: RefCell<VecDeque<Coroutine>>,
}

impl core::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Scheduler")
            .field("ready", &self.ready.borrow().len())
            .finish()
    }
}

impl Scheduler {
    /// 创建一个空调度器。
    #[must_use]
    pub fn new() -> Self {
        Self {
            ready: RefCell::new(VecDeque::new()),
        }
    }

    /// 派生一个协程（默认栈大小），返回取回其返回值的 [`JoinHandle`]。
    ///
    /// 协程被加入就绪队列，在后续 [`Self::run_until_idle`] 中执行。
    ///
    /// # Errors
    /// 栈分配失败时返回 [`StackError`]。
    pub fn spawn<F, T>(&self, f: F) -> Result<JoinHandle<T>, StackError>
    where
        F: FnOnce() -> T + 'static,
        T: 'static,
    {
        self.spawn_with(Hint::Normal, f)
    }

    /// 同 [`Self::spawn`]，但带调度归类 [`Hint`]（供未来 worker 放置）。
    ///
    /// # Errors
    /// 栈分配失败时返回 [`StackError`]。
    pub fn spawn_with<F, T>(&self, hint: Hint, f: F) -> Result<JoinHandle<T>, StackError>
    where
        F: FnOnce() -> T + 'static,
        T: 'static,
    {
        let slot: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
        let slot_for_coro = Rc::clone(&slot);
        let coro = Coroutine::new_with_hint(DEFAULT_STACK_SIZE, hint, move || {
            let result = f();
            *slot_for_coro.borrow_mut() = Some(result);
        })?;
        self.ready.borrow_mut().push_back(coro);
        Ok(JoinHandle { slot })
    }

    /// 驱动所有就绪协程直到队列清空（FIFO 轮转）。
    ///
    /// 协程 `yield_now` 让出后重新入队，下一轮继续；返回即出队丢弃。运行期间本调度器登记为
    /// 线程局部当前调度器，使协程内 [`spawn`] 生效。
    pub fn run_until_idle(&self) {
        let prev = CURRENT_SCHED.with(|c| c.replace(Some(NonNull::from(self))));
        loop {
            // 借用立即释放，resume 期间 ready 不被借用（协程内 spawn 可再借用）。
            let next = self.ready.borrow_mut().pop_front();
            let Some(mut coro) = next else { break };
            match coro.resume() {
                ResumeOutcome::Yielded => self.ready.borrow_mut().push_back(coro),
                ResumeOutcome::Done => drop(coro),
            }
        }
        CURRENT_SCHED.with(|c| c.set(prev));
    }
}

/// 在**当前正在运行的调度器**上派生协程（须在协程内、`run_until_idle` 期间调用）。
///
/// # Errors
/// 栈分配失败时返回 [`StackError`]。
///
/// # Panics
/// 当前线程没有正在运行的调度器时 panic。
pub fn spawn<F, T>(f: F) -> Result<JoinHandle<T>, StackError>
where
    F: FnOnce() -> T + 'static,
    T: 'static,
{
    spawn_with(Hint::Normal, f)
}

/// 同 [`spawn`]，但带调度归类 [`Hint`]。
///
/// # Errors
/// 栈分配失败时返回 [`StackError`]。
///
/// # Panics
/// 当前线程没有正在运行的调度器时 panic。
pub fn spawn_with<F, T>(hint: Hint, f: F) -> Result<JoinHandle<T>, StackError>
where
    F: FnOnce() -> T + 'static,
    T: 'static,
{
    let Some(sched) = CURRENT_SCHED.with(Cell::get) else {
        panic!("spawn must be called within a running scheduler")
    };
    // SAFETY: 指针由 run_until_idle 设置，指向其栈上 &self；调度器用 RefCell 做内部可变，
    // 此处取得的 &Scheduler 与 run_until_idle 的 &self 均为共享引用，互不冲突。
    unsafe { sched.as_ref() }.spawn_with(hint, f)
}
