//! 协程栈分配。
//!
//! 每个协程独占一段 mmap 内存作为栈, 顶部 (高地址) 作为 SP 起点。
//! 底部保留一页 `PROT_NONE` 作为 guard page, 越界栈溢出会触发 `SIGSEGV`
//! 而不是悄悄踩坏邻接内存。

use core::ptr::NonNull;

dlsm_core::dlsm_error! {
    /// 栈分配错误（错误码区段 `20000..=29999`）。消息一律英文。
    pub enum StackError {
        /// 请求大小为 0。
        20001 ZeroSize => "stack size must be non-zero",
        /// `mmap` 系统调用失败。
        20002 Mmap(#[source] std::io::Error) => "mmap failed: {0}",
        /// `mprotect` 系统调用失败 (设置 guard page)。
        20003 Mprotect(#[source] std::io::Error) => "mprotect failed: {0}",
    }
}

/// 协程栈, 拥有底部 guard page。
///
/// 内存布局 (低 -> 高):
/// ```text
///   [guard page (PROT_NONE)] [usable stack ............... top]
/// ```
/// `top()` 返回 16 字节对齐的栈顶, 调用方按 ABI 约定向下写入 (SP 朝低地址生长)。
#[derive(Debug)]
pub struct Stack {
    base: NonNull<u8>,
    total_bytes: usize,
    guard_bytes: usize,
}

// SAFETY: Stack 持有的 mmap 内存仅在 Stack 内部访问; 转移所有权 (Send) 安全。
// 协程切换会跨线程恢复栈状态, 因此也要求 Sync。
unsafe impl Send for Stack {}
unsafe impl Sync for Stack {}

impl Stack {
    /// 分配一段大小至少为 `usable` 字节的协程栈, 并在底部预留一页 guard page。
    ///
    /// `usable` 会被向上对齐到页大小。
    ///
    /// # Errors
    /// - [`StackError::ZeroSize`] 当 `usable == 0`
    /// - [`StackError::Mmap`] 当 `mmap(2)` 失败
    /// - [`StackError::Mprotect`] 当 guard page 设置失败
    pub fn new(usable: usize) -> Result<Self, StackError> {
        if usable == 0 {
            return Err(StackError::ZeroSize);
        }

        let page = page_size();
        let usable_aligned = (usable + page - 1) & !(page - 1);
        let total_bytes = usable_aligned + page; // + guard page

        // SAFETY: 常量参数符合 mmap 文档; 返回值在下方 MAP_FAILED 检查。
        let raw = unsafe {
            libc::mmap(
                core::ptr::null_mut(),
                total_bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(StackError::Mmap(std::io::Error::last_os_error()));
        }
        // SAFETY: mmap 成功返回非空地址
        let base = unsafe { NonNull::new_unchecked(raw.cast::<u8>()) };

        // 底部一页设为 PROT_NONE 作为 guard
        // SAFETY: base 是有效的 mmap 起点, page 大小不超过 total_bytes
        let rc = unsafe { libc::mprotect(raw, page, libc::PROT_NONE) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            // SAFETY: base/total_bytes 来自上面成功 mmap; 唯一一次回收
            unsafe {
                libc::munmap(raw, total_bytes);
            }
            return Err(StackError::Mprotect(err));
        }

        Ok(Self {
            base,
            total_bytes,
            guard_bytes: page,
        })
    }

    /// 可用栈字节数 (不含 guard page)。
    #[inline]
    #[must_use]
    pub fn usable_size(&self) -> usize {
        self.total_bytes - self.guard_bytes
    }

    /// 栈顶指针 (高地址端, 16 字节对齐)。
    #[inline]
    #[must_use]
    pub fn top(&self) -> NonNull<u8> {
        // SAFETY: base + total_bytes 是 mmap 区紧邻其后的字节, 仅用作地址计算不做解引用;
        // total_bytes 是页大小的倍数, 因此 base + total_bytes 满足 16 字节对齐。
        let top_ptr = unsafe { self.base.as_ptr().add(self.total_bytes) };
        // SAFETY: top_ptr 是非空 (base 非空且偏移不溢出)
        unsafe { NonNull::new_unchecked(top_ptr) }
    }

    /// 可写栈底 (guard page 之后的第一个可用字节)。
    #[inline]
    #[must_use]
    pub fn bottom(&self) -> NonNull<u8> {
        // SAFETY: 偏移到 guard 后第一个字节, 仍在 mmap 区内
        let p = unsafe { self.base.as_ptr().add(self.guard_bytes) };
        unsafe { NonNull::new_unchecked(p) }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        // SAFETY: base/total_bytes 来自构造时成功的 mmap; 单次 drop 不会重复 unmap
        let rc =
            unsafe { libc::munmap(self.base.as_ptr().cast::<libc::c_void>(), self.total_bytes) };
        debug_assert_eq!(rc, 0, "munmap should not fail on a valid mapping");
    }
}

fn page_size() -> usize {
    // SAFETY: sysconf 是常量查询, _SC_PAGESIZE 永远存在
    let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    debug_assert!(v > 0);
    // 在 Linux 上 sysconf(_SC_PAGESIZE) 永远返回正值 (>= 4096); usize 必能容纳。
    usize::try_from(v).unwrap_or(4096)
}
