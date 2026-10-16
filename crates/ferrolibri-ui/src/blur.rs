//! Window blur: a transparent window with the OS acrylic backdrop (Windows).
//!
//! iced's wgpu surface is opaque by default. Presenting through DirectComposition
//! (`WGPU_DX12_PRESENTATION_SYSTEM=visual` on the DX12 backend) makes it
//! premultiplied-alpha capable; the patched crates in `vendor/` honour that
//! variable (see vendor/README.md). Elsewhere the window stays opaque.
use std::sync::atomic::{AtomicBool, Ordering};

static READY: AtomicBool = AtomicBool::new(false);
/// Lowest window opacity the slider offers (percent).
pub const MIN_OPACITY: u8 = 10;
/// Window and panel fills while the window is translucent: darker than the
/// opaque palette, because the acrylic layer lifts the backdrop (about 52/255
/// over black) and the glass look needs to stay near-black.
pub const GLASS_BACKGROUND: iced::Color = iced::Color::from_rgb8(14, 14, 16);
pub const GLASS_PANEL: iced::Color = iced::Color::from_rgb8(20, 21, 24);
/// Light tint of the acrylic layer (R, G, B, A); the fill alpha does the darkening.
pub const ACRYLIC_TINT: (u8, u8, u8, u8) = (18, 18, 21, 60);

/// Select DX12 with a DirectComposition swap chain before iced starts.
/// Explicit user choices (environment variables) are respected; blur is then
/// only offered when they amount to the same configuration.
pub fn prepare_environment() {
    #[cfg(target_os = "windows")]
    {
        for (key, value) in [
            ("WGPU_BACKEND", "dx12"),
            ("WGPU_DX12_PRESENTATION_SYSTEM", "visual"),
        ] {
            if std::env::var_os(key).is_none() {
                // SAFETY: called first thing in `main`, before any thread exists.
                unsafe { std::env::set_var(key, value) };
            }
        }
        let is = |key: &str, accepted: &[&str]| {
            std::env::var(key)
                .map(|v| accepted.contains(&v.to_lowercase().as_str()))
                .unwrap_or(false)
        };
        READY.store(
            is("WGPU_BACKEND", &["dx12"])
                && is(
                    "WGPU_DX12_PRESENTATION_SYSTEM",
                    &["visual", "dxgifromvisual"],
                ),
            Ordering::Relaxed,
        );
    }
}
/// Whether the window was (or will be) created able to show transparency.
pub fn ready() -> bool {
    READY.load(Ordering::Relaxed)
}
/// Alpha of the window and panel fills for an opacity of 10-100 %. `active` is
/// false until the window really is translucent, so it never flashes transparent.
pub fn fill_alpha(opacity: u8, active: bool) -> f32 {
    if !active {
        return 1.0;
    }
    f32::from(opacity.clamp(MIN_OPACITY, 100)) / 100.0
}
#[cfg(target_os = "windows")]
pub fn apply(handle: &dyn iced::window::Window, on: bool) -> Result<(), String> {
    use iced::window::raw_window_handle::RawWindowHandle;
    // Only one backdrop may be active; clear the DWM system backdrops first.
    match window_vibrancy::clear_mica(handle) {
        Ok(()) | Err(window_vibrancy::Error::UnsupportedPlatformVersion(_)) => {}
        Err(error) => return Err(error.to_string()),
    }
    window_vibrancy::clear_acrylic(handle).map_err(|e| e.to_string())?;
    let hwnd = match handle.window_handle().map_err(|e| e.to_string())?.as_raw() {
        RawWindowHandle::Win32(win32) => win32.hwnd.get(),
        _ => return Err("Not a Win32 window".into()),
    };
    set_accent(hwnd, if on { ACCENT_ACRYLIC } else { ACCENT_DISABLED })
}
#[cfg(target_os = "windows")]
const ACCENT_DISABLED: u32 = 0;
#[cfg(target_os = "windows")]
const ACCENT_ACRYLIC: u32 = 4;
/// `SetWindowCompositionAttribute` with an accent policy. Unlike the DWM system
/// backdrop, this acrylic keeps blurring while the window is not focused.
#[cfg(target_os = "windows")]
fn set_accent(hwnd: isize, state: u32) -> Result<(), String> {
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        gradient_color: u32,
        animation_id: u32,
    }
    #[repr(C)]
    struct Data {
        attribute: u32,
        data: *mut std::ffi::c_void,
        size: usize,
    }
    type SetWindowCompositionAttribute = unsafe extern "system" fn(isize, *mut Data) -> i32;
    const WCA_ACCENT_POLICY: u32 = 19;
    let (r, g, b, a) = ACRYLIC_TINT;
    let mut policy = AccentPolicy {
        state,
        flags: 0,
        // ABGR; acrylic ignores a fully transparent tint.
        gradient_color: u32::from(r)
            | (u32::from(g) << 8)
            | (u32::from(b) << 16)
            | (u32::from(a.max(1)) << 24),
        animation_id: 0,
    };
    let mut data = Data {
        attribute: WCA_ACCENT_POLICY,
        data: (&mut policy as *mut AccentPolicy).cast(),
        size: std::mem::size_of::<AccentPolicy>(),
    };
    // SAFETY: user32 is always loaded in a GUI process; the function pointer type
    // matches the (undocumented but long-stable) signature and `policy`/`data`
    // outlive the call.
    unsafe {
        let user32 = libloading::Library::new("user32.dll").map_err(|e| e.to_string())?;
        let set: libloading::Symbol<SetWindowCompositionAttribute> = user32
            .get(b"SetWindowCompositionAttribute\0")
            .map_err(|e| e.to_string())?;
        if set(hwnd, &mut data) == 0 {
            return Err("SetWindowCompositionAttribute failed".into());
        }
    }
    Ok(())
}
#[cfg(not(target_os = "windows"))]
pub fn apply(_: &dyn iced::window::Window, _: bool) -> Result<(), String> {
    Err("Window blur is only available on Windows".into())
}

pub struct BlurState {
    /// The window was created able to show transparency.
    pub ready: bool,
    /// The backdrop is currently applied to the window.
    pub applied: bool,
    pub pending: bool,
    /// The OS refused it; the window stays opaque for the session.
    pub failed: bool,
}
impl Default for BlurState {
    fn default() -> Self {
        Self {
            ready: ready(),
            applied: false,
            pending: false,
            failed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alpha_is_opaque_until_active_and_follows_opacity() {
        assert_eq!(fill_alpha(15, false), 1.0);
        assert_eq!(fill_alpha(100, true), 1.0);
        assert!((fill_alpha(15, true) - 0.15).abs() < 1e-6);
        assert!((fill_alpha(0, true) - 0.1).abs() < 1e-6);
        assert_eq!(fill_alpha(250, true), 1.0);
        let steps: Vec<f32> = (10..=100).step_by(5).map(|s| fill_alpha(s, true)).collect();
        assert!(steps.windows(2).all(|w| w[0] < w[1]));
    }
}
