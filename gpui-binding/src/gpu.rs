//! GPU and device information, as the window's own platform handle reports it.
//!
//! `Window::gpu_specs()` (`gpui-0.2.2/src/window.rs:4377`) is public and answers an
//! `Option<GpuSpecs>`, whose fields are public (`gpui.rs:302-311`) — but it goes straight through to
//! the platform window (`window.rs:4378`), so only a frame can ask it and only a window has an
//! answer. This module is that answer, kept per window.
//!
//! **Nothing is reported until a frame has asked.** A reader outside a frame — `gos test`, or a
//! program asking before its first window — gets empty strings and `false`, not a placeholder: a
//! diagnostics panel that prints a device name nobody looked up is worse than one that prints
//! nothing.
//!
//! It is sampled **once per window**, on that window's first frame. The driver's name and version
//! do not change while a program runs, and the call is a trip through the platform layer, so asking
//! every frame would be a per-frame cost for a value that cannot have changed.

use std::cell::RefCell;
use std::collections::HashMap;

/// What the platform knows about the GPU behind one window.
#[derive(Clone, Default)]
pub struct Specs {
    /// Whether the GPU is really a fake, running on the CPU — `llvmpipe` and its like.
    pub software: bool,
    /// The device's name, as the driver reports it.
    pub device: String,
    pub driver: String,
    /// Further information about the driver.
    pub info: String,
    /// Whether a frame has asked. Until one has, every field is empty rather than absent, because a
    /// reader answers with values and not with an option.
    pub asked: bool,
}

thread_local! {
    static SPECS: RefCell<HashMap<i64, Specs>> = RefCell::new(HashMap::new());
}

/// Samples `win`'s GPU from the window itself, once.
///
/// Called at the top of a frame with the window in hand. A second call for a window that has already
/// been asked does nothing, so this is safe to call every frame.
pub fn sample(win: i64, window: &gpui::Window) {
    let asked = SPECS.with(|specs| {
        specs
            .borrow()
            .get(&win)
            .map(|specs| specs.asked)
            .unwrap_or(false)
    });
    if asked {
        return;
    }
    let sampled = window.gpu_specs();
    SPECS.with(|specs| {
        specs.borrow_mut().insert(
            win,
            match sampled {
                Some(gpu) => Specs {
                    software: gpu.is_software_emulated,
                    device: gpu.device_name,
                    driver: gpu.driver_name,
                    info: gpu.driver_info,
                    asked: true,
                },
                // Asked, and the platform had nothing to say: that is an answer too, and it is
                // remembered as one so the question is not asked again.
                None => Specs {
                    asked: true,
                    ..Default::default()
                },
            },
        )
    });
}

/// The names [`field`] answers to, in the order a program would print them.
///
/// The vocabulary, so a program can iterate it rather than hard-code it — and so that "the names are
/// exact" is a claim with something behind it: `field`'s matching cannot be exercised by a check
/// outside a frame, because an unasked window is empty before a name is ever looked at.
pub fn names() -> Vec<String> {
    ["device", "driver", "info"]
        .iter()
        .map(|name| name.to_string())
        .collect()
}

/// One field of `win`'s GPU, by name: `"device"`, `"driver"` or `"info"`.
///
/// The names are exact, as [`crate::view::window_option`]'s are, and anything else — a near miss, a
/// field that does not exist, an empty name — is `""` rather than a guess.
pub fn field(win: i64, name: &str) -> String {
    SPECS.with(|specs| {
        let specs = specs.borrow();
        let Some(specs) = specs.get(&win) else {
            return String::new();
        };
        match name {
            "device" => specs.device.clone(),
            "driver" => specs.driver.clone(),
            "info" => specs.info.clone(),
            _ => String::new(),
        }
    })
}

/// Whether `win`'s GPU is emulated on the CPU. `false` until a frame has asked.
pub fn is_software(win: i64) -> bool {
    SPECS.with(|specs| {
        specs
            .borrow()
            .get(&win)
            .map(|specs| specs.software)
            .unwrap_or(false)
    })
}
