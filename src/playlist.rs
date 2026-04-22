use crate::*;
use chrono::TimeZone;
use slint::Weak;
use smt::i32x2_to_u64;

pub fn ui_load(app_weak: Weak<AppWindow>, app_lib: AppLibRc, id: u64) {
    app_weak.unwrap().global::<AppStatus>().set_loading(true);

    let (id_1, id_2) = smt::u64_to_i32x2(id);
    app_weak
        .upgrade()
        .unwrap()
        .global::<PlaylistProperty>()
        .set_playlist_id_1(id_1);
    app_weak
        .upgrade()
        .unwrap()
        .global::<PlaylistProperty>()
        .set_playlist_id_2(id_2);
    let id = i32x2_to_u64(id_1, id_2);
    let app_weak = app_weak.clone();
    let app_lib = app_lib.clone();
    let _ = slint::spawn_local(async move {
        playlist_load_data(id, app_weak, app_lib).await;
    });
}

pub fn bind_playlist_page(_app_weak: Weak<AppWindow>, _app_lib: AppLibRc) {
    // let app = app_weak.upgrade().unwrap();

    // app.global::<PlaylistProperty>().on_load_data({
    //     let app_weak = app_weak.clone();
    //     let app_lib = app_lib.clone();
    //     move |id_1, id_2| {

    //     }
    // });
}

async fn playlist_load_data(id: u64, app_weak: Weak<AppWindow>, app_lib: AppLibRc) {
    let playlist = app_lib
        .get_playlist_cached(id)
        .await
        .unwrap();
    let track_details = app_lib.get_tracks_cached(&playlist.track_ids).await;
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

    app.global::<PlaylistProperty>().set_subscribable(
        playlist.creator.id
            != app_lib
                .login_user
                .borrow()
                .as_ref()
                .map_or(0, |user| user.id),
    );

    let time = chrono::Local
        .timestamp_millis_opt(playlist.create_time as i64)
        .unwrap();

    app.global::<PlaylistProperty>()
        .set_create_time(time.format("%Y-%m-%d").to_string().into());
    drop(app);

    if let Ok(playlist_cover_path) = app_lib
        .get_playlist_cover(id, &playlist.cover_img_url, 400)
        .await
    {
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
        .await
    {
        if let Ok(image) = slint::Image::load_from_path(&avatar_path) {
            app_weak
                .upgrade()
                .unwrap()
                .global::<PlaylistProperty>()
                .set_creator_avatar(image);
        }
    }
}



// mica is ok
// [perf][playlist_load_data][id=2115392436] get_playlist_cached: 1266 ms
// [perf][playlist_load_data][id=2115392436] get_tracks_cached: 939 ms
// [perf][playlist_load_data][id=2115392436] to_songlist_model+set_song_data: 2 ms
// [perf][playlist_load_data][id=2115392436] ui_bindings_update: 3 ms
// [perf][playlist_load_data][id=2115392436] get_playlist_cover: 0 ms
// [perf][playlist_load_data][id=2115392436] load_cover_image+set_cover: 18 ms
// [perf][playlist_load_data][id=2115392436] get_avatar: 12 ms
// [perf][playlist_load_data][id=2115392436] load_avatar+set_creator_avatar: 1 ms
// [perf][playlist_load_data][id=2115392436] total: 2243 ms
