use std::{cell::RefCell, collections::HashSet, path::{Path, PathBuf}};
use std::collections::HashMap;
use crate::{Config, player::PlayerCore};
use anyhow::{Context, Result};
use async_compat::CompatExt;

use image::{Rgba, RgbaImage};
use qrcode_generator::QrCodeEcc;
use crate::NcmApi;

pub struct AppLib {
    pub client: NcmApi,
    pub config: Config,
    pub login_user: RefCell<Option<crate::model::Account>>,
    pub loved_ids: RefCell<HashSet<u64>>,
    pub player_core: RefCell<PlayerCore>,
    track_detail_cache: moka::sync::Cache<u64, crate::model::TrackDetail>,
    playlist_cache: moka::sync::Cache<u64, crate::model::PlaylistDetail>
}


impl AppLib {
    pub async fn new() -> Self {
        let config = Config::default();

        let track_detail_cache =
            moka::sync::Cache::new(Config::TRACK_DETAIL_MEMERY_CACHE_CAPACITY);
        let playlist_cache =
            moka::sync::Cache::new(Config::PLAYLIST_MEMERY_CACHE_CAPACITY);
        
        // println!("Loading cookie from {:?}", config.cookie_store_path);
        let cookie_str = std::fs::read_to_string(&config.cookie_store_path)
            .unwrap_or_default();

        let client = NcmApi::new(&cookie_str);

        let Ok(user) = client
            .user_account()
            .compat().await
            else {
                  return Self {
                    player_core: RefCell::new(PlayerCore::new(client.clone(), &config).unwrap()),
                client,
                config,
                login_user: RefCell::new(None),
                loved_ids: RefCell::new(HashSet::new()),
                track_detail_cache,
                playlist_cache
            };
        };

        let loved_ids = client.like_list(user.id)
            .compat().await
            .unwrap_or_default();

        Self {
              player_core: RefCell::new(PlayerCore::new(client.clone(), &config).unwrap()),
            client,
            config,
            login_user: RefCell::new(Some(user)),
            loved_ids: RefCell::new(loved_ids),
            track_detail_cache,
            playlist_cache
        }
    }

    pub async fn init(&self, cookie_str :&str) -> Result<()> {
        
        let user = self.client
            .user_account()
            .compat().await
            .context("Cannot get user account")?;

        let loved_ids = self.client.like_list(user.id)
            .compat().await
            .context("Cannot get user liked songs")
            .unwrap_or_default()
            .into_iter()
            .collect();
        *self.loved_ids.borrow_mut() = loved_ids;
        *self.login_user.borrow_mut() = Some(user);
        self.save_cookie(cookie_str)?;
        Ok(())
    }

    pub fn save_cookie(&self, str: &str) -> Result<()> {
        std::fs::write(&self.config.cookie_store_path, str)
            .context("Cannot save cookie")
    }

    pub fn generate_qrcode_image(qr_str: &str, qr_size: u32, path: &Path, foreground: Rgba<u8>, background: Rgba<u8>) {
        let ecc = QrCodeEcc::Low;
        let matrix = qrcode_generator::to_matrix(qr_str, ecc).unwrap();
        let matrix_size = matrix.len();


        let mut img = RgbaImage::new(qr_size, qr_size);
        let module_size = qr_size / matrix_size as u32;

        for (y, row) in matrix.iter().enumerate() {
            for (x, &is_foreground) in row.iter().enumerate() {
                let color = if is_foreground { foreground } else { background };
                for dy in 0..module_size {
                    for dx in 0..module_size {
                        let px = x as u32 * module_size + dx;
                        let py = y as u32 * module_size + dy;
                        img.put_pixel(px, py, color);
                    }
                }
            }
        }

        img.save(path).unwrap();
    }

    pub async fn get_album_cover(&self, id: u64, url: &str, width: u16) -> anyhow::Result<PathBuf> {
        let filename = format!("Album_{}_{}", id, width);
        let cache_dir = self.config.cache_dir.join(Config::IMAGE_CACHE_SUBDIR);

        self.client.get_image(&filename, url, cache_dir, width, width).compat().await
    }

    pub async fn get_playlist_cover(&self, id: u64, url: &str, width: u16) -> anyhow::Result<PathBuf> {
        let filename = format!("Playlist_{}_{}", id, width);
        let cache_dir = self.config.cache_dir.join(Config::IMAGE_CACHE_SUBDIR);

        self.client.get_image(&filename, url, cache_dir, width, width).compat().await
    }

    pub async fn get_avatar(&self, id: u64, url: &str, width: u16) -> anyhow::Result<PathBuf> {
        let filename = format!("Avatar_{}_{}", id, width);
        let cache_dir = self.config.cache_dir.join(Config::IMAGE_CACHE_SUBDIR);

        self.client.get_image(&filename, url, cache_dir, width, width).compat().await
    }

    pub async fn get_tracks_cached(&self, ids : &[u64]) -> Vec<crate::model::TrackDetail>{
        let uncached= ids.iter().filter(|id| {
            !self.track_detail_cache.contains_key(*id)
        }).copied().collect::<Vec<_>>();
        let mut res = self
            .client
            .songs_detail(&uncached)
            .compat().await
            .unwrap_or(Vec::new())
            .into_iter()
            .map(|track| {
                (track.id, track)
            })
            .collect::<HashMap<u64, crate::model::TrackDetail>>();

        ids.iter().filter_map(|id| {
            match self.track_detail_cache.get(id) {
                Some(track) => Some(track),
                None => {
                    res.remove(id).and_then(|track| {
                        self.track_detail_cache.insert(*id, track.clone());
                        Some(track)
                    })
                }
            }
        }).collect()
    }

    pub async fn get_playlist_cached(&self, id: u64) -> Option<crate::model::PlaylistDetail> {
        if let Some(playlist) = self.playlist_cache.get(&id) {
            return Some(playlist);
        }

        let playlist = self.client.playlist_detail(id, None).compat().await.ok()?;

        self.playlist_cache.insert(id, playlist.clone());
        Some(playlist)
    }
}
