use async_compat::CompatExt;
use slint::{Model, ModelRc};

use crate::*;

pub fn bind_sidebar(app_weak: AppWeak, app_lib: AppLibRc) {
    app_weak.unwrap().global::<SideBarProperty>().on_update({
        let app_weak = app_weak.clone();
        let app_lib = app_lib.clone();
        move || {
            slint::spawn_local({
                let app_weak = app_weak.clone();
                let app_lib = app_lib.clone();
                async move {
                    update_sidebar(app_weak, app_lib).await;
                }
        }).unwrap();}
    });
}

async fn update_sidebar(app_weak: AppWeak, app_lib: AppLibRc) {
    let Some(login_id) = app_lib.login_user.borrow().as_ref().map(|u| u.id) else {
        return;
    };
    let Ok(user_playlists) = app_lib.client.user_playlist(login_id).compat().await else {
        return;
    };

    let created_playlists_cover = user_playlists.created.iter().enumerate().map(|(i, playlist)| {
        (i, playlist.cover_img_url.clone())
    });

    let subscribed_playlists_cover = user_playlists.subscribed.iter().enumerate().map(|(i, playlist)| {
        (i, playlist.cover_img_url.clone())
    });

    let created_playlists = ModelRc::new(
        user_playlists
        .created
        .into_iter()
        .map(|playlist| {
            SideBarItemMessage {
                name : playlist.name.into(),
                route : format!("playlist/{}", playlist.id).into(),
                cover : slint::Image::load_from_path(&app_lib.config.assets_dir.join("music.svg")).unwrap()
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
                cover : slint::Image::load_from_path(&app_lib.config.assets_dir.join("music.svg")).unwrap()
            }
        }).collect::<slint::VecModel<_>>()
    );
    
    // println!("created playlists: {}, subscribed playlists: {}", created_playlists.row_count(), subscribed_playlists.row_count());
    app_weak.unwrap().global::<SideBarProperty>().set_created_playlist(created_playlists);
    app_weak.unwrap().global::<SideBarProperty>().set_subscribed_playlist(subscribed_playlists);

    // futures::stream::iter(created_playlists_cover).ch;
}
