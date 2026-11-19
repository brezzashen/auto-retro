//! CoreGraphics, CoreAnimation and Objective-C runtime calls of the WindowServer probe (`render_probe.rs`).
use std::ffi::{c_char, c_void};
type Id = *mut c_void;

#[link(name = "QuartzCore", kind = "framework")]
extern "C" {
    // Referenced so the linker keeps QuartzCore, which defines CAFenceHandle.
    static kCAFillModeForwards: Id;
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCreate(option: u32, relative_to: u32) -> *const c_void;
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayIsAsleep(display: u32) -> u32;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: *const c_void);
}
#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Id;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

/// Requests a fence from the default CA render server, the call apps block on, then drops it.
pub fn fence_round_trip() -> Result<(), &'static str> {
    unsafe {
        let _ = kCAFillModeForwards;
        let send: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let class = objc_getClass(c"CAFenceHandle".as_ptr());
        if class.is_null() {
            return Err("CAFenceHandle class unavailable");
        }
        let pool = objc_autoreleasePoolPush();
        let fence = send(class, sel_registerName(c"newFenceFromDefaultServer".as_ptr()));
        if !fence.is_null() {
            send(fence, sel_registerName(c"invalidate".as_ptr()));
            send(fence, sel_registerName(c"release".as_ptr()));
        }
        objc_autoreleasePoolPop(pool);
        if fence.is_null() { Err("render server returned no fence") } else { Ok(()) }
    }
}

/// Whether any online display is asleep (display sleep, not system sleep): a secondary
/// display waking on its own reconfigures WindowServer just like the main one.
pub fn display_asleep() -> bool {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: the buffer holds `ids.len()` entries and CoreGraphics writes at most that many.
    unsafe {
        if CGGetOnlineDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) != 0 {
            return false;
        }
        ids[..(count as usize).min(ids.len())].iter().any(|&id| CGDisplayIsAsleep(id) != 0)
    }
}

/// On-screen window IDs over WindowServer's main connection.
pub fn window_list_round_trip() {
    unsafe {
        let list = CGWindowListCreate(1, 0);
        if !list.is_null() {
            CFRelease(list);
        }
    }
}
