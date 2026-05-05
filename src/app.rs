use std::cell::RefCell;

use crate::{navigator::Navigator, *};

pub type AppLibRc = Rc<AppLib>;
pub type AppWeak = Weak<AppWindow>;

#[derive(Clone)]
pub struct App {
    pub app_ui: AppWeak,
    pub app_lib: AppLibRc,
    pub navigator: Rc<RefCell<Navigator>>,
}

impl App {
    pub fn new(app_ui: AppWeak, app_lib: AppLibRc) -> Self {
        Self {
            app_ui,
            app_lib,
            navigator: Rc::new(RefCell::new(Navigator::new())),
        }
    }

    pub fn bind_event(&self) {
        self.load_app_status();
        self.bind_playlist();
        self.bind_songlist();
        self.bind_player();
        self.bind_sidebar();
        self.bind_route(); 
        self.bind_play_queue();
        self.bind_lyrics();
        self.bind_lyrics_page();
    }

    pub fn load_app_status(&self) {
        let app = self.app_ui.upgrade().unwrap();

        if let Some(user) = self.app_lib.login_user.borrow().as_ref() {
            app.global::<AppStatus>().set_logined(true);
            let (id_1, id_2) = smt::u64_to_i32x2(user.id);
            app.global::<AppStatus>().set_login_id_1(id_1);
            app.global::<AppStatus>().set_login_id_2(id_2);
        } else {
            app.global::<AppStatus>().set_logined(false);
        }
    }
}

