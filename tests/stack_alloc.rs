//! 协程栈分配测试。

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use dlsm_greenthread::Stack;

#[test]
fn creates_stack_with_requested_usable_size() {
    let stack = Stack::new(64 * 1024).expect("stack creation must succeed");
    // 可用字节数应 >= 请求的大小 (实现可能向上对齐到页)
    assert!(stack.usable_size() >= 64 * 1024);
}

#[test]
fn stack_top_is_aligned_to_16_bytes() {
    // System V AMD64 ABI 要求函数入口前栈 16 字节对齐
    let stack = Stack::new(16 * 1024).unwrap();
    let top = stack.top().as_ptr() as usize;
    assert_eq!(top % 16, 0);
}

#[test]
fn stack_rejects_zero_size() {
    let err = Stack::new(0).expect_err("zero must error");
    assert!(format!("{err}").contains("zero"));
}
