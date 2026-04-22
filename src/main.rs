// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
pub mod login;
pub mod playlist;
pub mod songlist;
pub mod player;
pub mod types;
pub mod route;
pub mod sidebar;
pub mod app_runtime;


use i_slint_backend_winit::{WinitWindowAccessor};
use slint::{ComponentHandle, VecModel, Weak};
use smt::app_lib::AppLib;
use std::{
    error::Error, rc::Rc
};
use winit::platform::windows::WindowExtWindows;

use crate::route::Route;
pub type AppLibRc = Rc<AppLib>;
pub type AppWeak = Weak<AppWindow>;


slint::include_modules!();

fn load_app_status(app_weak: Weak<AppWindow>, app_lib: AppLibRc) {
    let app = app_weak.upgrade().unwrap();
    app
        .global::<PlayerStatus>()
        .set_event_loop_gap(app_lib.config.player_event_loop_gap_ms as i64);

    if let Some(user) = app_lib.login_user.borrow().as_ref() {
        app.global::<AppStatus>().set_logined(true);
        let (id_1, id_2) = smt::u64_to_i32x2(user.id);
        app.global::<AppStatus>().set_login_id_1(id_1);
        app.global::<AppStatus>().set_login_id_2(id_2);
    } else {
        app.global::<AppStatus>().set_logined(false);
    }
}

fn bind_event(app_weak: Weak<AppWindow>, app_lib: AppLibRc) {
    load_app_status(app_weak.clone(), app_lib.clone());
    playlist::bind_playlist_page(app_weak.clone(), app_lib.clone());
    songlist::bind_songlist(app_weak.clone(), app_lib.clone());
    player::bind_player(app_weak.clone(), app_lib.clone());
    sidebar::bind_sidebar(app_weak.clone(), app_lib.clone());
    Route::from_path("playlist/2115392436").unwrap().set_route(&app_weak, app_lib);
}


fn main() -> Result<(), Box<dyn Error>> {
    let app = AppWindow::new()?;
    let app_weak = app.as_weak();

    futures::executor::block_on(async move {
        
        let _ = slint::spawn_local({
            let app_weak = app_weak.clone();
            async move {
            app_weak
                .unwrap()
                .window()
                .winit_window()
                .await
                .unwrap()
                .set_system_backdrop(winit::platform::windows::BackdropType::MainWindow);
                println!("mica is ok");
        }});

        let app_lib = Rc::new(AppLib::new().await);
        if app_lib.login_user.borrow().is_none() {
            app_weak
                .upgrade()
                .unwrap()
                .global::<AppStatus>()
                .set_logined(false);
            app.global::<LoginProperty>().on_login_ready({
                let app_lib = app_lib.clone();
                let app_weak = app.as_weak();
                move || {
                    bind_event(app_weak.clone(), app_lib.clone());
                }
            });
            login::bind_login_page(app_weak.clone(), app_lib.clone());
        } else {
            app_weak
                .upgrade()
                .unwrap()
                .global::<AppStatus>()
                .set_logined(true);
            bind_event(app_weak.clone(), app_lib.clone());
        }


        app.window().on_winit_window_event({
            let app_weak = app_weak.clone();
            move |_window, window_event| {
                match window_event {
                    winit::event::WindowEvent::Focused(true) => {
                        app_weak.unwrap().invoke_set_foucs();
                    }
                    _ => {}
                }
                i_slint_backend_winit::EventResult::Propagate
            }
        });

        app.run().unwrap();
    });
    Ok(())
}
