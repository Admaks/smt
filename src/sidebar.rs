use async_compat::CompatExt;
use futures::{StreamExt, stream};
use slint::{Model, ModelExt, ModelRc};
use smt::{Config};

use crate::*;

#[repr(i32)]
enum SideBarItemType {
    Common = 0,
    Container = 1,
    Divider = 2,
}

impl App {
    fn apply_sidebar_cover_updates_batch(model_rc: &ModelRc<SideBarItemMessage>, pending: &mut Vec<(usize, slint::Image)>) {
        for (row, image) in pending.drain(..) {
            let mut t = model_rc.row_data_tracked(row).unwrap();
            t.icon = image;
            model_rc.set_row_data(row, t);
        }
    }

    pub fn bind_sidebar(&self) {
        let app_runtime = self.clone();
        let app_ui = self.app_ui.unwrap();
        app_ui.global::<SideBarProperty>().on_update({
            let app_runtime = app_runtime.clone();
            move || {
                slint::spawn_local({
                    let app_runtime = app_runtime.clone();
                    async move {
                        app_runtime.sidebar_load().await;
                    }
            }).unwrap();}
        });
        
        app_ui.global::<SideBarProperty>().on_expand({
            let app_runtime = app_runtime.clone();
            move |index| {
                app_runtime.siderbar_expand(index);
            }
        });

        app_ui.global::<SideBarProperty>().on_toggle({
            let app = app_runtime.clone();
            move |toggle| {
                if !toggle {
                    return;
                }
                app.sidebar_toggle();           
            }
        });
    }

    fn sidebar_toggle(&self) {
        let app_ui = self.app_ui.unwrap();
        let items = app_ui.global::<SideBarProperty>().get_sidebar_items();
        for i in 0..items.row_count() {
            if items.row_data(i).unwrap().r#type == SideBarItemType::Container as i32 {
                let mut container = items.row_data_tracked(i).unwrap();
                container.expand = false;
                items.set_row_data(i, container);
            }
        }
        self.sidebar_show();
    }

    fn siderbar_expand(&self, index: i32) {
        let app_ui = self.app_ui.unwrap();
        let items = app_ui.global::<SideBarProperty>().get_sidebar_items();

        let Some(mut container) = items.row_data_tracked(index.try_into().unwrap()) else {
            return;
        };

        container.expand = !container.expand;

        items.set_row_data(index.try_into().unwrap(), container);
        self.sidebar_show();
    }

    fn sidebar_show(&self) {
        let app_ui = self.app_ui.unwrap();
        let items = app_ui.global::<SideBarProperty>().get_sidebar_items();
        let show_items = app_ui.global::<SideBarProperty>().get_sidebar_items().iter().filter(|item| {
            let Ok(father) = item.father.try_into() else {
                return true;
            };
            
            let Some(parents) = items.row_data(father) else {
                return true;
            };

            parents.expand
        }).collect::<slint::VecModel<_>>();

        app_ui.global::<SideBarProperty>().set_sidebar_items_show(slint::ModelRc::new(show_items));
    }
    
    async fn sidebar_load(&self) {
        let Some(login_id) = self.app_lib.login_user.borrow().as_ref().map(|u| u.id) else {
            return;
        };

        let Ok(user_playlists) = self.app_lib.client.user_playlist(login_id).compat().await else {
            return;
        };

        let mut items: Vec<SideBarItemMessage> = Vec::new();
        let mut cover_jobs = Vec::new();

        let music_icon = slint::Image::load_from_path(&self.app_lib.config.assets_dir.join("music.svg"))
            .unwrap_or_default();

        let mut push_item = |mut item: SideBarItemMessage| -> usize {
            let row = items.len();
            item.index = row as i32;
            items.push(item);
            row
        };

        push_item(SideBarItemMessage {
            name: "主页".into(),
            route: "home".into(),
            iconfont: "\u{e710}".into(),
            icon: slint::Image::default(),
            father: -1,
            grade: 0,
            display: true,
            expand: false,
            r#type: SideBarItemType::Common as i32,
            index: 0,
        });

        push_item(SideBarItemMessage {
            name: "音乐库".into(),
            route: format!("playlist/{}", user_playlists.lovelist.id).into(),
            iconfont: "\u{e6fb}".into(),
            icon: slint::Image::default(),
            father: -1,
            grade: 0,
            display: true,
            expand: false,
            r#type: SideBarItemType::Common as i32,
            index: 0,
        });

        push_item(SideBarItemMessage {
            name: "".into(),
            route: "".into(),
            iconfont: "".into(),
            icon: slint::Image::default(),
            father: -1,
            grade: 0,
            display: true,
            expand: false,
            r#type: SideBarItemType::Divider as i32,
            index: 0,
        });

        let created_container_row = push_item(SideBarItemMessage {
            name: "创建的歌单".into(),
            route: "create_playlist".into(),
            iconfont: "\u{e6f3}".into(),
            icon: slint::Image::default(),
            father: -1,
            grade: 0,
            display: true,
            expand: false,
            r#type: SideBarItemType::Container as i32,
            index: 0,
        });

        for playlist in user_playlists.created {
            let row = push_item(SideBarItemMessage {
                name: playlist.name.into(),
                route: format!("playlist/{}", playlist.id).into(),
                icon: music_icon.clone(),
                iconfont: "".into(),
                father: created_container_row as i32,
                grade: 1,
                display: true,
                expand: false,
                r#type: SideBarItemType::Common as i32,
                index: 0,
            });

            cover_jobs.push((row, playlist.cover_img_id, playlist.cover_img_url));
        }

        let subscribed_container_row = push_item(SideBarItemMessage {
            name: "关注的歌单".into(),
            route: "subscribe_playlist".into(),
            iconfont: "\u{e6f3}".into(),
            icon: slint::Image::default(),
            father: -1,
            grade: 0,
            display: true,
            expand: false,
            r#type: SideBarItemType::Container as i32,
            index: 0,
        });

        for playlist in user_playlists.subscribed {
            let row = push_item(SideBarItemMessage {
                name: playlist.name.into(),
                route: format!("playlist/{}", playlist.id).into(),
                icon: music_icon.clone(),
                iconfont: "".into(),
                father: subscribed_container_row as i32,
                grade: 1,
                display: true,
                expand: false,
                r#type: SideBarItemType::Common as i32,
                index: 0,
            });

            cover_jobs.push((row, playlist.cover_img_id, playlist.cover_img_url));
        }

        let app_lib = self.app_lib.clone();
        let model_rc = ModelRc::new(slint::VecModel::from(items));
        self.app_ui.unwrap().global::<SideBarProperty>().set_sidebar_items(model_rc.clone());

        self.sidebar_show();
        tokio::task::yield_now().compat().await;

        let cover_fetches = stream::iter(cover_jobs.into_iter().map(|(row, playlist_id, url)| {
            let app_lib = app_lib.clone();
            async move {
                let Ok(path) = app_lib.get_playlist_cover(playlist_id, &url, 72).await else {
                    return None;
                };
                let Ok(image) = slint::Image::load_from_path(&path) else {
                    return None;
                };
                Some((row, image))
            }
        }))
        .buffer_unordered(Config::COVER_FETCH_CONCURRENCY);

        let mut pending = Vec::with_capacity(Config::COVER_UPDATE_BATCH);
        futures::pin_mut!(cover_fetches);
        let mut i = 0;
        while let Some(result) = cover_fetches.next().await {
            if let Some((row, image)) = result {
                pending.push((row, image));
                if pending.len() >= Config::COVER_UPDATE_BATCH {
                    Self::apply_sidebar_cover_updates_batch(&model_rc, &mut pending);
                }
            }
            i += 1;
            if i % 10 == 0 {
                self.sidebar_show();
            }
        }

        if !pending.is_empty() {
            Self::apply_sidebar_cover_updates_batch(&model_rc, &mut pending);
        }

        self.sidebar_show();
    }
}

