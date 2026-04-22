use async_compat::CompatExt;
use futures::{StreamExt, stream};
use slint::{Model, ModelExt, ModelRc};
use smt::Config;

use crate::*;

impl App {
    fn apply_sidebar_cover_updates_batch(model_rc: &ModelRc<SideBarItemMessage>, pending: &mut Vec<(usize, slint::Image)>) {
        for (row, image) in pending.drain(..) {
            let mut t = model_rc.row_data_tracked(row).unwrap();
            t.cover = image;
            model_rc.set_row_data(row, t);
        }
    }

    pub fn bind_sidebar(&self) {
        let app_runtime = self.clone();
        self.app_ui.unwrap().global::<SideBarProperty>().on_update({
            let app_runtime = app_runtime.clone();
            move || {
                slint::spawn_local({
                    let app_runtime = app_runtime.clone();
                    async move {
                        app_runtime.update_sidebar().await;
                    }
            }).unwrap();}
        });

        // self.app_ui.unwrap().global::<AppStatus>().
    }

    async fn update_sidebar(&self) {
        let Some(login_id) = self.app_lib.login_user.borrow().as_ref().map(|u| u.id) else {
            return;
        };
        let Ok(user_playlists) = self.app_lib.client.user_playlist(login_id).compat().await else {
            return;
        };

        let created_playlists_cover = user_playlists.created.iter().enumerate().map(|(i, playlist)| {
            (i, playlist.cover_img_id, playlist.cover_img_url.clone())
        }).collect::<Vec<_>>();

        let subscribed_playlists_cover = user_playlists.subscribed.iter().enumerate().map(|(i, playlist)| {
            (i, playlist.cover_img_id, playlist.cover_img_url.clone())
        }).collect::<Vec<_>>();

        let created_playlists = ModelRc::new(
            user_playlists
            .created
            .into_iter()
            .map(|playlist| {
                SideBarItemMessage {
                    name : playlist.name.into(),
                    route : format!("playlist/{}", playlist.id).into(),
                    cover : slint::Image::load_from_path(&self.app_lib.config.assets_dir.join("music.svg")).unwrap()
                }
            }).collect::<slint::VecModel<_>>()
        );

        let subscribed_playlists = ModelRc::new(
            user_playlists
            .subscribed
            .into_iter()
            .map(|playlist| {
                SideBarItemMessage {
                    name : playlist.name.into(),
                    route : format!("playlist/{}", playlist.id).into(),
                    cover : slint::Image::load_from_path(&self.app_lib.config.assets_dir.join("music.svg")).unwrap()
                }
            }).collect::<slint::VecModel<_>>()
        );
        
        // println!("created playlists: {}, subscribed playlists: {}", created_playlists.row_count(), subscribed_playlists.row_count());
        self.app_ui.unwrap().global::<SideBarProperty>().set_created_playlist(created_playlists.clone());
        self.app_ui.unwrap().global::<SideBarProperty>().set_subscribed_playlist(subscribed_playlists.clone());

        let app_lib = self.app_lib.clone();
        slint::spawn_local(async move {
            let created_fetches = stream::iter(created_playlists_cover.into_iter().map(|(row, playlist_id, url)| {
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

            let mut created_pending = Vec::with_capacity(Config::COVER_UPDATE_BATCH);
            futures::pin_mut!(created_fetches);
            while let Some(result) = created_fetches.next().await {
                if let Some((row, image)) = result {
                    created_pending.push((row, image));
                    if created_pending.len() >= Config::COVER_UPDATE_BATCH {
                        Self::apply_sidebar_cover_updates_batch(&created_playlists, &mut created_pending);
                    }
                }
            }
            if !created_pending.is_empty() {
                Self::apply_sidebar_cover_updates_batch(&created_playlists, &mut created_pending);
            }

            let subscribed_fetches = stream::iter(subscribed_playlists_cover.into_iter().map(|(row, playlist_id, url)| {
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

            let mut subscribed_pending = Vec::with_capacity(Config::COVER_UPDATE_BATCH);
            futures::pin_mut!(subscribed_fetches);
            while let Some(result) = subscribed_fetches.next().await {
                if let Some((row, image)) = result {
                    subscribed_pending.push((row, image));
                    if subscribed_pending.len() >= Config::COVER_UPDATE_BATCH {
                        Self::apply_sidebar_cover_updates_batch(&subscribed_playlists, &mut subscribed_pending);
                    }
                }
            }
            if !subscribed_pending.is_empty() {
                Self::apply_sidebar_cover_updates_batch(&subscribed_playlists, &mut subscribed_pending);
            }
        }).unwrap();

    }
}

