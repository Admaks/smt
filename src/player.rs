
use chrono::Duration;
use smt::{
  i32x2_to_u64, player::PlayStatus, u64_to_i32x2
};

use crate::{app::App, *};
impl App {
    pub fn bind_player(&self) {
        let app_runtime = self.clone();
        
        self.app_ui.unwrap().global::<PlayerProperty>().on_event_loop({
            let app_runtime = app_runtime.clone();
            move || {
                app_runtime.event_loop();
            }
        });

        self.app_ui.unwrap().global::<PlayerProperty>().on_prev({
            let app_lib = app_runtime.app_lib.clone();
            move || {
                app_lib.player_core.borrow_mut().prev_song();
            }
        });

        self.app_ui.unwrap().global::<PlayerProperty>().on_next({
            let app_lib = app_runtime.app_lib.clone();
            move || {
                app_lib.player_core.borrow_mut().next_song();
            }
        });

        self.app_ui.unwrap().global::<PlayerProperty>().on_seek_to_duration({
            let app_lib = app_runtime.app_lib.clone();
            move |duration| {
                app_lib.player_core.borrow_mut().seek_to_duration(std::time::Duration::from_millis(duration as u64));
            }
        });

        self.app_ui.unwrap().global::<PlayerProperty>().on_pause_resume({
            let app_lib = app_runtime.app_lib.clone();
            move || {
                app_lib.player_core.borrow_mut().toggle_pause_resume();
            }
        });

        self.app_ui.unwrap().global::<PlayerProperty>().on_toggle_shuffle({
            let app_lib = app_runtime.app_lib.clone();
            move || {
                app_lib.player_core.borrow_mut().toggle_shuffle();
            }
        });

        self.app_ui.unwrap().global::<PlayerProperty>().on_toggle_repeat({
            let app_lib = app_runtime.app_lib.clone();
            move || {
                app_lib.player_core.borrow_mut().toggle_play_mode();
            }
        });

        self.app_ui.unwrap().global::<PlayerProperty>()
            .set_event_loop_gap(self.app_lib.config.player_event_loop_gap_ms as i64);
    }

    fn event_loop(&self) {
        {
            let current_id = self.app_lib.player_core.borrow().get_current_id();
            let app = self.app_ui.unwrap();
            let ui_player_status = app.global::<PlayerProperty>();
            let ui_playing_song = ui_player_status.get_playing_song();
            let ui_song_id = i32x2_to_u64(ui_playing_song.id_1, ui_playing_song.id_2);
        
            match current_id {
                Some(id) if id != ui_song_id => {
                    self.reload_playing_song(id);
                }
                None if 0 != ui_song_id ||
                    ui_player_status.get_status_tragger() != ui_player_status.get_status_none() => {
                    self.reload_playing_song(0);
                }
                _ => ()
            }
        }


        let frame = self.app_lib.player_core.borrow_mut().event_loop();
        let app = self.app_ui.unwrap();

            let duration = match frame.play_status {
              PlayStatus::Playing(duration) => {
                app.global::<PlayerProperty>().set_status_tragger(app.global::<PlayerProperty>().get_status_playing());
                Some(duration)
            },
              PlayStatus::Paused(duration) => {
                app.global::<PlayerProperty>().set_status_tragger(app.global::<PlayerProperty>().get_status_pause());
                Some(duration)
            },
              PlayStatus::Stopped => {
                app.global::<PlayerProperty>().set_status_tragger(app.global::<PlayerProperty>().get_status_none());
                None
            },
              PlayStatus::Downloading => {
                app.global::<PlayerProperty>().set_status_tragger(app.global::<PlayerProperty>().get_status_loading());
                None
            }
        };

        if let Some(duration) = duration {
            let duration = chrono::Duration::from_std(duration).unwrap();
            let played_duration_int = duration.num_milliseconds();
            let played_duration = Self::format_duration(duration);

            app.global::<PlayerProperty>().set_played_duration(played_duration.into());
            app.global::<PlayerProperty>().set_played_duration_int(played_duration_int as i32);
        }

        app.global::<PlayerProperty>().set_loop_type(match frame.play_order.play_mode {
            smt::player::PlayMode::LoopAll => 2,
            smt::player::PlayMode::LoopOne => 1,
            smt::player::PlayMode::Sequence => 0,
        });

        app.global::<PlayerProperty>()
            .set_shuffled(matches!(frame.play_order.shuffle_state, smt::player::ShuffleState::Enabled));

        drop(app);

    }

    fn format_duration(duration: chrono::Duration) -> String{
        let seconds = duration.num_seconds() % 60;
        let minites = duration.num_minutes() % 60;
        let hours = duration.num_hours();
        format!("{}:{:02}:{:02}", hours, minites, seconds)
    }

    fn reload_playing_song(&self, id: u64) {
        if id == 0 {
            let song = SongDetail {
                album: "".into(),
                album_id_1: 0,
                album_id_2: 0,
                duration: "0:00:00".into(),
                id_1: 0,
                id_2: 0,
                image: slint::Image::load_from_path(&self.app_lib.config.assets_dir.join("music.svg")).unwrap_or_default(),
                loved: false,
                name: "无音乐".into(),
                pic_url: "".into(),
                selected: false,
                singer: "".into()
            };
            self.app_ui.unwrap().global::<PlayerProperty>().set_playing_song(song);
        } else {
            let (id_1, id_2) = u64_to_i32x2(id);
            let loading_song = SongDetail {
                album: "".into(),
                album_id_1: 0,
                album_id_2: 0,
                duration: "0:00:00".into(),
                id_1,
                id_2,
                image: slint::Image::load_from_path(&self.app_lib.config.assets_dir.join("music.svg")).unwrap_or_default(),
                loved: false,
                name: "".into(),
                pic_url: "".into(),
                selected: false,
                singer: "".into()
            };

            self.app_ui.unwrap().global::<PlayerProperty>().set_playing_song(loading_song);

            let _ = slint::spawn_local({
                let app_weak = self.app_ui.clone();
                let app_lib = self.app_lib.clone();
                let id = id.clone();
                async move {
                    let mut track_detail = app_lib.get_tracks_cached(&[id]).await;
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
                    let total_duration = Self::format_duration(Duration::milliseconds(total_duration_int as i64));

                    app.global::<PlayerProperty>()
                        .set_total_duraiton(total_duration.into());
                    app.global::<PlayerProperty>()
                        .set_total_duration_int(total_duration_int as i32);
                    

                    let song_detail = types::
                        to_track_detail_detail(
                            app_lib.clone(),
                            track_detail.swap_remove(0),
                            default_image);

                    app.global::<PlayerProperty>().set_playing_song(song_detail);

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
                    let mut playing_song = app.global::<PlayerProperty>().get_playing_song();
                    playing_song.image = image;
                    app.global::<PlayerProperty>().set_playing_song(playing_song);
                }
            });
        }
    }
}   
