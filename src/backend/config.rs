use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub cookie_store_path: PathBuf,
    pub cache_dir: PathBuf,
    pub max_cons: usize,
    pub cache_capacity: u64,
    pub assets_dir: PathBuf,
    pub insert_prefetch_count: usize,
    pub predict_prefetch_count: usize,
    pub history_capacity: usize,
    pub persist_every_ticks: u64,
    pub player_state_path: PathBuf,
    pub player_event_loop_gap_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cookie_store_path: PathBuf::from("D:/code/smt/smt/cache/cookie"),
            cache_dir: PathBuf::from("D:/code/smt/smt/cache"),
            assets_dir: PathBuf::from("D:/code/smt/smt/asset"),
            max_cons: 3,
            cache_capacity: 1024 * 1024 * 1024 * 1,
            insert_prefetch_count: 3,
            predict_prefetch_count: 2,
            history_capacity: 100,
            persist_every_ticks: 60,
            player_state_path: PathBuf::from("D:/code/smt/smt/cache/player_state.json"),
            player_event_loop_gap_ms: 50,
        }
    }
}

impl Config {
    // Low-level tuning knobs: compile-time constants, not user-facing config fields.
    pub const PLAYER_QUEUE_BUDGET: usize = 256;
    pub const MUSIC_CACHE_SUBDIR: &'static str = "music";
    pub const IMAGE_CACHE_SUBDIR: &'static str = "image";

    pub const PLAYER_STATE_TMP_SUFFIX: &'static str = ".tmp";

    pub const IMAGE_DOWNLOAD_CONCURRENCY: usize = 8;
    pub const AUDIO_DOWNLOAD_CONCURRENCY: usize = 5;
    pub const DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 8;
    pub const DOWNLOAD_TIMEOUT_SECS: u64 = 20;
    pub const DOWNLOAD_MAX_RETRIES: usize = 2;
    pub const DOWNLOAD_RETRY_BACKOFF_MS: u64 = 150;

    // Poll default output device at a low frequency to avoid per-tick overhead.
    pub const PLAYER_DEFAULT_OUTPUT_CHECK_INTERVAL_MS: u64 = 1000;

    pub const COVER_FETCH_CONCURRENCY: usize = 4;
    pub const COVER_UPDATE_BATCH: usize = 8;

    pub const TRACK_DETAIL_MEMERY_CACHE_CAPACITY: u64 = 1024 * 1024 * 1;
    pub const PLAYLIST_MEMERY_CACHE_CAPACITY: u64 = 1024 * 1024 * 1;

    pub const NAVIGATOR_HISTORY_MAX: usize = 100;

    pub const MUSIC_BUFFER_SIZE: usize = 1024 * 1024 * 5;
}
