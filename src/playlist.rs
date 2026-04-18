use crate::*;
use chrono::{TimeZone};
use slint::Weak;
use smt::{i32x2_to_u64};

pub fn bind_playlist_page(app_weak: Weak<AppWindow>, app_lib: AppLibRc) {
    let app = app_weak.upgrade().unwrap();

    app.global::<PlaylistProperty>().on_load_data({
        let app_weak = app_weak.clone();
        let app_lib = app_lib.clone();
        move |id_1, id_2| {
            let id = i32x2_to_u64(id_1, id_2);
            let app_weak = app_weak.clone();
            let app_lib = app_lib.clone();
            let _ = slint::spawn_local(async move {
                playlist_load_data(id, app_weak, app_lib).await;
            });
        }
    });
}

async fn playlist_load_data(id: u64, app_weak: Weak<AppWindow>, app_lib: AppLibRc) {
    let playlist = app_lib
        .client
        .playlist_detail(id, None)
        .await
        .unwrap();
    let track_details = app_lib.get_tracks(&playlist.track_ids).await;
    let default_image_path = app_lib.config.assets_dir.join("music.svg");

    let app = app_weak.upgrade().unwrap();
    app.global::<PlaylistProperty>()
        .set_name(playlist.name.into());
    // app.global::<PlaylistProperty>()
    //     .set_updated_time(playlist.create_time.try_into().unwrap());
    app.global::<PlaylistProperty>()
        .set_description(playlist.description.into());
    app.global::<PlaylistProperty>()
        .set_play_count(playlist.play_count.try_into().unwrap());
    app.global::<PlaylistProperty>()
        .set_subscribed(playlist.subscribed);
    app.global::<PlaylistProperty>()
        .set_track_count(playlist.track_count);
    app.global::<PlaylistProperty>()
        .set_creator_name(playlist.creator.nickname.into());
    
    app.global::<PlaylistProperty>()
        .set_song_data(songlist::to_songlist_model(
            track_details,
            &default_image_path,
            app_lib.clone(),
        ));
    
    app.global::<PlaylistProperty>()
        .set_subscribable(
            playlist.creator.id != 
            app_lib
            .login_user
            .borrow()
            .as_ref()
            .map_or(0, |user| user.id));

    let time = chrono::Local.timestamp_millis_opt(playlist.create_time as i64).unwrap();

    app.global::<PlaylistProperty>()
        .set_create_time(time.format("%Y-%m-%d").to_string().into());
    drop(app);

    if let Ok(playlist_cover_path) = app_lib
        .get_playlist_cover(id, &playlist.cover_img_url, 400)
        .await {    
        if let Ok(image) = slint::Image::load_from_path(&playlist_cover_path) {
            app_weak
            .upgrade()
            .unwrap()
            .global::<PlaylistProperty>()
            .set_cover(image);
        }
    }
    
    if let Ok(avatar_path) = app_lib
        .get_avatar(playlist.creator.id, &playlist.creator.avatar_url, 72)
        .await {
        if let Ok(image) = slint::Image::load_from_path(&avatar_path) {
            app_weak
            .upgrade()
            .unwrap()
            .global::<PlaylistProperty>()
            .set_creator_avatar(image);
        }
    }
}
