// Accessibility trust check (ApplicationServices)

pub fn ax_is_process_trusted() -> bool {
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}
