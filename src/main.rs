// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
pub mod login;
pub mod playlist;
pub mod songlist;
pub mod player;
pub mod types;
pub mod navigator;
pub mod sidebar;
pub mod app;
pub mod play_queue;
pub mod frame;


use i_slint_backend_winit::{WinitWindowAccessor};
use slint::{ComponentHandle, VecModel, Weak};
use smt::app_lib::AppLib;
use std::{
    error::Error, rc::Rc
};
use winit::platform::windows::WindowExtWindows;

use crate::app::App;


slint::include_modules!();


fn main() -> Result<(), Box<dyn Error>> {
    let app = AppWindow::new()?;
    let app_weak = app.as_weak();

    futures::executor::block_on(async move {
        slint::spawn_local({
            let app_weak = app_weak.clone();
            async move {
            let winit_window = app_weak
                .unwrap()
                .window()
                .winit_window()
                .await
                .unwrap();

            winit_window.set_system_backdrop(winit::platform::windows::BackdropType::MainWindow);
            winit_window.set_undecorated_shadow(true);
            winit_window.set_decorations(false);
            winit_window.set_resizable(true);
        }}).unwrap();


        let app_lib = Rc::new(AppLib::new().await);
        let app_runtime = App::new(app_weak.clone(), app_lib.clone());
        
        app_runtime.bind_frame();

        if app_lib.login_user.borrow().is_none() {
            app_weak
                .upgrade()
                .unwrap()
                .global::<AppStatus>()
                .set_logined(false);
            app.global::<LoginProperty>().on_login_ready({
                let app_runtime = app_runtime.clone();
                move || {
                    app_runtime.bind_event();
                }
            });
            app_runtime.bind_login_page();
        } else {
            app_weak
                .upgrade()
                .unwrap()
                .global::<AppStatus>()
                .set_logined(true);
            app_runtime.bind_event();
        }

        app.run().unwrap();
    });
    Ok(())
}
