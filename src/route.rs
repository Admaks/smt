use slint::{ComponentHandle, Weak};
use crate::{playlist::ui_load, *};

#[repr(i32)]
pub enum Route {
    Playlist { 
        id: u64 
    } = 1,
}

impl Route{
    fn discriminant(&self) -> i32 {
        // SAFETY: Because `Self` is marked `repr(i32)`, its layout is a `repr(C)` `union`
        // between `repr(C)` structs, each of which has the `i32` discriminant as its first
        // field, so we can read the discriminant without offsetting the pointer.
        unsafe { *<*const _>::from(self).cast::<i32>() }
    }

    pub fn from_path(path: &str) -> Option<Self> {
        let mut parts = path.split('/');
        let page = parts.next()?;
        match page {
            "playlist" => {
                let id_str = parts.next()?;
                let id = id_str.parse().ok()?;
                Some(Route::Playlist { id })
            },
            _ => None,
        }
    }

    pub fn set_route(&self, app_weak: &Weak<AppWindow>, app_lib: AppLibRc) {
        match self {
            Route::Playlist { id } => {
                let app = app_weak.upgrade().unwrap();
                ui_load(app_weak.clone(), app_lib, *id);
                app.global::<AppStatus>().set_route(self.discriminant());
            },
        }
    }
}
