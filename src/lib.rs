//! 用户态协程运行时。
//!
//! 通过汇编实现上下文切换（fast 模式仅保存通用寄存器，full 模式额外保存
//! SIMD 状态）。提供 P-Core / C-Core 双线程池，通过 `CoroutineHint` 标记
//! 协程类型并自动分发到对应池。
//!
//! 详细设计见 `docs/superpowers/specs/2026-05-22-bwtree-design.md`。

#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

#[cfg(target_arch = "x86_64")]
mod context;
#[cfg(target_arch = "x86_64")]
mod coroutine;
#[cfg(target_arch = "x86_64")]
mod park;
#[cfg(target_arch = "x86_64")]
mod scheduler;
mod stack;
#[cfg(target_arch = "x86_64")]
mod threadpool;

#[cfg(target_arch = "x86_64")]
pub use coroutine::{yield_now, Coroutine, ResumeOutcome};
#[cfg(target_arch = "x86_64")]
pub use park::CoroutinePark;
#[cfg(target_arch = "x86_64")]
pub use scheduler::{spawn, JoinHandle, Scheduler};
pub use stack::{Stack, StackError};
#[cfg(target_arch = "x86_64")]
pub use threadpool::{Spawner, ThreadPool};
