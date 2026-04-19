
use chrono::Duration;
use smt::{
    i32x2_to_u64,
    player::{CurrentSongStatus, PlayerStatusKind},
    u64_to_i32x2,
};

use crate::*;

pub fn bind_player(app_weak: AppWeak, app_lib: AppLibRc) {
    app_weak.unwrap().global::<PlayerStatus>().on_event_loop({
        let app_weak = app_weak.clone();
        let app_lib = app_lib.clone();
        move || {
            event_loop(app_weak.clone(), app_lib.clone());
        }
    });

    app_weak.unwrap().global::<PlayerStatus>().on_prev({
        let app_lib = app_lib.clone();
        move || {
            app_lib.player_core.borrow_mut().prev_song();
        }
    });

    app_weak.unwrap().global::<PlayerStatus>().on_next({
        let app_lib = app_lib.clone();
        move || {
            app_lib.player_core.borrow_mut().next_song();
        }
    });

    app_weak.unwrap().global::<PlayerStatus>().on_seek_to_duration({
        let app_lib = app_lib.clone();
        move |duration| {
            app_lib.player_core.borrow_mut().seek_to_duration(std::time::Duration::from_millis(duration as u64));
        }
    });

    app_weak.unwrap().global::<PlayerStatus>().on_pause_resume({
        let app_lib = app_lib.clone();
        move || {
            app_lib.player_core.borrow_mut().toggle_pause_resume();
        }
    });
}


fn event_loop(app_weak: AppWeak, app_lib: AppLibRc) {
    {
        let current_id = app_lib.player_core.borrow().get_current_id();
        let app = app_weak.unwrap();
        let ui_player_status = app.global::<PlayerStatus>();
        let ui_playing_song = ui_player_status.get_playing_song();
        let ui_song_id = i32x2_to_u64(ui_playing_song.id_1, ui_playing_song.id_2);
    
        match current_id {
            Some(id) if id != ui_song_id => {
                reload_playing_song(app_weak.clone(), app_lib.clone(), id);
            }
            None if 0 != ui_song_id ||
                ui_player_status.get_status_tragger() != ui_player_status.get_status_none() => {
                reload_playing_song(app_weak.clone(), app_lib.clone(), 0);
            }
            _ => ()
        }
    }

    let frame = app_lib.player_core.borrow_mut().event_loop();

    let app = app_weak.unwrap();
    let duration = match frame.player_status {
        PlayerStatusKind::Playing => {
            app.global::<PlayerStatus>().set_status_tragger(app.global::<PlayerStatus>().get_status_playing());
            match frame.current_song_status {
                CurrentSongStatus::Position(duration) => Some(duration),
                _ => None,
            }
        },
        PlayerStatusKind::Paused => {
            app.global::<PlayerStatus>().set_status_tragger(app.global::<PlayerStatus>().get_status_pause());
            match frame.current_song_status {
                CurrentSongStatus::Position(duration) => Some(duration),
                _ => None,
            }
        },
        PlayerStatusKind::Stopped => {
            app.global::<PlayerStatus>().set_status_tragger(app.global::<PlayerStatus>().get_status_none());
            None
        },
        PlayerStatusKind::Downloading => {
            app.global::<PlayerStatus>().set_status_tragger(app.global::<PlayerStatus>().get_status_loading());
            None
        }
    };

    if let Some(duration) = duration {
        let duration = chrono::Duration::from_std(duration).unwrap();
        let played_duration_int = duration.num_milliseconds();
        let played_duration = format_duration(duration);

        app.global::<PlayerStatus>().set_played_duration(played_duration.into());
        app.global::<PlayerStatus>().set_played_duration_int(played_duration_int as i32);
    }
    drop(app);

}

fn format_duration(duration: chrono::Duration) -> String{
    let seconds = duration.num_seconds() % 60;
    let minites = duration.num_minutes() % 60;
    let hours = duration.num_hours();
    format!("{}:{:02}:{:02}", hours, minites, seconds)
}


fn reload_playing_song(app_weak: AppWeak, app_lib: AppLibRc, id: u64) {
    if id == 0 {
        let song = SongDetail {
            album: "".into(),
            album_id_1: 0,
            album_id_2: 0,
            duration: "0:00:00".into(),
            id_1: 0,
            id_2: 0,
            image: slint::Image::load_from_path(&app_lib.config.assets_dir.join("music.svg")).unwrap_or_default(),
            loved: false,
            name: "无音乐".into(),
            pic_url: "".into(),
            selected: false,
            singer: "".into()
        };
        app_weak.unwrap().global::<PlayerStatus>().set_playing_song(song);
    } else {
        let (id_1, id_2) = u64_to_i32x2(id);
        let loading_song = SongDetail {
            album: "".into(),
            album_id_1: 0,
            album_id_2: 0,
            duration: "0:00:00".into(),
            id_1,
            id_2,
            image: slint::Image::load_from_path(&app_lib.config.assets_dir.join("music.svg")).unwrap_or_default(),
            loved: false,
            name: "".into(),
            pic_url: "".into(),
            selected: false,
            singer: "".into()
        };

        app_weak.unwrap().global::<PlayerStatus>().set_playing_song(loading_song);

        let _ = slint::spawn_local({
            let app_weak = app_weak.clone();
            let app_lib = app_lib.clone();
            let id = id.clone();
            async move {
                let mut track_detail = app_lib.get_tracks(&[id]).await;
                let default_image = slint::Image::load_from_path(
                        &app_lib.config.assets_dir.join("music.svg"))
                    .unwrap_or_default();

                let app = app_weak.unwrap();

                let Some(current_id) = app_lib.player_core.borrow().get_current_id() else {
                    return;
                };

                if current_id  != id {
                    return;
                }

                let pic_url = track_detail[0].album.pic_url.clone();
                let cover = app_lib
                    .get_album_cover(track_detail[0].album.id, &pic_url, 100);

                let total_duration_int = track_detail[0].duration;
                let total_duration = format_duration(Duration::milliseconds(total_duration_int as i64));

                app.global::<PlayerStatus>()
                    .set_total_duraiton(total_duration.into());
                app.global::<PlayerStatus>()
                    .set_total_duration_int(total_duration_int as i32);
                

                let song_detail = types::
                    to_track_detail_detail(
                        app_lib.clone(),
                        track_detail.swap_remove(0),
                        default_image);

                app.global::<PlayerStatus>().set_playing_song(song_detail);

                drop(app);

                let Ok(path) = cover.await else {
                    return;
                };

                let Ok(image)= slint::Image::load_from_path(&path) else {
                    return;
                };

                let Some(current_id) = app_lib.player_core.borrow().get_current_id() else {
                    return;
                };

                if current_id  != id {
                    return;
                }

                let app = app_weak.unwrap();
                let mut playing_song = app.global::<PlayerStatus>().get_playing_song();
                playing_song.image = image;
                app.global::<PlayerStatus>().set_playing_song(playing_song);
            }
        });
    }
}

