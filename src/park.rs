//! 协程感知的锁等待策略。

use dlsm_sync::Park;

use crate::coroutine::yield_now;

/// 把 [`dlsm_sync::Park`] 注入点实现为协程让出：等待锁时调用 [`yield_now`]，
/// 让出调度器去跑其他就绪协程，而非空转占用 worker。
///
/// 用于 `McsLock::<CoroutinePark>::with_park()`。**仅可在协程上下文内使用**——
/// 在普通线程（无当前协程）上调用会 panic（见 [`yield_now`]）。
#[derive(Debug, Clone, Copy, Default)]
pub struct CoroutinePark;

impl Park for CoroutinePark {
    #[inline]
    fn park() {
        yield_now();
    }
}
