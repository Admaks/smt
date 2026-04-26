use smt::Config;

use crate::{*};


impl App {
    pub fn bind_route(&self) {
        let app = self.clone();
        let app_ui = self.app_ui.unwrap();


        app_ui.global::<RouteProperty>().on_navigate(move |path | {
            if let Some(route) = Route::from_path(&path) {
                route.set_route(&app);
                true
            } else {
                false
            }
        });

        let app = self.clone();
        app_ui.global::<RouteProperty>().on_backward(move || {
            app.navigator.borrow_mut().back(&app).is_ok()
        });

        let app = self.clone();
        app_ui.global::<RouteProperty>().on_forward(move || {
            app.navigator.borrow_mut().forward(&app).is_ok()
        });
    }
}

#[repr(i32)]
pub enum Route {
    Home = 0,
    Playlist { 
        id: u64 
    } = 1,
    PlayQueue = 2,
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

    pub fn set_route(&self, app_runtime: &App) {
        match self {
            Route::Home => {
                let app = app_runtime.app_ui.upgrade().unwrap();
                app.global::<RouteProperty>().set_route(self.discriminant());
            },

            Route::Playlist { id } => {
                app_runtime.playlist_ui_load(*id);

                let app = app_runtime.app_ui.upgrade().unwrap();
                app.global::<RouteProperty>().set_route(self.discriminant());
            },

            Route::PlayQueue => {
                // app_runtime.playqueue_ui_load();

                let app = app_runtime.app_ui.upgrade().unwrap();
                app.global::<RouteProperty>().set_route(self.discriminant());
            },
        }
    }
}


pub struct Navigator { 
    history: Vec<Option<Route>>,
    start: usize,
    end: usize,
    current: usize,
}

impl Navigator {
    pub fn new() -> Self {
        Self {
            history: (0..Config::NAVIGATOR_HISTORY_MAX as usize).map(|_| None).collect(),
            start: 0,
            end: 0,
            current: 0,
        }
    }

    fn next(&self, index: usize) -> usize {
        (index + 1) % self.history.len()
    }

    fn prev(&self, index: usize) -> usize {
        (index + self.history.len() - 1) % self.history.len()
    }

    fn navigate_to_current(&self, app: &App) -> anyhow::Result<()> {
        if let Some(route) = &self.history[self.current] {
            route.set_route(app);
            Ok(())
        } else {
            Err(anyhow::anyhow!("No route at current history index"))
        }
    }

    pub fn back(&mut self, app: &App) -> anyhow::Result<()> {
        if self.current == self.start {
            return Err(anyhow::anyhow!("No more history"));
        }
        self.current = self.prev(self.current);

        self.navigate_to_current(app)
    }

    pub fn forward(&mut self, app: &App) -> anyhow::Result<()> {
        if self.current == self.end {
            return Err(anyhow::anyhow!("No more history"));
        }
        self.current = self.next(self.current);
        self.navigate_to_current(app)
    }

    pub fn to(&mut self, route: Route, app: &App) {
        let current = self.next(self.current);
        self.history[current] = Some(route);
        self.current = current;
        self.end = current;
        if current == self.start {
            self.start = self.next(self.start);
        }

        self.navigate_to_current(app).ok();
    }
}
