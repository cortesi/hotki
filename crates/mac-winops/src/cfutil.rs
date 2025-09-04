use core_foundation::{
    base::{CFTypeRef, TCFType},
    dictionary::CFDictionaryRef,
    number::CFNumber,
    string::{CFString, CFStringRef},
};

/// Borrow a CFStringRef and convert to Rust String.
pub(crate) fn cfstring_to_string(s: CFStringRef) -> String {
    // SAFETY: CFStringRef obtained from system APIs; wrap under get rule.
    let cf = unsafe { CFString::wrap_under_get_rule(s) };
    cf.to_string()
}

/// Get a String value for the given CFDictionary key.
pub(crate) fn dict_get_string(dict: CFDictionaryRef, key: CFStringRef) -> Option<String> {
    unsafe extern "C" {
        fn CFGetTypeID(cf: CFTypeRef) -> u64;
        fn CFStringGetTypeID() -> u64;
    }
    let value = unsafe {
        core_foundation::dictionary::CFDictionaryGetValue(dict, key as *const core::ffi::c_void)
    } as CFTypeRef;
    if value.is_null() {
        return None;
    }
    let is_string = unsafe { CFGetTypeID(value) == CFStringGetTypeID() };
    if !is_string {
        return None;
    }
    Some(cfstring_to_string(value as CFStringRef))
}

/// Get a 32-bit integer from CFDictionary for the given key.
pub(crate) fn dict_get_i32(dict: CFDictionaryRef, key: CFStringRef) -> Option<i32> {
    unsafe extern "C" {
        fn CFGetTypeID(cf: CFTypeRef) -> u64;
        fn CFNumberGetTypeID() -> u64;
    }
    let value = unsafe {
        core_foundation::dictionary::CFDictionaryGetValue(dict, key as *const core::ffi::c_void)
    } as CFTypeRef;
    if value.is_null() {
        return None;
    }
    if unsafe { CFGetTypeID(value) != CFNumberGetTypeID() } {
        return None;
    }
    let n = unsafe { CFNumber::wrap_under_get_rule(value as _) };
    n.to_i64().map(|v| v as i32)
}

/// Get an f64 from CFDictionary for the given key.
pub(crate) fn dict_get_f64(dict: CFDictionaryRef, key: CFStringRef) -> Option<f64> {
    unsafe extern "C" {
        fn CFGetTypeID(cf: CFTypeRef) -> u64;
        fn CFNumberGetTypeID() -> u64;
    }
    let value = unsafe {
        core_foundation::dictionary::CFDictionaryGetValue(dict, key as *const core::ffi::c_void)
    } as CFTypeRef;
    if value.is_null() {
        return None;
    }
    if unsafe { CFGetTypeID(value) != CFNumberGetTypeID() } {
        return None;
    }
    let n = unsafe { CFNumber::wrap_under_get_rule(value as _) };
    n.to_f64()
}

/// Get a bool from CFDictionary for the given key.
pub(crate) fn dict_get_bool(dict: CFDictionaryRef, key: CFStringRef) -> Option<bool> {
    unsafe extern "C" {
        fn CFGetTypeID(cf: CFTypeRef) -> u64;
        fn CFBooleanGetTypeID() -> u64;
        fn CFBooleanGetValue(b: CFTypeRef) -> bool;
    }
    let value = unsafe {
        core_foundation::dictionary::CFDictionaryGetValue(dict, key as *const core::ffi::c_void)
    } as CFTypeRef;
    if value.is_null() {
        return None;
    }
    if unsafe { CFGetTypeID(value) != CFBooleanGetTypeID() } {
        return None;
    }
    Some(unsafe { CFBooleanGetValue(value as _) })
}
