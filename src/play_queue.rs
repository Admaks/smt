use  crate::{*};

impl App {
    pub fn play_queue_ui_load(&self) {
        self.app_ui.unwrap().global::<AppStatus>().set_loading(true);

        let app_runtime = self.clone();
        slint::spawn_local(async move {
            app_runtime.play_queue_load_data().await;
        }).unwrap();
    }

    async fn play_queue_load_data(&self) {
        let player_core = self.app_lib.player_core.borrow();
        let (play_queue, base) = player_core.get_playlist();
        let play_queue = play_queue.iter().cloned().collect::<Vec<_>>();
        drop(player_core);
        
        let play_queue = self.app_lib.get_tracks_cached(&play_queue).await;

        let default_image_path = self.app_lib.config.assets_dir.join("music.svg");
        let model_rc = Self::to_songlist_model(self.app_lib.clone(), play_queue, &default_image_path);
        self.app_ui.upgrade().unwrap().global::<PlayQueueProperty>().set_queue(model_rc);



        let (base_id_1, base_id_2) = match base {
            smt::player::PlaylistBase::Playlist(id) => {
                self.app_ui.upgrade().unwrap().global::<PlayQueueProperty>().set_base_type(1);
                smt::u64_to_i32x2(id)
            },
            smt::player::PlaylistBase::Album(id) => {
                self.app_ui.upgrade().unwrap().global::<PlayQueueProperty>().set_base_type(2);
                smt::u64_to_i32x2(id)
            },
            smt::player::PlaylistBase::Artist(id) => {
                self.app_ui.upgrade().unwrap().global::<PlayQueueProperty>().set_base_type(3);
                smt::u64_to_i32x2(id)
            },
            smt::player::PlaylistBase::None => {
                self.app_ui.upgrade().unwrap().global::<PlayQueueProperty>().set_base_type(0);
                (0, 0)
            }
        };
        self.app_ui.upgrade().unwrap().global::<PlayQueueProperty>().set_base_id_1(base_id_1);
        self.app_ui.upgrade().unwrap().global::<PlayQueueProperty>().set_base_id_2(base_id_2);

        self.app_ui.upgrade().unwrap().global::<AppStatus>().set_loading(false);
    }
}

