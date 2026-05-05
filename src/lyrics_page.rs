use smt::{i32x2_to_u64, u64_to_i32x2};

use crate::*;

impl App {
    pub fn bind_lyrics_page(&self) {
        let app = self.clone();

        self.app_ui.unwrap().global::<LyricsPageProperty>().on_show(move || {
            app.lyrics_page_show();
        });
    }

    pub fn lyrics_page_reload(&self) {
        let app_runtime = self.clone();
        slint::spawn_local(async move {
            app_runtime.lyrics_page_reload_async().await;
        }).unwrap();
    }
    
    async fn lyrics_page_reload_async(&self) {
        let app_ui = self.app_ui.unwrap();

        // 读取歌词页当前显示的歌曲 ID，如果已经是当前播放歌曲则无需重载
        let ui_song_id = i32x2_to_u64(
            app_ui.global::<LyricsPageProperty>().get_id_1(),
            app_ui.global::<LyricsPageProperty>().get_id_2(),
        );
        if ui_song_id == self.app_lib.player_core.borrow().get_current_id().unwrap_or(0) {
            return;
        }

        // 重新获取歌词数据
        self.lyrics_reload();

        let Some(reload_song_id) = self.app_lib.player_core.borrow().get_current_id() else {
            return;
        };

        // 更新歌词页显示的歌曲 ID
        let (ui_id_1, ui_id_2) = u64_to_i32x2(reload_song_id);
        app_ui.global::<LyricsPageProperty>().set_id_1(ui_id_1);
        app_ui.global::<LyricsPageProperty>().set_id_2(ui_id_2);

        // 歌曲可能在异步操作期间切换，每次 await 后检查是否仍是同一首歌
        let check_song_still_current = || {
            self.app_lib
                .player_core
                .borrow()
                .get_current_id()
                .is_some_and(|id| id == reload_song_id)
        };

        let mut tracks = self.app_lib.get_tracks_cached(&[reload_song_id]).await;
        if !check_song_still_current() || tracks.is_empty() {
            return;
        }

        let track = tracks.swap_remove(0);
        drop(tracks);

        // 获取封面
        let Ok(cover_path) = self
            .app_lib
            .get_album_cover(track.album.id, &track.album.pic_url, 600)
            .await
        else {
            return;
        };
        if !check_song_still_current() {
            return;
        }

        app_ui.global::<LyricsPageProperty>().set_cover(
            slint::Image::load_from_path(&cover_path).unwrap_or_default(),
        );

        // 获取模糊背景
        let blur_cover_path = self
            .app_lib
            .get_blur_image(&cover_path)
            .await
            .unwrap_or(cover_path);
        if !check_song_still_current() {
            return;
        }

        app_ui.global::<LyricsPageProperty>().set_background(
            slint::Image::load_from_path(&blur_cover_path).unwrap_or_default(),
        );
    }

    fn lyrics_page_show(&self) {
        let app_ui = self.app_ui.unwrap();
        self.lyrics_page_reload();
        app_ui.global::<LyricsPageProperty>().set_display(!app_ui.global::<LyricsPageProperty>().get_display());
    }
}
