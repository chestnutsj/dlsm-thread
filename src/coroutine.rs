//! 协程封装与调度入口。

use core::cell::Cell;
use core::ptr::NonNull;
use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::context::{coroutine_entry, swap_context, Context};
use crate::stack::{Stack, StackError};

/// 单次 [`Coroutine::resume`] 的结果。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ResumeOutcome {
    /// 协程主动调用 [`yield_now`] 让出, 可再次 resume。
    Yielded,
    /// 协程已运行完毕 (闭包返回或 panic)。
    Done,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum State {
    Ready,
    Running,
    Yielded,
    Done,
}

/// 协程 inner: 必须堆分配并固定地址 (栈上指针指向 inner 内字段)。
struct CoroutineInner {
    scheduler_ctx: Context,
    coro_ctx: Context,
    #[allow(dead_code)] // 通过 RSP 持有, 但需要其生命周期与 inner 绑定
    stack: Stack,
    state: State,
    panic_payload: Option<Box<dyn Any + Send + 'static>>,
}

/// 一个独立的用户态协程, 拥有自己的栈与寄存器上下文。
///
/// 不可 `Clone`; 通过 `&mut self` 的 `resume` 推进执行。
pub struct Coroutine {
    inner: Box<CoroutineInner>,
}

// 注: 当前 Coroutine 不实现 Send——单线程调度器（[`crate::Scheduler`]）在一个 OS 线程上
// resume 它，闭包可捕获 !Send 数据（如 Rc）。未来支持 M:N 跨线程迁移时再引入 Send 变体。

thread_local! {
    /// 当前正在执行的协程 inner; yield_now 与 coroutine_main 通过它定位 scheduler_ctx。
    static CURRENT: Cell<Option<NonNull<CoroutineInner>>> = const { Cell::new(None) };
}

impl Coroutine {
    /// 创建一个新协程, 闭包将在首次 `resume` 时开始执行。
    ///
    /// # Errors
    /// 当栈分配失败时返回 [`StackError`]。
    pub fn new<F>(stack_size: usize, f: F) -> Result<Self, StackError>
    where
        F: FnOnce() + 'static,
    {
        let stack = Stack::new(stack_size)?;

        // 双重 Box: 内层是 dyn FnOnce 的 fat ptr, 外层是 thin ptr (FromRaw 需要它)
        let closure: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
        let closure_ptr = Box::into_raw(closure) as usize;

        let mut inner = Box::new(CoroutineInner {
            scheduler_ctx: Context::default(),
            coro_ctx: Context::default(),
            stack,
            state: State::Ready,
            panic_payload: None,
        });

        // 初始化协程的寄存器上下文:
        // - RIP = coroutine_entry (asm)
        // - RSP = stack_top - 8 (使 entry 处的 RSP 满足"被 call 后"对齐 16k+8)
        // - R12 = closure_ptr (entry 把它移到 rdi 然后 jmp coroutine_main)
        let sp = (inner.stack.top().as_ptr() as usize) - 8;
        inner.coro_ctx.rip = coroutine_entry as *const () as usize;
        inner.coro_ctx.rsp = sp;
        inner.coro_ctx.r12 = closure_ptr;

        Ok(Self { inner })
    }

    /// 推进协程一次。
    ///
    /// - 若协程主动 `yield_now`, 返回 [`ResumeOutcome::Yielded`]
    /// - 若协程的闭包已返回 (或 panic), 返回 [`ResumeOutcome::Done`]
    /// - 已 Done 的协程再次调用仍返回 `Done`, 不再切换上下文
    pub fn resume(&mut self) -> ResumeOutcome {
        if self.inner.state == State::Done {
            return ResumeOutcome::Done;
        }

        let inner_ptr = NonNull::from(&mut *self.inner);
        let prev = CURRENT.with(|c| {
            let p = c.get();
            c.set(Some(inner_ptr));
            p
        });

        self.inner.state = State::Running;

        // SAFETY: scheduler_ctx 与 coro_ctx 都在 inner (Box 分配, 地址稳定) 内,
        // swap_context 的不变量由 init/yield/coroutine_main 路径协同保证。
        unsafe {
            swap_context(&mut self.inner.scheduler_ctx, &self.inner.coro_ctx);
        }

        CURRENT.with(|c| c.set(prev));

        match self.inner.state {
            State::Done => {
                if let Some(payload) = self.inner.panic_payload.take() {
                    std::panic::resume_unwind(payload);
                }
                ResumeOutcome::Done
            }
            State::Yielded => ResumeOutcome::Yielded,
            State::Ready | State::Running => {
                unreachable!("after swap_context returned, state must be Yielded or Done")
            }
        }
    }
}

impl core::fmt::Debug for Coroutine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Coroutine")
            .field("state", &self.inner.state)
            .finish()
    }
}

/// 让出当前协程, 切回调度器; 必须在协程内调用。
///
/// # Panics
/// 若当前不在协程上下文中调用 (没有 `CURRENT`), 会 panic。
pub fn yield_now() {
    let Some(inner) = CURRENT.with(Cell::get) else {
        panic!("yield_now must be called inside a coroutine")
    };

    // SAFETY: inner 由 resume 设置, 指向 Box 中的稳定地址; 在协程内调用期间一定存活。
    unsafe {
        let coro_ctx_ptr = core::ptr::addr_of_mut!((*inner.as_ptr()).coro_ctx);
        let sched_ctx_ptr = core::ptr::addr_of!((*inner.as_ptr()).scheduler_ctx);
        (*inner.as_ptr()).state = State::Yielded;
        swap_context(coro_ctx_ptr, sched_ctx_ptr);
        // 重新恢复后控制流回到这里
        (*inner.as_ptr()).state = State::Running;
    }
}

/// 协程主体的 Rust 入口; 由汇编 `coroutine_entry` 通过 `jmp` 调入,
/// 永远不返回 (要么 swap 回 scheduler, 要么进入 UB-trap)。
///
/// # Safety
/// 仅可由 `coroutine_entry` 间接调用; `closure_ptr` 必须是 `Box::into_raw` 产出的
/// `Box<Box<dyn FnOnce() + Send>>` 原始指针。
#[no_mangle]
unsafe extern "C" fn coroutine_main(closure_ptr: *mut ()) -> ! {
    // SAFETY: 调用方 (coroutine_entry) 保证 closure_ptr 来自 Box::into_raw
    let boxed: Box<Box<dyn FnOnce()>> =
        unsafe { Box::from_raw(closure_ptr.cast::<Box<dyn FnOnce()>>()) };

    let result = catch_unwind(AssertUnwindSafe(move || (*boxed)()));

    let Some(inner) = CURRENT.with(Cell::get) else {
        // SAFETY: coroutine_main 仅由 coroutine_entry 调入, 后者只能由 resume 启动,
        // 而 resume 一定设置过 CURRENT。这里到达即调用方违反了安全约定。
        unsafe { core::hint::unreachable_unchecked() }
    };

    // SAFETY: inner 来自 resume 设置的稳定地址
    unsafe {
        let coro_ctx_ptr = core::ptr::addr_of_mut!((*inner.as_ptr()).coro_ctx);
        let sched_ctx_ptr = core::ptr::addr_of!((*inner.as_ptr()).scheduler_ctx);
        (*inner.as_ptr()).state = State::Done;
        if let Err(payload) = result {
            (*inner.as_ptr()).panic_payload = Some(payload);
        }
        // 永久切回 scheduler
        swap_context(coro_ctx_ptr, sched_ctx_ptr);
    }

    // 已切回 scheduler, 我们不应再到达这里; ud2 等价。
    // SAFETY: state == Done, scheduler 不会再 resume 我们
    unsafe { core::hint::unreachable_unchecked() }
}
