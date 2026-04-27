use crate::*;

use i_slint_backend_winit::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicI32, Ordering};
#[cfg(target_os = "windows")]
use std::sync::{Arc, OnceLock};
#[cfg(target_os = "windows")]
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::event::{ElementState, MouseButton, WindowEvent};

#[cfg(target_os = "windows")]
fn use_cross_platform_winit_drag() -> bool {
    cfg!(feature = "cross-platform-winit-drag")
}

#[derive(Clone, Copy, Debug)]
struct TitleBarRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl TitleBarRect {
    fn as_i32(self) -> (i32, i32, i32, i32) {
        (self.x as i32, self.y as i32, self.width as i32, self.height as i32)
    }

    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x
            && x < self.x + self.width
            && y >= self.y
            && y < self.y + self.height
    }
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct AtomicRect {
    x: AtomicI32,
    y: AtomicI32,
    width: AtomicI32,
    height: AtomicI32,
}

#[cfg(target_os = "windows")]
impl AtomicRect {
    fn set(&self, rect: TitleBarRect) {
        let (x, y, w, h) = rect.as_i32();
        self.x.store(x, Ordering::Relaxed);
        self.y.store(y, Ordering::Relaxed);
        self.width.store(w, Ordering::Relaxed);
        self.height.store(h, Ordering::Relaxed);
    }

    fn get(&self) -> (i32, i32, i32, i32) {
        (
            self.x.load(Ordering::Relaxed),
            self.y.load(Ordering::Relaxed),
            self.width.load(Ordering::Relaxed),
            self.height.load(Ordering::Relaxed),
        )
    }
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct HitTestState {
    title_rect: AtomicRect,
    // TODO: 后续若恢复 Win11 Snap Layout 的 HTMAXBUTTON 命中，可恢复 max_button_rect。
    // max_button_rect: AtomicRect,
}

#[cfg(target_os = "windows")]
static HIT_TEST_STATE: OnceLock<Arc<HitTestState>> = OnceLock::new();

#[cfg(target_os = "windows")]
fn hit_test_state() -> Arc<HitTestState> {
    HIT_TEST_STATE
        .get_or_init(|| Arc::new(HitTestState::default()))
        .clone()
}

// 预留标题栏拖拽热区，后续由外部逻辑传入真实位置和大小。
const RESERVED_TITLE_BAR_RECT: TitleBarRect = TitleBarRect {
    x: 0.0,
    y: 0.0,
    width: 1000.0,
    height:1500.0,
};

// 预留最大化按钮热区，后续由外部逻辑传入真实位置和大小。
// TODO: 回归 Slint 内部最大化按钮处理，暂不使用 Rust 侧最大化按钮命中区域。
// const RESERVED_MAX_BUTTON_RECT: TitleBarRect = TitleBarRect {
//     x: 0.0,
//     y: 0.0,
//     width: 0.0,
//     height: 0.0,
// };

#[cfg(target_os = "windows")]
mod win_title_hit_test {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::UI::Shell::{
        DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, HTCAPTION, WM_NCDESTROY, WM_NCHITTEST,
    };

    const SUBCLASS_ID: usize = 0x534D_5401;

    fn get_x_lparam(lp: LPARAM) -> i32 {
        (lp as i32 as i16) as i32
    }

    fn get_y_lparam(lp: LPARAM) -> i32 {
        ((lp as i32 >> 16) as i16) as i32
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _subclass_id: usize,
        _ref_data: usize,
    ) -> LRESULT {
        if msg == WM_NCHITTEST {
            let Some(state) = super::HIT_TEST_STATE.get() else {
                return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            };

            let x_screen = get_x_lparam(lparam);
            let y_screen = get_y_lparam(lparam);
            let mut window_rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };

            if unsafe { GetWindowRect(hwnd, &mut window_rect) } != 0 {
                let x = x_screen - window_rect.left;
                let y = y_screen - window_rect.top;
                let (rx, ry, rw, rh) = state.title_rect.get();
                // TODO: 后续若恢复 Win11 Snap Layout 的 HTMAXBUTTON 命中，在此增加最大化按钮区域判断。

                if x >= rx && x < rx + rw && y >= ry && y < ry + rh {
                    return HTCAPTION as LRESULT;
                }
            }
        }

        if msg == WM_NCDESTROY {
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            }
        }

        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    pub fn install(hwnd: HWND) -> Result<(), &'static str> {
        let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) };
        if ok == 0 {
            return Err("SetWindowSubclass failed");
        }
        Ok(())
    }
}

impl App {
    // TODO: 后续实现 Windows 11 最大化按钮的贴靠布局（分屏）支持
    // 需要处理 WM_NCHITTEST 并返回 HTMAXBUTTON

    fn ui_rect_to_hit_test_rect(&self, x: f64, y: f64, width: f64, height: f64) -> TitleBarRect {
        let scale_factor = self
            .app_ui
            .unwrap()
            .window()
            .with_winit_window(|winit_window| winit_window.scale_factor())
            .unwrap_or(1.0);

        TitleBarRect {
            x: (x * scale_factor).round(),
            y: (y * scale_factor).round(),
            width: (width * scale_factor).max(0.0).round(),
            height: (height * scale_factor).max(0.0).round(),
        }
    }

    pub fn set_title_bar_rect(&self, x: f64, y: f64, width: f64, height: f64) {
        #[cfg(target_os = "windows")]
        {
            hit_test_state()
                .title_rect
                .set(self.ui_rect_to_hit_test_rect(x, y, width, height));
        }
    }

    pub fn set_max_button_rect(&self, x: f64, y: f64, width: f64, height: f64) {
        // TODO: 回归 Slint 内部最大化按钮处理，暂不使用 Rust 侧最大化按钮区域。
        let _ = (x, y, width, height);
    }

    pub fn bind_win32_hit_test_regions(&self) {
        let title_rect = Rc::new(RefCell::new(RESERVED_TITLE_BAR_RECT));
        self.app_ui
            .unwrap()
            .global::<FrameProperty>()
            .on_update_title_bar_rect({
                let app = self.clone();
                let title_rect = title_rect.clone();
                move |x, y, width, height| {
                    let hit_test_rect = app.ui_rect_to_hit_test_rect(
                        x as f64,
                        y as f64,
                        width as f64,
                        height as f64,
                    );

                    *title_rect.borrow_mut() = hit_test_rect;
                    app.set_title_bar_rect(x as f64, y as f64, width as f64, height as f64);
                }
            });
        // TODO: 回归 Slint 内部最大化按钮处理，暂不监听 UI 的最大化按钮区域回调。

        {
            let app_ui = self.app_ui.clone();
            let title_rect = title_rect.clone();
            let last_cursor_pos = Rc::new(RefCell::new((0.0f64, 0.0f64)));
            let last_click_at = Rc::new(RefCell::new(None::<Instant>));
            const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(350);

            self.app_ui.unwrap().window().on_winit_window_event({
                let last_cursor_pos = last_cursor_pos.clone();
                let last_click_at = last_click_at.clone();
                move |_window, event| {
                    match event {
                        WindowEvent::CursorMoved { position, .. } => {
                            *last_cursor_pos.borrow_mut() = (position.x, position.y);
                        }
                        WindowEvent::MouseInput {
                            state: ElementState::Pressed,
                            button: MouseButton::Left,
                            ..
                        } => {
                            let (x, y) = *last_cursor_pos.borrow();
                            let title = *title_rect.borrow();

                            if title.contains(x, y) {
                                let now = Instant::now();
                                let is_double_click = last_click_at
                                    .borrow()
                                    .map(|t| now.duration_since(t) <= DOUBLE_CLICK_INTERVAL)
                                    .unwrap_or(false);
                                *last_click_at.borrow_mut() = Some(now);

                                if let Some(window) = app_ui.upgrade() {
                                    window.window().with_winit_window(|winit_window| {
                                        if is_double_click {
                                            winit_window.set_maximized(!winit_window.is_maximized());
                                        } else {
                                            let _ = winit_window.drag_window();
                                        }
                                    });
                                }

                                return i_slint_backend_winit::EventResult::PreventDefault;
                            }
                        }
                        _ => {}
                    }

                    i_slint_backend_winit::EventResult::Propagate
                }
            });
        }

        {
            let app_ui = self.app_ui.clone();
            slint::spawn_local(async move {
                let app = app_ui.unwrap();
                let winit_window = app.window().winit_window().await.unwrap();

                #[cfg(target_os = "windows")]
                {
                    if !use_cross_platform_winit_drag() {
                        let _ = hit_test_state();

                        if let Ok(window_handle) = winit_window.window_handle() {
                            if let RawWindowHandle::Win32(win32_handle) = window_handle.as_raw() {
                                let hwnd = win32_handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
                                let result = win_title_hit_test::install(hwnd);
                                match result {
                                    Ok(()) => {
                                        println!("[frame] win32 hit-test installed: hwnd={:?}", hwnd);
                                    }
                                    Err(err) => {
                                        println!("[frame] install title hit-test failed: {:?}", err);
                                    }
                                }
                            } else {
                                println!("[frame] install skipped: non-win32 raw handle");
                            }
                        } else {
                            println!("[frame] install skipped: window_handle unavailable");
                        }
                    } else {
                        println!("[frame] testing mode: using cross-platform winit drag path on windows");
                    }
                }

                #[cfg(not(target_os = "windows"))]
                let _ = winit_window;
            })
            .unwrap();
        }
    }

    pub fn bind_frame(&self) {
        self.set_title_bar_rect(
            RESERVED_TITLE_BAR_RECT.x,
            RESERVED_TITLE_BAR_RECT.y,
            RESERVED_TITLE_BAR_RECT.width,
            RESERVED_TITLE_BAR_RECT.height,
        );
        // TODO: 回归 Slint 内部最大化按钮处理，暂不初始化 Rust 侧最大化按钮区域。

        self.bind_win32_hit_test_regions();

        self.app_ui.unwrap().global::<FrameProperty>().on_minimize({
            let app_ui = self.app_ui.clone();
            move || {
                let app_ui = app_ui.clone();
                slint::spawn_local(async move {
                    let window = app_ui.unwrap();
                    let winit_window = window.window().winit_window().await.unwrap();
                    winit_window.set_minimized(true);
                })
                .unwrap();
            }
        });

        self.app_ui.unwrap().global::<FrameProperty>().on_maximize({
            let app_ui = self.app_ui.clone();
            move || {
                let app_ui = app_ui.clone();
                slint::spawn_local(async move {
                    let app_ui = app_ui.unwrap();
                    let winit_window = app_ui.window().winit_window().await.unwrap();
                    if winit_window.is_maximized() {
                        winit_window.set_maximized(false);
                        winit_window.set_resizable(true);
                    } else {
                        winit_window.set_maximized(true);
                        winit_window.set_resizable(false);
                        winit_window.set_resizable(true);
                    }                    
                })
                .unwrap();
            }
        });

        self.app_ui.unwrap().global::<FrameProperty>().on_listen_maximize({
            let app_ui = self.app_ui.clone();
            move || {
                app_ui.unwrap().window().is_maximized()
            }
        });

        self.app_ui.unwrap().global::<FrameProperty>().on_close({
            let app_ui = self.app_ui.clone();
            move || {
                let app_ui = app_ui.clone();
                app_ui.unwrap().window().with_winit_window(|_window| {
                    std::process::exit(0);
                });
            }
        });
    }
}

