#![no_std]

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn alloc(_len: i32) -> i32 {
    // A fixed scratch pointer is enough for no-op plugin tests because
    // on_request does not read the buffers. Host still performs write checks.
    1024
}

#[no_mangle]
pub extern "C" fn dealloc(_ptr: i32, _len: i32) {}

#[no_mangle]
pub extern "C" fn on_request(
    _method_ptr: i32,
    _method_len: i32,
    _path_ptr: i32,
    _path_len: i32,
) -> i32 {
    0
}
