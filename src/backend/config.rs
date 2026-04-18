use std::path::PathBuf;

use serde::{Deserialize, Serialize};



#[derive(Serialize, Deserialize, Clone)]
pub struct Config{
    pub cookie_store_path: PathBuf,
    pub cache_dir: PathBuf,
    pub max_cons: usize,
    pub cache_capacity: u64,
    pub assets_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cookie_store_path: PathBuf::from("D:/code/smt/smt/cache/cookie"),
            cache_dir: PathBuf::from("D:/code/smt/smt/cache"),
            assets_dir: PathBuf::from("D:/code/smt/smt/asset"),
            max_cons: 5,
            cache_capacity: 1024 * 1024 * 1024 * 1
        }
    }
}


impl Config {

}
