//! M:N 协程线程池。
//!
//! 一组 OS worker 线程共享一个就绪队列（`Mutex<VecDeque> + Condvar`），各自取协程 `resume`。
//! 协程 `yield_now` 让出后重新入队，**可被任意 worker 接手**（跨 OS 线程迁移）——这要求被
//! 调度的协程是 `Send` 的，由 [`ThreadPool::spawn`] / [`Spawner::spawn`] 的 `Send` 约束保证。
//! 据此用 [`SendCoroutine`] 包装承载跨线程移动。
//!
//! 这是引擎"并行度 ≈ 核数"的执行基质：大量协程复用在固定数量 worker 上。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use crate::coroutine::{Coroutine, ResumeOutcome};
use crate::stack::StackError;

/// 协程默认栈大小（128 KiB）。
const DEFAULT_STACK_SIZE: usize = 128 * 1024;

/// 可跨 worker 线程移动的协程载体。
///
/// # Safety
/// 仅由 [`ThreadPool::spawn`] / [`Spawner::spawn`] 构造，二者要求闭包 `Send`；协程不携带任何
/// 线程亲和状态（栈/上下文为独立内存，`resume` 在任意线程上等价），故跨线程移动并恢复是健全的。
struct SendCoroutine(Coroutine);

// SAFETY: 见类型文档——构造点强制闭包 Send，协程无线程亲和状态。
unsafe impl Send for SendCoroutine {}

/// 队列与计数（受 `PoolInner::state` 的 Mutex 保护）。
struct PoolState {
    queue: VecDeque<SendCoroutine>,
    /// 已派生但未完成的协程数；归零即空闲。
    outstanding: usize,
}

/// 线程池共享内核。
struct PoolInner {
    state: Mutex<PoolState>,
    /// 有新任务 / 关停时唤醒 worker。
    work: Condvar,
    /// `outstanding` 归零时唤醒 [`ThreadPool::wait_idle`]。
    idle: Condvar,
    shutdown: AtomicBool,
}

impl PoolInner {
    #[inline]
    fn lock(&self) -> MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn enqueue<F>(&self, f: F) -> Result<(), StackError>
    where
        F: FnOnce() + Send + 'static,
    {
        let coro = Coroutine::new(DEFAULT_STACK_SIZE, f)?;
        {
            let mut st = self.lock();
            st.outstanding += 1;
            st.queue.push_back(SendCoroutine(coro));
        }
        self.work.notify_one();
        Ok(())
    }
}

/// 取出一个就绪协程；返回 `None` 表示已关停且队列排空，worker 应退出。
fn pop_or_park(inner: &PoolInner) -> Option<SendCoroutine> {
    let mut st = inner.lock();
    loop {
        if let Some(coro) = st.queue.pop_front() {
            return Some(coro);
        }
        if inner.shutdown.load(Ordering::Acquire) {
            return None;
        }
        st = inner.work.wait(st).unwrap_or_else(PoisonError::into_inner);
    }
}

/// 单个 worker 的主循环。
fn worker_loop(inner: &PoolInner) {
    while let Some(mut coro) = pop_or_park(inner) {
        match coro.0.resume() {
            ResumeOutcome::Yielded => {
                inner.lock().queue.push_back(coro);
                inner.work.notify_one();
            }
            ResumeOutcome::Done => {
                let now_idle = {
                    let mut st = inner.lock();
                    st.outstanding -= 1;
                    st.outstanding == 0
                };
                if now_idle {
                    inner.idle.notify_all();
                }
            }
        }
    }
}

/// 向某个线程池派生协程的可克隆句柄。
///
/// 协程内部需派生新协程时，捕获一个 `Spawner`（克隆即可，内部是 `Arc`，天然 `Send`）。
#[derive(Clone)]
pub struct Spawner {
    inner: Arc<PoolInner>,
}

impl core::fmt::Debug for Spawner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Spawner").finish_non_exhaustive()
    }
}

impl Spawner {
    /// 向线程池派生一个协程。
    ///
    /// # Errors
    /// 栈分配失败时返回 [`StackError`]。
    pub fn spawn<F>(&self, f: F) -> Result<(), StackError>
    where
        F: FnOnce() + Send + 'static,
    {
        self.inner.enqueue(f)
    }
}

/// M:N 协程线程池：固定数量 OS worker 线程驱动共享就绪队列上的协程。
pub struct ThreadPool {
    inner: Arc<PoolInner>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl core::fmt::Debug for ThreadPool {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ThreadPool")
            .field("workers", &self.workers.len())
            .field("outstanding", &self.inner.lock().outstanding)
            .finish()
    }
}

impl ThreadPool {
    /// 创建含 `num_workers` 个 worker 线程的线程池（至少 1 个）。
    #[must_use]
    pub fn new(num_workers: usize) -> Self {
        let n = num_workers.max(1);
        let inner = Arc::new(PoolInner {
            state: Mutex::new(PoolState {
                queue: VecDeque::new(),
                outstanding: 0,
            }),
            work: Condvar::new(),
            idle: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let mut workers = Vec::with_capacity(n);
        for _ in 0..n {
            let inner = Arc::clone(&inner);
            workers.push(thread::spawn(move || worker_loop(&inner)));
        }
        Self { inner, workers }
    }

    /// 向线程池派生一个协程。
    ///
    /// # Errors
    /// 栈分配失败时返回 [`StackError`]。
    pub fn spawn<F>(&self, f: F) -> Result<(), StackError>
    where
        F: FnOnce() + Send + 'static,
    {
        self.inner.enqueue(f)
    }

    /// 取一个可克隆的派生句柄（供协程内部派生新协程）。
    #[must_use]
    pub fn spawner(&self) -> Spawner {
        Spawner {
            inner: Arc::clone(&self.inner),
        }
    }

    /// 阻塞直到所有已派生协程执行完毕（队列排空且无在途协程）。
    pub fn wait_idle(&self) {
        let mut st = self.inner.lock();
        while st.outstanding > 0 {
            st = self
                .inner
                .idle
                .wait(st)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// 关停：置关停标志、唤醒并 join 所有 worker。排空队列中剩余协程后退出。
    fn shutdown(&mut self) {
        self.inner.shutdown.store(true, Ordering::Release);
        self.inner.work.notify_all();
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}
