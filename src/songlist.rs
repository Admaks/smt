use std::{cmp::{max, min}, path::Path};
use futures::stream::{self, StreamExt};
use slint::{Model, ModelExt, ModelRc};
use smt::{i32x2_to_u64};
use crate::*;

use app::AppLibRc;
impl App {
    // 按批次写回封面，避免每张图片都触发一次高频 UI 更新。
    fn apply_song_cover_updates_batch(model_rc: &ModelRc<SongDetail>, pending: &mut Vec<(usize, slint::Image)>) {
        for (row, image) in pending.drain(..) {
            let mut t = model_rc.row_data_tracked(row).unwrap();
            t.image = image;
            model_rc.set_row_data(row, t);
        }
    }

    pub fn to_songlist_model(app_lib: AppLibRc, songs: Vec<smt::model::TrackDetail>, default_image_path: &Path) -> ModelRc<SongDetail> {
        // 初始化默认封面：网络封面未返回前先用本地图，保证列表首帧可用。
        let default_image = slint::Image::load_from_path(default_image_path).unwrap();

        // 先把后端歌曲结构转换成 UI 可直接绑定的 SongDetail。
        let vec_model = songs.into_iter().map(|song| {
            types::to_track_detail_detail(app_lib.clone(), song, default_image.clone())
        }).collect::<VecModel<_>>();

        let model_rc = ModelRc::new(vec_model);

        // 预提取封面任务参数，避免在异步任务中反复读取模型。
        let mut rows = Vec::with_capacity(model_rc.row_count());
        for i in 0..model_rc.row_count() {
            let t = model_rc.row_data(i).unwrap();
            rows.push((i, i32x2_to_u64(t.album_id_1, t.album_id_2), t.pic_url));
        }

        let model_rc_for_task = model_rc.clone();
        slint::spawn_local(async move {
            // 受限并发拉取封面，避免瞬时请求过多占满带宽和 IO。
            let fetches = stream::iter(rows.into_iter().map(|(row, album_id, url)| {
                let app_lib = app_lib.clone();
                async move {
                    let Ok(path) = app_lib.get_album_cover(album_id, &url, 72).await else {
                        return None;
                    };
                    let Ok(image) = slint::Image::load_from_path(&path) else {
                        return None;
                    };
                    Some((row, image))
                }
            }))
            .buffer_unordered(smt::Config::COVER_FETCH_CONCURRENCY); 

            let mut pending = Vec::with_capacity(smt::Config::COVER_UPDATE_BATCH);
            futures::pin_mut!(fetches);
            while let Some(result) = fetches.next().await {
                if let Some((row, image)) = result {
                    pending.push((row, image));
                    // 达到阈值再统一回写，降低 set_row_data 调用次数。
                    if pending.len() >= smt::Config::COVER_UPDATE_BATCH {
                        Self::apply_song_cover_updates_batch(&model_rc_for_task, &mut pending);
                    }
                }
            }

            // 收尾阶段刷掉最后一批不足阈值的更新。
            if !pending.is_empty() {
                Self::apply_song_cover_updates_batch(&model_rc_for_task, &mut pending);
            }
        }).unwrap();

        model_rc
    }

    // 处理列表播放：同源列表优先按选中项播放，否则重建播放队列。
    fn songlist_play(app_lib: AppLibRc, base_id_1: i32, base_id_2: i32, base_type: i32, model_rc: ModelRc<SongDetail>) {
        let base_id = i32x2_to_u64(base_id_1, base_id_2);

        // 根据页面来源确定基准类型，后续用于决定是增量播放还是重建队列。
        let id = match base_type {
            1 => smt::player::PlaylistBase::Playlist(base_id),
            2 => smt::player::PlaylistBase::Album(base_id),
            3 => smt::player::PlaylistBase::Artist(base_id),
            _ => smt::player::PlaylistBase::None,
        };

        let playlist: Vec<u64> = model_rc
            .iter()
            .filter_map(|data| {
                if data.selected {
                    Some(i32x2_to_u64(data.id_1, data.id_2))
                } else {
                    None
                }
            })
            .collect();

        if playlist.is_empty() {
            return;
        }

        let mut player_core = app_lib.player_core.borrow_mut();

        if id == *player_core.get_playlist_base() && playlist.len() == 1{
            player_core.play(playlist[0]);
        } else {
            let playlist_0 = playlist[0];
            let playlist_len = playlist.len();
            player_core.replace_playlist(playlist, id);
            if playlist_len == 1 {
                player_core.play(playlist_0);
            } else {
                player_core.resume();
            }
        }

        // 播放动作完成后清空选择，避免残留状态影响下一次交互。
        Self::songlist_clear_select(model_rc);
    }


    fn songlist_select(i: i32, select_type: i32, model_rc: ModelRc<SongDetail>) -> i32 {
        // 非法索引直接返回当前是否有选中项。
        let Ok(index): Result<usize, _> = i.try_into() else {
            return model_rc.iter().filter(|data| data.selected).count() as i32;
        };

        match select_type {
            // 单选：先清空已有选择，再切换当前项。
            0 => {
                let Some(selected) = model_rc.row_data(index).map(|x| x.selected) else {
                    return model_rc.iter().filter(|data| data.selected).count() as i32;
                };

                model_rc
                    .iter()
                    .enumerate()
                    .filter_map(|item| if item.1.selected { Some(item.0) } else { None })
                    .for_each(|row| {
                        if let Some(mut t) = model_rc.row_data_tracked(row) {
                            t.selected = false;
                            model_rc.set_row_data(row, t);
                        }
                    });

                if let Some(mut t) = model_rc.row_data_tracked(index) {
                    t.selected = !selected;
                    model_rc.set_row_data(index, t);
                }
            }
            // 加选：仅切换当前项。
            1 => {
                if let Some(mut t) = model_rc.row_data_tracked(index) {
                    t.selected = !t.selected;
                    model_rc.set_row_data(index, t);
                }
            }
            // 区间选：在已选区间和当前点击项之间全部设为选中。
            2 => {
                let Some(mut start) = model_rc.iter().position(|item| item.selected) else {
                    // 若此前无选中项，则退化为选中当前项。
                    if let Some(mut t) = model_rc.row_data_tracked(index) {
                        t.selected = true;
                        model_rc.set_row_data(index, t);
                    }
                    return model_rc.iter().filter(|data| data.selected).count() as i32;
                };

                let mut end: usize = 0;
                for row in (0..model_rc.row_count()).rev() {
                    if model_rc.row_data(row).map(|x| x.selected).unwrap_or(false) {
                        end = row;
                        break;
                    }
                }

                start = min(start, index);
                end = max(end, index);

                // 区间选择语义为闭区间 [start, end] 全部置为选中。
                for row in start..=end {
                    if let Some(mut t) = model_rc.row_data_tracked(row) {
                        t.selected = true;
                        model_rc.set_row_data(row, t);
                    }
                }
            }
            // 全选
            3 => {
                let selected = model_rc.iter().any(|data| !data.selected);
                for row in 0..model_rc.row_count() {
                    if let Some(mut t) = model_rc.row_data_tracked(row) {
                        t.selected = selected;
                        model_rc.set_row_data(row, t);
                    }
                }
            }
            _ => ()
        }

        model_rc.iter().filter(|data| data.selected).count() as i32
    }

    pub fn songlist_clear_select(model_rc: ModelRc<SongDetail>) {
        // 统一清空所有选中状态，供播放后或外部命令复位使用。
        for row in 0..model_rc.row_count() {
            let mut t = model_rc.row_data_tracked(row).unwrap();
            t.selected = false;
            model_rc.set_row_data(row, t);
        }
    }

    pub fn songlist_insert(app_lib: AppLibRc, model_rc: ModelRc<SongDetail>) {
        let playlist = model_rc
            .iter()
            .filter_map(|data| {
                if data.selected {
                    Some(i32x2_to_u64(data.id_1, data.id_2))
                } else {
                    None
                }
            })
            .collect::<Vec<u64>>();
        app_lib.player_core.borrow_mut().insert_songs(&playlist);
        Self::songlist_clear_select(model_rc);
    }

    pub fn bind_songlist(&self) {
        let app = self.app_ui.upgrade().unwrap();
        // 只在这里集中绑定回调，UI 事件分发到静态方法执行。
        app.global::<SongListProperty>().on_select(Self::songlist_select);
        app.global::<SongListProperty>().on_clear_select(Self::songlist_clear_select);
        app.global::<SongListProperty>().on_play({
            let app_lib = self.app_lib.clone();
            move |base_id_1, base_id_2, base_type ,model_rc | {
                Self::songlist_play(app_lib.clone(), base_id_1, base_id_2, base_type, model_rc);
            }
        });
        
        app.global::<SongListProperty>().on_insert({
            let app_lib = self.app_lib.clone();
            move |model_rc| {
                Self::songlist_insert(app_lib.clone(), model_rc);
            }
        });
    }
}
