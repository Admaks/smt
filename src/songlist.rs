use std::{cmp::{max, min}, path::Path};
use futures::stream::{self, StreamExt};
use slint::{Model, ModelExt, ModelRc};
use smt::{i32x2_to_u64};
use crate::*;

fn apply_cover_updates_batch(model_rc: &ModelRc<SongDetail>, pending: &mut Vec<(usize, slint::Image)>) {
    for (row, image) in pending.drain(..) {
        let mut t = model_rc.row_data_tracked(row).unwrap();
        t.image = image;
        model_rc.set_row_data(row, t);
    }
}

pub fn to_songlist_model(songs: Vec<smt::model::TrackDetail>, default_image_path: &Path, app_lib : AppLibRc) -> ModelRc<SongDetail> {
    let default_image = slint::Image::load_from_path(default_image_path).unwrap();

    let vec_model = songs.into_iter().map(|song| {
        types::to_track_detail_detail(app_lib.clone(), song, default_image.clone())
    }).collect::<VecModel<_>>();

    let model_rc = ModelRc::new(vec_model);

    let mut rows = Vec::with_capacity(model_rc.row_count());
    for i in 0..model_rc.row_count() {
        let t = model_rc.row_data(i).unwrap();
        rows.push((i, i32x2_to_u64(t.album_id_1, t.album_id_2), t.pic_url));
    }

    let model_rc_for_task = model_rc.clone();
    let _ = slint::spawn_local(async move {
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
        .buffer_unordered(smt::Config::SONGLIST_COVER_FETCH_CONCURRENCY); 

        let mut pending = Vec::with_capacity(smt::Config::SONGLIST_UI_UPDATE_BATCH);
        futures::pin_mut!(fetches);
        while let Some(result) = fetches.next().await {
            if let Some((row, image)) = result {
                pending.push((row, image));
                if pending.len() >= smt::Config::SONGLIST_UI_UPDATE_BATCH {
                    apply_cover_updates_batch(&model_rc_for_task, &mut pending);
                }
            }
        }

        if !pending.is_empty() {
            apply_cover_updates_batch(&model_rc_for_task, &mut pending);
        }
    });

    model_rc
}


fn on_clicked(i: i32, select_type: i32, model_rc: ModelRc<SongDetail>) {
    match select_type {
        // 单选
        0 => {
            model_rc
            .iter()
            .enumerate()
            .filter_map(|item| -> Option<usize>{
                if item.1.selected {
                    Some(item.0)
                } else {
                    None
                }
            })
            .for_each(|row| {
                let mut t = model_rc.row_data_tracked(row).unwrap();
                t.selected = false;
                model_rc.set_row_data(row, t);
            });

            let mut t = model_rc.row_data_tracked(i.try_into().unwrap()).unwrap();
            t.selected = true;
            model_rc.set_row_data(i.try_into().unwrap(), t);
        }
        
        // 加选
        1 => {
            let mut t = model_rc.row_data_tracked(i.try_into().unwrap()).unwrap();
            t.selected = true;
            model_rc.set_row_data(i.try_into().unwrap(), t);
        }
        // 区间选
        2 => {
            let Some(mut start) = model_rc.iter().position(|item| { item.selected }) else  {
                let mut t = model_rc.row_data_tracked(i.try_into().unwrap()).unwrap();
                t.selected = true;
                model_rc.set_row_data(i.try_into().unwrap(), t);
                return;
            };
            
            let mut end: usize = 0;
            for i in (0..model_rc.row_count()).rev() {
                if model_rc.row_data(i).unwrap().selected {
                    end = i;
                    break;
                }
            }

            let i:usize = i.try_into().unwrap();

            start = min(start, i);
            end  = max(end, i);

            for row in start..=end {
                let mut t = model_rc.row_data_tracked(row).unwrap();
                t.selected = true;
                model_rc.set_row_data(row, t);
            }
        }
        // 全选
        3 => {
            for row in 0..model_rc.row_count() {
                let mut t = model_rc.row_data_tracked(row).unwrap();
                t.selected = true;
                model_rc.set_row_data(row, t);
            }
        }
        _ => ()
    }
}

pub fn bind_songlist(app_weak: AppWeak, app_lib : AppLibRc) {
    let app = app_weak.upgrade().unwrap();
    app.global::<SongListProperty>().on_clicked(on_clicked);
    app.global::<SongListProperty>().on_play({
        let app_lib = app_lib.clone();
        move |id_1, id_2, model_rc | {
            let id = i32x2_to_u64(id_1, id_2);
            let playlist = model_rc.iter().map(|date| {
                i32x2_to_u64(date.id_1, date.id_2)
            }).collect();

            let mut player_core = app_lib.player_core.borrow_mut();
            player_core.replace_playlist(playlist);
            player_core.play(id);
        }
    });
}
