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

/// Consume an owned CFStringRef (Create rule) and convert to Rust String.
///
/// Use this for values returned from functions that follow the Create/Copy rule
/// (e.g., `AXUIElementCopyAttributeValue`). Ownership is transferred to Rust
/// and released after conversion.
pub(crate) fn cfstring_to_string_copy(s: CFStringRef) -> String {
    // SAFETY: CFStringRef follows Create rule; wrap takes ownership.
    let cf = unsafe { CFString::wrap_under_create_rule(s) };
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

/// Get a nested dictionary for the given CFDictionary key.
pub(crate) fn dict_get_dict(dict: CFDictionaryRef, key: CFStringRef) -> Option<CFDictionaryRef> {
    unsafe extern "C" {
        fn CFGetTypeID(cf: CFTypeRef) -> u64;
        fn CFDictionaryGetTypeID() -> u64;
    }
    let value = unsafe {
        core_foundation::dictionary::CFDictionaryGetValue(dict, key as *const core::ffi::c_void)
    } as CFTypeRef;
    if value.is_null() {
        return None;
    }
    if unsafe { CFGetTypeID(value) != CFDictionaryGetTypeID() } {
        return None;
    }
    Some(value as CFDictionaryRef)
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

/// Get a 64-bit integer from CFDictionary for the given key.
pub(crate) fn dict_get_i64(dict: CFDictionaryRef, key: CFStringRef) -> Option<i64> {
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
    n.to_i64()
}

/// Get a boolean from CFDictionary for the given key.
pub(crate) fn dict_get_bool(dict: CFDictionaryRef, key: CFStringRef) -> Option<bool> {
    unsafe extern "C" {
        fn CFGetTypeID(cf: CFTypeRef) -> u64;
        fn CFBooleanGetTypeID() -> u64;
        fn CFBooleanGetValue(boolean: CFTypeRef) -> bool;
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
    let raw = unsafe { CFBooleanGetValue(value) };
    Some(raw)
}

// (no f64 numeric helper is currently required)

/// Read a CGRect-like dictionary from `dict[key]` and return (x, y, width, height) as i32.
/// The bounds dictionary uses CFString keys: "X", "Y", "Width", "Height" with CFNumber values.
pub(crate) fn dict_get_rect_i32(
    dict: CFDictionaryRef,
    key_bounds: CFStringRef,
) -> Option<(i32, i32, i32, i32)> {
    unsafe extern "C" {
        fn CFGetTypeID(cf: CFTypeRef) -> u64;
        fn CFDictionaryGetTypeID() -> u64;
        fn CFNumberGetTypeID() -> u64;
    }
    let b_any = unsafe {
        core_foundation::dictionary::CFDictionaryGetValue(
            dict,
            key_bounds as *const core::ffi::c_void,
        )
    } as CFTypeRef;
    if b_any.is_null() || unsafe { CFGetTypeID(b_any) != CFDictionaryGetTypeID() } {
        return None;
    }
    let bdict = b_any as CFDictionaryRef;
    let kx = CFString::from_static_string("X");
    let ky = CFString::from_static_string("Y");
    let kw = CFString::from_static_string("Width");
    let kh = CFString::from_static_string("Height");
    let get_i32 = |k: &CFString| -> Option<i32> {
        let v = unsafe {
            core_foundation::dictionary::CFDictionaryGetValue(
                bdict,
                k.as_concrete_TypeRef() as *const core::ffi::c_void,
            )
        } as CFTypeRef;
        if v.is_null() || unsafe { CFGetTypeID(v) != CFNumberGetTypeID() } {
            return None;
        }
        let n = unsafe { CFNumber::wrap_under_get_rule(v as _) };
        n.to_i64().map(|v| v as i32)
    };
    if let (Some(x), Some(y), Some(w), Some(h)) =
        (get_i32(&kx), get_i32(&ky), get_i32(&kw), get_i32(&kh))
    {
        Some((x, y, w, h))
    } else {
        None
    }
}
