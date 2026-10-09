//! The application's name: the bold word macOS puts at the left of the menu bar.
//!
//! # What macOS actually draws there
//!
//! Upstream's `set_menus` example shows `set_menus` in the menu bar, and its
//! `text` example shows `text` — while `text.rs` names its own menu `"GPUI
//! Typography"` (`tools/upstream/examples/text.rs:303`). So the string is not
//! the menu's name, and it is not anything the example sets.
//!
//! `gpui` does set a title: `create_menu_bar` calls `menu.setTitle_(name)` and
//! `menu_item.setTitle_(name)` on the first menu it is handed
//! (`gpui-0.2.2/src/platform/mac/platform.rs:252-267`). AppKit ignores both for
//! that one item. Measured on this machine, item for item through the
//! Accessibility API, the application menu of each side of the `17_set_menus`
//! pair is `Services`, a separator, `Quit` — identical, key equivalents and
//! enabled states included — and the only difference anywhere in either menu
//! bar is that word.
//!
//! Three mechanisms were tried against a bare Cocoa binary and photographed from
//! inside its own run loop (so the app, not the shell that started it, was
//! frontmost):
//!
//! | mechanism | menu-bar result |
//! |---|---|
//! | nothing | the executable's name |
//! | `[NSProcessInfo setProcessName:]`, before `NSApplication` exists | the executable's name |
//! | the same, after `setMainMenu:` | the executable's name |
//! | `[NSMenuItem setTitle:]` on the application menu, after `setMainMenu:` | the executable's name |
//! | `CFBundleName` seeded into `[[NSBundle mainBundle] infoDictionary]` | **the seeded name** |
//! | the same, seeded again once the bar had been drawn | the executable's name |
//!
//! Two things follow. The name is read from the main bundle, which for a bare
//! binary has no `CFBundleName` at all — so the dictionary has to be given one,
//! and it has to be given one **before the menu bar is first drawn**. And
//! `setProcessName:` is not the lever: it changes what `NSProcessInfo` answers,
//! and not what is on screen.
//!
//! # Why the port has to set it at all
//!
//! A bare binary's application is named after the file it was executed from, and
//! upstream's examples are one binary each — `target/debug/examples/set_menus`
//! answers to `set_menus` with no code at all. Every port is run by the *same*
//! runner, `gos-runner`, and macOS will not let a process be renamed after the
//! fact, so `gos run --main-thread examples/17_set_menus.gos` would show
//! `gos-runner` where upstream shows `set_menus`.
//!
//! Seeding `CFBundleName` is the one supported way out, and it is a real rename
//! rather than a drawn-on label: the Apple menu's `Force Quit …` item follows it,
//! because that item is the same application name.
//!
//! # Only the name
//!
//! `[NSProcessInfo setProcessName:]` is deliberately **not** called. It does not
//! move the menu bar, and it would change what `ps -o comm=` answers — which is
//! how `scripts/pair.sh` tells the port's runner apart from the shell wrapper that
//! started it.

/// Gives the application `name`, as the menu bar will show it.
///
/// Must be called before `gpui::run`: the name has to be in place before the menu
/// bar is first drawn, and after that the window server keeps what it has. A
/// program that calls it later changes nothing.
pub fn set(name: &str) {
    #[cfg(target_os = "macos")]
    mac::set_bundle_name(name);
    #[cfg(not(target_os = "macos"))]
    let _ = name;
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_char, c_void, CString};

    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    /// The address of `objc_msgSend`, retyped as the signature of the call being
    /// made. Every Objective-C message send goes through this one symbol, whose
    /// real signature depends on the message, so the cast is the call.
    unsafe fn send0(receiver: *mut c_void, selector: *mut c_void) -> *mut c_void {
        let f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(receiver, selector) }
    }

    unsafe fn send1(
        receiver: *mut c_void,
        selector: *mut c_void,
        arg: *mut c_void,
    ) -> *mut c_void {
        let f: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(receiver, selector, arg) }
    }

    unsafe fn send2_void(
        receiver: *mut c_void,
        selector: *mut c_void,
        a: *mut c_void,
        b: *mut c_void,
    ) {
        let f: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(receiver, selector, a, b) }
    }

    /// `BOOL` — one byte on this ABI.
    unsafe fn send1_bool(
        receiver: *mut c_void,
        selector: *mut c_void,
        arg: *mut c_void,
    ) -> bool {
        let f: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> bool =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(receiver, selector, arg) }
    }

    fn class(name: &str) -> *mut c_void {
        let name = CString::new(name).expect("class name");
        unsafe { objc_getClass(name.as_ptr()) }
    }

    fn selector(name: &str) -> *mut c_void {
        let name = CString::new(name).expect("selector name");
        unsafe { sel_registerName(name.as_ptr()) }
    }

    /// `[NSString stringWithUTF8String:]` for a Rust string.
    fn nsstring(text: &str) -> *mut c_void {
        let text = CString::new(text).expect("string");
        unsafe {
            send1(
                class("NSString"),
                selector("stringWithUTF8String:"),
                text.as_ptr() as *mut c_void,
            )
        }
    }

    pub fn set_bundle_name(name: &str) {
        unsafe {
            let bundle = send0(class("NSBundle"), selector("mainBundle"));
            if bundle.is_null() {
                return;
            }
            let info = send0(bundle, selector("infoDictionary"));
            if info.is_null() {
                return;
            }
            // A bundled application's info dictionary is an immutable NSDictionary
            // read from its Info.plist; a bare binary's is the mutable one AppKit
            // makes up on the spot, and that is the one being seeded here. Asking
            // rather than assuming keeps this from messaging a class that would not
            // answer — which on this ABI is a crash, not an error.
            let setter = selector("setObject:forKey:");
            if !send1_bool(info, selector("respondsToSelector:"), setter) {
                return;
            }
            let text = nsstring(name);
            if text.is_null() {
                return;
            }
            send2_void(info, setter, text, nsstring("CFBundleName"));
        }
    }
}
