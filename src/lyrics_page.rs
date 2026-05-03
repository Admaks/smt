use crate::*;

impl App {
    pub fn bind_lyrics_page(&self) {
        let app = self.clone();

        self.app_ui.unwrap().global::<LyricsPageProperty>().on_show(move || {
            let app = app.clone();
            slint::spawn_local(async move {
                app.lyrics_page_show().await;
            }).unwrap();
        });
    }

    async fn lyrics_page_show(&self) {
        let app_ui = self.app_ui.unwrap();

        let Some(current_song) = self.app_lib.player_core.borrow().get_current_id() else {
            return;
        };

        let mut tracks = self.app_lib.get_tracks_cached(&[current_song]).await;
        
        if tracks.is_empty() {
            return;
        }

        let track = tracks.swap_remove(0);

        drop(tracks);

        let Ok(cover_path) = self.app_lib.get_album_cover(track.album.id, &track.album.pic_url, 600).await else {
            return;
        };

        app_ui.global::<LyricsPageProperty>().set_cover(
            slint::Image::load_from_path(&cover_path).unwrap_or_default()
        );

        let blur_cover_path = self.app_lib.get_blur_image_cached(&cover_path).await.unwrap_or(cover_path);

        app_ui.global::<LyricsPageProperty>().set_background(
            slint::Image::load_from_path(&blur_cover_path).unwrap_or_default()
        );

        app_ui.global::<LyricsPageProperty>().set_display(true);
    }
}

