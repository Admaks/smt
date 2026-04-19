use smt::model::TrackDetail;

use crate::{AppLibRc, SongDetail};

pub fn to_track_detail_detail(app_lib: AppLibRc ,track_detail: TrackDetail, image: slint::Image) -> SongDetail {
    let (album_id_1, album_id_2) = smt::u64_to_i32x2(track_detail.album.id);
    let loved = app_lib.loved_ids.borrow().contains(&track_detail.id);
    let (id_1, id_2) = smt::u64_to_i32x2(track_detail.id);
    let mut sec = track_detail.duration / 1000;
    let mut min = sec / 60;
    let hour = min / 60;
    min %= 60;
    sec %= 60;

    let to2 = |time| {
        if time < 10 {
            format!("0{}", time)
        } else {
            format!("{}", time)
        }
    };
        
    let duration = if hour == 0 {
        format!("{}:{}",to2(min), to2(sec))
    } else {
        format!("{}:{}:{}", to2(hour), to2(min), to2(sec))
    };
    let artist = if track_detail.artist.len() > 1 {
        track_detail.artist.into_iter().map(|a| a.name).collect::<Vec<_>>().join("/")
    } else if track_detail.artist.len() == 1 {
        track_detail.artist[0].name.clone()
    } else {
        String::from("未知歌手")
    };

    return SongDetail { 
        selected: false,
        album: slint::SharedString::from(track_detail.album.name),
        duration: slint::SharedString::from(duration),
        id_1,
        id_2,
        album_id_1,
        album_id_2,
        name: slint::SharedString::from(track_detail.name),
        pic_url: slint::SharedString::from(track_detail.album.pic_url),
        singer: slint::SharedString::from(artist),
        image: image,
        loved
    };
}



