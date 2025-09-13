use std::ffi::c_void;

use once_cell::sync::OnceCell;

type AxGetWindowFn = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;

static AX_GET_WINDOW_SYM: OnceCell<Option<AxGetWindowFn>> = OnceCell::new();

#[inline]
fn resolve_sym() -> Option<AxGetWindowFn> {
    AX_GET_WINDOW_SYM
        .get_or_init(|| unsafe {
            let name = b"_AXUIElementGetWindow\0";
            let ptr = libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr() as *const _);
            if ptr.is_null() {
                None
            } else {
                Some(std::mem::transmute::<_, AxGetWindowFn>(ptr))
            }
        })
        .clone()
}

/// Best-effort: resolve CGWindowID for an AX window element using the private
/// `_AXUIElementGetWindow` symbol. Returns `Some(id)` on success, or `None` if
/// the symbol is unavailable or the call fails.
pub fn window_id_for_ax_element(element: *mut c_void) -> Option<u32> {
    let f = resolve_sym()?;
    let mut id: u32 = 0;
    let rc = unsafe { f(element, &mut id as *mut u32) };
    if rc == 0 && id != 0 { Some(id) } else { None }
}
