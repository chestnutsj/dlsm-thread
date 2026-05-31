//! 上下文切换原语 (`x86_64` System V ABI)。
//!
//! 仅保存调用者保留 (callee-saved) 寄存器 + RIP + RSP。XMM/AVX/x87 状态不保存,
//! 因此调用方有责任不在 yield 边界保留浮点中间值; 实测在 OLTP 路径几乎不影响。
//! 后续 `full` 模式 (TODO) 可补全 XMM/MXCSR 保存。

#![cfg(target_arch = "x86_64")]

use core::arch::global_asm;

/// 协程上下文 (8 个 8 字节寄存器槽)。布局必须与 `swap_context` 的偏移一致。
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Context {
    /// 0: 下一条指令地址 (恢复时跳转目标)。
    pub rip: usize,
    /// 8: 栈指针。
    pub rsp: usize,
    /// 16: RBX (callee-saved)。
    pub rbx: usize,
    /// 24: RBP (frame pointer, callee-saved)。
    pub rbp: usize,
    /// 32: R12 (callee-saved, 初始化时用来携带闭包指针)。
    pub r12: usize,
    /// 40: R13 (callee-saved)。
    pub r13: usize,
    /// 48: R14 (callee-saved)。
    pub r14: usize,
    /// 56: R15 (callee-saved)。
    pub r15: usize,
}

// 编译期断言大小一致 (字段偏移由 #[repr(C)] + 顺序 + 同类型 8B 字段保证)。
const _: [(); 64] = [(); core::mem::size_of::<Context>()];

global_asm!(
    r#"
// swap_context(rdi=save_to, rsi=load_from)
.global swap_context
swap_context:
    pop rax
    mov [rdi + 0],  rax
    mov [rdi + 8],  rsp
    mov [rdi + 16], rbx
    mov [rdi + 24], rbp
    mov [rdi + 32], r12
    mov [rdi + 40], r13
    mov [rdi + 48], r14
    mov [rdi + 56], r15

    mov rsp, [rsi + 8]
    mov rbx, [rsi + 16]
    mov rbp, [rsi + 24]
    mov r12, [rsi + 32]
    mov r13, [rsi + 40]
    mov r14, [rsi + 48]
    mov r15, [rsi + 56]
    jmp [rsi + 0]

// coroutine_entry: 由 swap_context 通过初始 Context.rip 跳入。
// r12 携带闭包指针 (在 Context.r12 中预置), 转发给 coroutine_main。
// 进入时 RSP 已经满足 (16k + 8) 的"被 call 后"对齐, 所以 jmp 直达即可。
.global coroutine_entry
coroutine_entry:
    mov rdi, r12
    jmp coroutine_main
"#
);

extern "C" {
    /// 保存当前寄存器到 `save_to`, 加载 `load_from` 的寄存器并跳到其 RIP。
    pub(crate) fn swap_context(save_to: *mut Context, load_from: *const Context);

    /// 协程的汇编入口; 不应直接调用, 仅作为 `Context.rip` 的初值。
    pub(crate) fn coroutine_entry();
}

#[cfg(test)]
mod layout_check {
    use super::Context;

    #[test]
    fn field_offsets_match_assembly_constants() {
        let mock = Context::default();
        let base = core::ptr::addr_of!(mock).cast::<u8>() as usize;
        assert_eq!(core::ptr::addr_of!(mock.rip) as usize - base, 0);
        assert_eq!(core::ptr::addr_of!(mock.rsp) as usize - base, 8);
        assert_eq!(core::ptr::addr_of!(mock.rbx) as usize - base, 16);
        assert_eq!(core::ptr::addr_of!(mock.rbp) as usize - base, 24);
        assert_eq!(core::ptr::addr_of!(mock.r12) as usize - base, 32);
        assert_eq!(core::ptr::addr_of!(mock.r13) as usize - base, 40);
        assert_eq!(core::ptr::addr_of!(mock.r14) as usize - base, 48);
        assert_eq!(core::ptr::addr_of!(mock.r15) as usize - base, 56);
    }
}
