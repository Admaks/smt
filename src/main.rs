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
            app_weak
                .unwrap()
                .window()
                .winit_window()
                .await
                .unwrap()
                .set_system_backdrop(winit::platform::windows::BackdropType::MainWindow);
        }}).unwrap();


        let app_lib = Rc::new(AppLib::new().await);
        let app_runtime = App::new(app_weak.clone(), app_lib.clone());
        
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


        // app.window().on_winit_window_event({
        //     let app_weak = app_weak.clone();
        //     move |_window, window_event| {
        //         match window_event {
        //             winit::event::WindowEvent::Focused(true) => {
        //                 app_runtime.app_weak.upgrade().unwrap().invoke_set_foucs();
        //             }
        //             _ => {}
        //         }
        //         i_slint_backend_winit::EventResult::Propagate
        //     }
        // });

        app.run().unwrap();
    });
    Ok(())
}
