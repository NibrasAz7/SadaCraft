//! Win32 layered-window overlay.
//!
//! One `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST` popup covers the
//! whole virtual screen (multi-monitor). Each frame is rendered by
//! [`Engine`] into a 32-bit top-down DIB section and pushed with
//! `UpdateLayeredWindow` (per-pixel alpha). Input model:
//!
//! - Normal mode: `WS_EX_TRANSPARENT` -> fully click-through. A
//!   `WH_MOUSE_LL` hook observes clicks for ripples (LL hooks see input even
//!   though the window is click-through).
//! - Pen mode: `WS_EX_TRANSPARENT` removed -> the window captures drags for
//!   freehand strokes. A `WH_KEYBOARD_LL` hook handles Esc (exit) and keys
//!   1-4 (pen color) regardless of focus.
//!
//! Global hotkeys: Ctrl+Alt+S show/hide, Ctrl+Alt+P pen mode, Ctrl+Alt+K
//! keystroke viz, Ctrl+Alt+C clear ink, Ctrl+Alt+Q quit.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::Config;
use crate::effects::Engine;

pub const CLASS_NAME: PCWSTR = w!("SadaSightOverlayWnd");
/// Cross-instance control message: wParam = toggle mask,
/// lParam = force-on mask | (force-off mask << 8). Bit order: ring,
/// ripple, trail, spotlight, pen, keys.
pub const WM_APP_EFFECTS: u32 = WM_APP + 1;

const TIMER_ID: usize = 1;
const FRAME_MS: u32 = 16;
const HK_TOGGLE: i32 = 1;
const HK_PEN: i32 = 2;
const HK_CLEAR: i32 = 3;
const HK_QUIT: i32 = 4;
const HK_KEYS: i32 = 5;

thread_local! {
    /// Clicks captured by the mouse LL hook; drained on the frame timer.
    static PENDING_CLICKS: RefCell<Vec<(i32, i32)>> = const { RefCell::new(Vec::new()) };
    /// Key vkCodes captured by the keyboard LL hook while pen mode is on.
    static PENDING_VK: Cell<u32> = const { Cell::new(0) };
    /// Key labels captured by the keyboard LL hook for keystroke viz.
    static PENDING_KEYS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Mirror of Overlay::pen_mode so hook procs can cheaply check it.
    static PEN_ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// Mirror of cfg.keys.enabled so the hook can cheaply check it.
    static KEYS_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

struct Overlay {
    hwnd: HWND,
    engine: Engine,
    mem_dc: HDC,
    dib: HBITMAP,
    /// Raw DIB-section pixels (top-down, premultiplied BGRA), `w*h*4` bytes.
    bits: *mut u8,
    /// Virtual-screen rect the window covers.
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    visible: bool,
    pen_mode: bool,
    mouse_hook: HHOOK,
    kbd_hook: HHOOK,
    cfg_path: PathBuf,
    tick: u64,
}

/// Blocking: creates the overlay window and runs the message loop until it
/// closes. `cfg_path` is where changed effect settings are persisted.
pub fn run(cfg: Config, cfg_path: PathBuf) -> Result<()> {
    unsafe {
        // Per-monitor-v2: window/cursor coordinates become physical pixels.
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            let _ = SetProcessDPIAware();
        }

        let (x, y, w, h) = virtual_screen();
        let hinst = HINSTANCE::from(GetModuleHandleW(None)?);
        let wc = WNDCLASSW {
            hInstance: hinst,
            lpszClassName: CLASS_NAME,
            lpfnWndProc: Some(wnd_proc),
            hCursor: LoadCursorW(None, IDC_CROSS)?,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(Error::from_thread());
        }

        let visible = cfg.visible;
        let start_pen = cfg.pen.enabled;
        let mut ov = Box::new(Overlay {
            hwnd: HWND::default(),
            engine: Engine::new(cfg, w, h),
            mem_dc: CreateCompatibleDC(None),
            dib: HBITMAP::default(),
            bits: std::ptr::null_mut(),
            x,
            y,
            w,
            h,
            visible,
            pen_mode: false,
            mouse_hook: HHOOK::default(),
            kbd_hook: HHOOK::default(),
            cfg_path,
            tick: 0,
        });

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("SadaSight"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst),
            Some(&*ov as *const Overlay as *const core::ffi::c_void),
        )?;
        ov.hwnd = hwnd;
        ov.create_dib()?;
        // Leak: the wndproc owns this pointer for the window's lifetime.
        let ov = Box::leak(ov);

        ov.mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_ll_hook), None, 0)?;
        ov.kbd_hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kbd_ll_hook), None, 0)?;
        KEYS_ACTIVE.set(ov.engine.cfg.keys.enabled);

        // Rasterize the chip-text glyph atlas once (GDI → alpha masks).
        ov.engine.set_glyphs(build_glyph_atlas());
        let (pm_w, pm_h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        ov.engine
            .set_key_area(-x as f32, -y as f32, (pm_w - x) as f32, (pm_h - y) as f32);

        let mods = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
        for (id, vk) in [
            (HK_TOGGLE, VK_S),
            (HK_PEN, VK_P),
            (HK_CLEAR, VK_C),
            (HK_QUIT, VK_Q),
            (HK_KEYS, VK_K),
        ] {
            if let Err(e) = RegisterHotKey(Some(hwnd), id, mods, vk.0 as u32) {
                eprintln!("[sadasight] RegisterHotKey Ctrl+Alt+{vk:?} failed: {e}");
            }
        }

        SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None);
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
        if start_pen {
            ov.set_pen_mode(true);
        }
        ov.paint();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }
    Ok(())
}

impl Overlay {
    /// (Re)create the backing DIB section at the current size.
    unsafe fn create_dib(&mut self) -> Result<()> {
        if !self.dib.0.is_null() {
            let _ = DeleteObject(HGDIOBJ::from(self.dib));
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.w,
                biHeight: -self.h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        self.dib =
            CreateDIBSection(Some(self.mem_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        SelectObject(self.mem_dc, HGDIOBJ::from(self.dib));
        self.bits = bits as *mut u8;
        Ok(())
    }

    /// Render one frame into the DIB and push it with UpdateLayeredWindow.
    unsafe fn paint(&mut self) {
        if self.bits.is_null() {
            return;
        }
        let buf =
            std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize);
        self.engine.render(buf, Instant::now());
        let size = SIZE {
            cx: self.w,
            cy: self.h,
        };
        let dst = POINT {
            x: self.x,
            y: self.y,
        };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            self.hwnd,
            None,
            Some(&dst),
            Some(&size),
            Some(self.mem_dc),
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
    }

    /// Frame timer: drain hook events, poll the cursor, repaint if dirty.
    unsafe fn tick(&mut self) {
        let now = Instant::now();
        PENDING_CLICKS.with(|c| {
            for (x, y) in c.borrow_mut().drain(..) {
                self.engine
                    .click((x - self.x) as f32, (y - self.y) as f32, now);
            }
        });
        match PENDING_VK.replace(0) {
            v if v == VK_ESCAPE.0 as u32 && self.pen_mode => self.set_pen_mode(false),
            v @ 0x31..=0x34 => self.engine.set_pen_color((v - 0x31) as usize),
            _ => {}
        }
        PENDING_KEYS.with(|k| {
            for label in k.borrow_mut().drain(..) {
                self.engine.key_press(label, now);
            }
        });

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mut dirty = self
            .engine
            .move_cursor((pt.x - self.x) as f32, (pt.y - self.y) as f32, now)
            | self.engine.animated()
            | self.engine.take_dirty();

        self.tick += 1;
        if self.tick % 120 == 0 {
            // Re-assert topmost and pick up display-layout changes.
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let (x, y, w, h) = virtual_screen();
            if (x, y, w, h) != (self.x, self.y, self.w, self.h) {
                self.resize(x, y, w, h);
                dirty = true;
            }
        }

        if dirty && self.visible {
            self.paint();
        }
    }

    /// Pen mode on = click-through off (window captures strokes).
    unsafe fn set_pen_mode(&mut self, on: bool) {
        self.pen_mode = on;
        self.engine.cfg.pen.enabled = on;
        PEN_ACTIVE.set(on);
        if !on {
            self.engine.pen_end();
        }
        let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
        let ex = if on {
            ex & !WS_EX_TRANSPARENT.0
        } else {
            ex | WS_EX_TRANSPARENT.0
        };
        SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex as isize);
        let _ = SetWindowPos(
            self.hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE,
        );
        self.engine.mark_dirty();
        self.save_cfg();
    }

    /// Apply a toggle/on/off effect mask triple (CLI + hotkeys + WM_APP).
    fn apply_effects(&mut self, toggle: u8, on: u8, off: u8) {
        let mut target = [false; 6];
        for i in 0..6 {
            let b = 1u8 << i;
            let cur = self.engine.cfg.effect_enabled(i);
            target[i] = if toggle & b != 0 {
                !cur
            } else if on & b != 0 {
                true
            } else if off & b != 0 {
                false
            } else {
                cur
            };
            if i != 4 {
                // Pen (4) is applied via set_pen_mode below.
                self.engine.cfg.set_effect(i, target[i]);
            }
        }
        KEYS_ACTIVE.set(target[5]);
        if target[4] != self.pen_mode {
            unsafe { self.set_pen_mode(target[4]) };
        }
        self.engine.mark_dirty();
        self.save_cfg();
    }

    unsafe fn toggle_visible(&mut self) {
        self.visible = !self.visible;
        self.engine.cfg.visible = self.visible;
        let _ = ShowWindow(self.hwnd, if self.visible { SW_SHOWNA } else { SW_HIDE });
        self.engine.mark_dirty();
        self.save_cfg();
    }

    /// Re-cover the (possibly changed) virtual screen rect.
    unsafe fn resize(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.x = x;
        self.y = y;
        self.w = w;
        self.h = h;
        if let Err(e) = self.create_dib() {
            eprintln!("[sadasight] DIB resize failed: {e}");
            return;
        }
        self.engine.resize(w, h);
        let (pm_w, pm_h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        self.engine
            .set_key_area(-x as f32, -y as f32, (pm_w - x) as f32, (pm_h - y) as f32);
        let _ = SetWindowPos(
            self.hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }

    fn save_cfg(&self) {
        if let Err(e) = self.engine.cfg.save(&self.cfg_path) {
            eprintln!("[sadasight] config save failed: {e}");
        }
    }
}

fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// lParam -> signed client x/y (WM_MOUSEMOVE etc.).
fn lp_x(lp: LPARAM) -> f32 {
    (lp.0 as i32 & 0xFFFF) as i16 as f32
}
fn lp_y(lp: LPARAM) -> f32 {
    ((lp.0 as i32 >> 16) & 0xFFFF) as i16 as f32
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Overlay;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let ov = &mut *ptr;
    match msg {
        WM_TIMER => {
            ov.tick();
            LRESULT(0)
        }
        WM_HOTKEY => {
            match wp.0 as i32 {
                HK_TOGGLE => ov.toggle_visible(),
                HK_PEN => ov.apply_effects(1 << 4, 0, 0),
                HK_KEYS => ov.apply_effects(1 << 5, 0, 0),
                HK_CLEAR => ov.engine.clear_ink(),
                HK_QUIT => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => return DefWindowProcW(hwnd, msg, wp, lp),
            }
            LRESULT(0)
        }
        m if m == WM_APP_EFFECTS => {
            ov.apply_effects(wp.0 as u8, (lp.0 & 0xFF) as u8, ((lp.0 >> 8) & 0xFF) as u8);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if ov.pen_mode {
                SetCapture(hwnd);
                ov.engine.pen_begin(lp_x(lp), lp_y(lp));
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if ov.pen_mode {
                ov.engine.pen_move(lp_x(lp), lp_y(lp));
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if ov.pen_mode {
                ov.engine.pen_end();
                let _ = ReleaseCapture();
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            let (x, y, w, h) = virtual_screen();
            ov.resize(x, y, w, h);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            for id in [HK_TOGGLE, HK_PEN, HK_CLEAR, HK_QUIT, HK_KEYS] {
                let _ = UnregisterHotKey(Some(hwnd), id);
            }
            if !ov.mouse_hook.0.is_null() {
                let _ = UnhookWindowsHookEx(ov.mouse_hook);
            }
            if !ov.kbd_hook.0.is_null() {
                let _ = UnhookWindowsHookEx(ov.kbd_hook);
            }
            if !ov.dib.0.is_null() {
                let _ = DeleteObject(HGDIOBJ::from(ov.dib));
                ov.dib = HBITMAP::default();
            }
            if !ov.mem_dc.0.is_null() {
                let _ = DeleteDC(ov.mem_dc);
                ov.mem_dc = HDC::default();
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Low-level mouse hook: observes clicks for the ripple effect even though
/// the overlay window is click-through. Events are queued thread-locally
/// (LL hooks dispatch on this thread's message loop) and applied on the
/// next frame tick.
unsafe extern "system" fn mouse_ll_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && !PEN_ACTIVE.get() {
        match wp.0 as u32 {
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
                let pt = (*(lp.0 as *const MSLLHOOKSTRUCT)).pt;
                PENDING_CLICKS.with(|c| c.borrow_mut().push((pt.x, pt.y)));
            }
            _ => {}
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

/// Low-level keyboard hook: while pen mode is active, Esc exits and keys
/// 1-4 select the pen color. Those keys are swallowed (return 1) so they
/// don't leak into the app underneath.
///
/// When the `keys` effect is on, key-down events are also observed for
/// keystroke viz — never swallowed (they're a display, not an intercept).
unsafe extern "system" fn kbd_ll_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && matches!(wp.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let vk = (*(lp.0 as *const KBDLLHOOKSTRUCT)).vkCode;
        if PEN_ACTIVE.get()
            && (vk == VK_ESCAPE.0 as u32 || (0x31..=0x34).contains(&vk))
        {
            PENDING_VK.set(vk);
            return LRESULT(1);
        }
        if KEYS_ACTIVE.get() {
            if let Some(label) = key_label(vk) {
                PENDING_KEYS.with(|k| k.borrow_mut().push(label));
            }
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

fn is_key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) < 0 }
}

/// vkCode → chip label ("CTRL+ALT+DEL" style). Modifiers alone produce no
/// chip; SHIFT is only shown when it joins a real shortcut (Ctrl/Alt/Win).
fn key_label(vk: u32) -> Option<String> {
    let base = vk_base_label(vk)?;
    let ctrl = is_key_down(VK_CONTROL);
    let alt = is_key_down(VK_MENU);
    let win = is_key_down(VK_LWIN) || is_key_down(VK_RWIN);
    let shift = is_key_down(VK_SHIFT) && (ctrl || alt || win);
    let mut label = String::new();
    for (on, name) in [
        (ctrl, "CTRL+"),
        (alt, "ALT+"),
        (shift, "SHIFT+"),
        (win, "WIN+"),
    ] {
        if on {
            label.push_str(name);
        }
    }
    label.push_str(&base);
    Some(label)
}

/// The key's own name, or `None` for modifiers/keys we don't display.
fn vk_base_label(vk: u32) -> Option<String> {
    match vk {
        // Letters/digits: vkCode is the ASCII code. Numpad 0-9 = 0x60-0x69.
        v @ 0x41..=0x5A => Some((v as u8 as char).to_string()),
        v @ 0x30..=0x39 => Some((v as u8 as char).to_string()),
        v @ 0x60..=0x69 => Some(((b'0' + (v - 0x60) as u8) as char).to_string()),
        v @ 0x70..=0x87 => Some(format!("F{}", v - 0x6F)), // F1-F24
        _ => {
            let name = match vk {
                x if x == VK_RETURN.0 as u32 => "ENTER",
                x if x == VK_SPACE.0 as u32 => "SPACE",
                x if x == VK_TAB.0 as u32 => "TAB",
                x if x == VK_ESCAPE.0 as u32 => "ESC",
                x if x == VK_BACK.0 as u32 => "BKSP",
                x if x == VK_DELETE.0 as u32 => "DEL",
                x if x == VK_INSERT.0 as u32 => "INS",
                x if x == VK_HOME.0 as u32 => "HOME",
                x if x == VK_END.0 as u32 => "END",
                x if x == VK_PRIOR.0 as u32 => "PGUP",
                x if x == VK_NEXT.0 as u32 => "PGDN",
                x if x == VK_LEFT.0 as u32 => "LEFT",
                x if x == VK_UP.0 as u32 => "UP",
                x if x == VK_RIGHT.0 as u32 => "RIGHT",
                x if x == VK_DOWN.0 as u32 => "DOWN",
                x if x == VK_CAPITAL.0 as u32 => "CAPS",
                x if x == VK_SNAPSHOT.0 as u32 => "PRTSC",
                x if x == VK_SCROLL.0 as u32 => "SCRLK",
                x if x == VK_PAUSE.0 as u32 => "PAUSE",
                x if x == VK_MULTIPLY.0 as u32 => "*",
                x if x == VK_ADD.0 as u32 => "+",
                x if x == VK_SUBTRACT.0 as u32 => "-",
                x if x == VK_DECIMAL.0 as u32 => ".",
                x if x == VK_DIVIDE.0 as u32 => "/",
                x if x == VK_OEM_PLUS.0 as u32 => "+",
                x if x == VK_OEM_MINUS.0 as u32 => "-",
                x if x == VK_OEM_COMMA.0 as u32 => ",",
                x if x == VK_OEM_PERIOD.0 as u32 => ".",
                x if x == VK_OEM_1.0 as u32 => ";",
                x if x == VK_OEM_2.0 as u32 => "/",
                x if x == VK_OEM_3.0 as u32 => "`",
                x if x == VK_OEM_4.0 as u32 => "[",
                x if x == VK_OEM_5.0 as u32 => "\\",
                x if x == VK_OEM_6.0 as u32 => "]",
                x if x == VK_OEM_7.0 as u32 => "'",
                _ => return None, // modifiers + anything unrecognized
            };
            Some(name.to_string())
        }
    }
}

/// Rasterize printable ASCII into alpha masks via GDI (one-time, at startup).
/// Each glyph is white text on black in a small DIB; the luminance channel
/// becomes the mask the engine stamps. Returns `(char, w, h, mask)` tuples.
unsafe fn build_glyph_atlas() -> Vec<(char, u8, u8, Vec<u8>)> {
    const CELL_H: i32 = 28;
    let mut out = Vec::new();
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        return out;
    }
    let font = CreateFontW(
        -20,
        0,
        0,
        0,
        FW_SEMIBOLD.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        CLEARTYPE_QUALITY,
        DEFAULT_PITCH.0 as u32 | FF_SWISS.0 as u32,
        w!("Segoe UI"),
    );
    if font.is_invalid() {
        let _ = DeleteDC(dc);
        return out;
    }
    SelectObject(dc, HGDIOBJ::from(font));
    let _ = SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, COLORREF(0x00FFFFFF)); // white text on black bg

    for code in 0x21u8..=0x7E {
        let c = code as char;
        let wch = [c as u16];
        let mut size = SIZE::default();
        if !GetTextExtentPoint32W(dc, &wch, &mut size).as_bool() || size.cx <= 0 {
            continue;
        }
        let cw = size.cx + 4;
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: cw,
                biHeight: -CELL_H,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            continue;
        };
        let old = SelectObject(dc, HGDIOBJ::from(bmp));
        let _ = PatBlt(dc, 0, 0, cw, CELL_H, BLACKNESS);
        let ty = ((CELL_H - size.cy) / 2).max(0);
        let _ = TextOutW(dc, 2, ty, &wch);
        let _ = GdiFlush();

        // Luminance of white-on-black = the alpha mask (BGRA: R at +2).
        let px = std::slice::from_raw_parts(bits as *const u8, (cw * CELL_H * 4) as usize);
        let mask: Vec<u8> = px.chunks_exact(4).map(|p| p[2]).collect();
        out.push((c, cw as u8, CELL_H as u8, mask));

        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ::from(bmp));
    }
    let _ = DeleteObject(HGDIOBJ::from(font));
    let _ = DeleteDC(dc);
    out
}
