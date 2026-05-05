use std::{time::Duration};

use async_compat::CompatExt;
use slint::{Model, ModelRc, VecModel};
use smt::i32x2_to_u64;


use crate::*;

impl App {

    pub fn bind_lyrics(&self) {
        let app = self.clone();

        self.app_ui.unwrap().global::<LyricsPropery>().on_seek_to_line(move |index | {
            let time = app.app_lib.playing_lyrics
                .borrow()
                .as_ref()
                .unwrap()
                .lines
                .get(index as usize)
                .unwrap().time;
            app.app_lib.player_core.borrow_mut().seek_to_duration(time);
        });
    }

    pub fn lyrics_reload(&self) {
        let Some(reload_song_id) = self.app_lib.player_core.borrow().get_current_id() else {
            return;
        };

        let ui_song_id = i32x2_to_u64(
            self.app_ui.unwrap().global::<LyricsPageProperty>().get_id_1(),
            self.app_ui.unwrap().global::<LyricsPageProperty>().get_id_2()
        );
        
        if ui_song_id == reload_song_id {
            return ;
        }

        if reload_song_id == 0 {
            let app = self.app_ui.upgrade().unwrap();
            let property = app.global::<LyricsPropery>();
            property.set_lyrics(ModelRc::new(VecModel::default()));
            property.set_current_index(-1);
            return;
        }

        let app_weak = self.app_ui.clone();
        let app_lib = self.app_lib.clone();
        slint::spawn_local(async move {
            let Ok(lyrics) = app_lib.client.lyrics(reload_song_id).compat().await else {
                return;
            };

            if reload_song_id != app_lib.player_core.borrow().get_current_id().unwrap_or(0) {
                return;
            }

            app_lib.playing_lyrics.replace(Some(lyrics.clone()));
            let Some(app) = app_weak.upgrade() else { return; };
                        
            let lyrics_lines: Vec<LyricsLine> = lyrics
                .lines
                .into_iter()
                .map(|line| LyricsLine {
                    time: line.time.as_millis() as i32,
                    content: slint::SharedString::from(line.content.unwrap_or_default()),
                    translation: slint::SharedString::from(line.translation.unwrap_or_default()),
                })
                .collect();

            let model: ModelRc<LyricsLine> = ModelRc::new(VecModel::from(lyrics_lines));        
            app.global::<LyricsPropery>().set_lyrics(model);
        }).unwrap();
    }

    pub fn lyrics_update_position(&self, position: Duration) {
        if self.app_lib.playing_lyrics.borrow().is_none() {
            return;
        }

        let app = self.app_ui.upgrade().unwrap();
        let property = app.global::<LyricsPropery>();
        let lyrics_model = property.get_lyrics();
        let count = lyrics_model.row_count();
        
        let playing_lyrics_refcell = self.app_lib.playing_lyrics.borrow();
        let Some(playing_lyrics) = playing_lyrics_refcell.as_ref() else {
            return;
        };
        
        let Some(index) = playing_lyrics.current_index(position) else {
            property.set_current_index(-1);
            return;
        };
        drop(playing_lyrics_refcell);
        if index >= count {
            return;
        }

        property.set_current_index(index as i32);
    }
}
