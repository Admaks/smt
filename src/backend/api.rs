use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{Context, bail};
// use crate::futures::RuntimeBridge;
// use crate::{model, runtime_bridge};
use ncm_api_rs::{create_client, ApiClient, Query};
use reqwest::header::{CACHE_CONTROL, COOKIE, HeaderValue, PRAGMA, REFERER, USER_AGENT};
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use super::model;

#[derive(Clone)]
#[allow(dead_code)]
pub struct NcmApi {
    client: Arc<RwLock<ApiClient>>,
    download_client: reqwest::Client,
    cookie: Arc<str>,
    // runtime_bridge: Arc<RuntimeBridge>,
}

#[derive(Copy, Clone)]
pub enum MusicQuality{
    Standard,
    Higher,
    ExHigh,
    Lossless,
    HiRes,
    JyEffect,
    Sky,
    Dolby,
    JyMaster,
}

impl MusicQuality {
    pub fn to_str(&self)->&'static str {
        match self {
            MusicQuality::Standard => "standard",
            MusicQuality::Higher => "higher",
            MusicQuality::ExHigh => "exhigh",
            MusicQuality::Lossless => "lossless",
            MusicQuality::HiRes => "hires",
            MusicQuality::JyEffect => "jyeffect",
            MusicQuality::Sky => "sky",
            MusicQuality::Dolby => "dolby",
            MusicQuality::JyMaster => "jymaster",
        }
    }

    pub fn to_string(&self)-> String {
        self.to_str().to_string()
    }
}

impl Into<String> for MusicQuality {
    fn into(self) -> String {
        self.to_str().to_string()
    }
}

impl Into<&'static str> for MusicQuality {
    fn into(self) -> &'static str {
        self.to_str()
    }
}

impl NcmApi {
    pub fn new(cookie: &str) -> NcmApi {
        let client = create_client(Some(cookie.to_string()));
        let download_client = reqwest::Client::builder()
            .build()
            .expect("failed to create reqwest download client");

        NcmApi {
            client: Arc::new(RwLock::new(client)),
            download_client,
            cookie: Arc::from(cookie),
            // runtime_bridge: Arc::new(RuntimeBridge::new().unwrap_or_else(|err| {
            //     panic!("failed to initialize runtime bridge for `NcmApi`: {err}")
            // })),
        }
    }

    fn comma_ids(ids: &[u64]) -> String {
        ids.iter().fold(String::new(), |mut acc, id| {
            if !acc.is_empty() {
                acc.push(',');
            }
            acc += &id.to_string();
            acc
        })
    }

    pub fn set_cookie(&mut self, cookie_str : &str) {
        self.client.blocking_write().set_cookie(cookie_str.to_string());
    }

    async fn download_bytes(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        let mut request = self
            .download_client
            .get(url)
            .header(USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36"))
            .header(REFERER, HeaderValue::from_static("https://music.163.com/"))
            .header(CACHE_CONTROL, HeaderValue::from_static("no-cache"))
            .header(PRAGMA, HeaderValue::from_static("no-cache"));

        if !self.cookie.is_empty() {
            request = request.header(COOKIE, self.cookie.as_ref());
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("failed to send download request: {}", url))?
            .error_for_status()
            .with_context(|| format!("download request returned error status: {}", url))?;

        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("failed to read download response body: {}", url))?;

        if bytes.is_empty() {
            bail!("downloaded empty file from {}", url);
        }

        Ok(bytes.to_vec())
    }

    pub async fn user_account(&self) -> anyhow::Result<model::Account> {
        let params = Query::new();
        let res = self.client.read().await.user_account(&params).await?;
        Ok(res.body.try_into()?)
    }

    pub async fn login_qrcode(&self) -> anyhow::Result<model::QrCode> {
        let mut params = Query::new();
        params.random_cn_ip = true;
        let response = self.client.read().await.login_qr_key(&params).await?;

        let unikey = response.body["unikey"].as_str().ok_or(anyhow::anyhow!("no unikey in response"))?;
        Ok(model::QrCode {
            unikey: unikey.to_string(),
            url: format!("https://music.163.com/login?codekey={}", unikey),
        })
    }

    pub async fn login_check_qrcode(&self, qr_code: model::QrCode) -> anyhow::Result<String> {
        let mut params = Query::new().param("key", &qr_code.unikey);
        params.random_cn_ip = true;
        let response = self.client.read().await.login_qr_check(&params).await?;
        let cookie = response.cookie.iter().fold(String::new(), |mut acc, cookie| {
            acc.push_str(cookie);
            acc
        });

        Ok(cookie)
    }


    pub async fn playlist_detail(&self, id: u64, s: Option<i32>) -> anyhow::Result<model::Playlist> {
        let mut params = Query::new().param("id", &id.to_string());
        if let Some(s) = s {
            params = params.param("s", &s.to_string())
        }

        let response = self.client.read().await.playlist_detail(&params).await?;
        let res: model::Playlist = response.body["playlist"].clone().try_into()?;

        Ok(res)
    }

    pub async fn songs_detail(&self, ids: &[u64]) -> anyhow::Result<Vec<model::TrackDetail>>{
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let ids = Self::comma_ids(&*ids);

        let response = self.client.read().await.song_detail(&Query::new().param("ids", &ids)).await?;

        response.body["songs"]
            .as_array()
            .ok_or(anyhow::anyhow!("songs not found"))?
            .into_iter()
            .map(|x| {x.clone().try_into()})
            .collect()
    }

    pub async fn like_list(&self, uid:u64) -> anyhow::Result<HashSet<u64>> {
        let response = self.client.read().await.likelist(&Query::new().param("uid", &uid.to_string())).await?;
        let ids = serde_json::from_value(response.body["ids"].clone())?;
        Ok(ids)
    }

    fn detect_image_extension(bytes: &[u8]) -> anyhow::Result<&'static str> {
        const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        if bytes.starts_with(&PNG_MAGIC) {
            return Ok("png");
        }

        if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
            return Ok("jpg");
        }

        Err(anyhow::anyhow!("unsupported image format, only png/jpg are allowed"))
    }

    #[allow(dead_code)]
    fn detect_audio_extension(bytes: &[u8]) -> anyhow::Result<&'static str> {
        if bytes.starts_with(b"fLaC") {
            return Ok("flac");
        }

        if bytes.starts_with(b"OggS") {
            return Ok("ogg");
        }

        if bytes.starts_with(b"ID3")
            || (bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0)
        {
            return Ok("mp3");
        }

        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
            return Ok("wav");
        }

        if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
            return Ok("m4a");
        }

        if bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] == 0xF1 || bytes[1] == 0xF9) {
            return Ok("aac");
        }

        Err(anyhow::anyhow!("unsupported audio format, only mp3/flac/wav/ogg/m4a/aac are allowed"))
    }

    pub async fn get_image(&self, unique_name:&str, url: &str, dir: PathBuf, width: u16, high: u16)
        -> anyhow::Result<PathBuf> {
        let png_path = dir.join(format!("{}.png", unique_name));
        if fs::metadata(&png_path).await.is_ok() {
            return Ok(png_path);
        }

        let jpg_path = dir.join(format!("{}.jpg", unique_name));
        if fs::metadata(&jpg_path).await.is_ok() {
            return Ok(jpg_path);
        }

        fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("failed to create image directory: {}", dir.display()))?;

        let bytes = self
            .download_bytes(&format!("{}?param={}y{}", url, width, high))
            .await?;
        let extension = Self::detect_image_extension(&bytes)?;
        let image_path = dir.join(format!("{}.{}", unique_name, extension));

        let mut file = File::create(&image_path)
            .await
            .with_context(|| format!("failed to create image file: {}", image_path.display()))?;
        file.write_all(&bytes)
            .await
            .with_context(|| format!("failed to write image file: {}", image_path.display()))?;

        Ok(image_path)
    }

    async fn songs_url(&self, ids: &[u64], br: MusicQuality) -> anyhow::Result<HashMap<u64, anyhow::Result<model::TrackUrl>>> {
        let ids = Self::comma_ids(ids);

        let response = self.client.read().await.song_url_v1(&Query::new()
            .param("id", &ids)
            .param("level", br.into())).await?;


        let data = response.body["data"]
            .as_array()
            .ok_or(anyhow::anyhow!("data not found in get_songs_url response"))?;

        Ok(data.iter().map(|track_value| {
            let track: anyhow::Result<model::TrackUrl> = track_value.clone().try_into();
            let id = track.as_ref().unwrap().id;
            (id, track)
        }).collect())
    }


    pub async fn songs_path(&self, ids: &[u64], br: MusicQuality, dir: PathBuf)
                            -> anyhow::Result<HashMap<u64, anyhow::Result<PathBuf>>> {
        const AUDIO_EXTENSIONS: [&str; 6] = ["mp3", "flac", "wav", "ogg", "m4a", "aac"];
        let br_tag = br.to_str();

        fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("failed to create songs directory: {}", dir.display()))?;

        let mut songs_url = self.songs_url(ids, br).await?;
        let mut local_files: HashMap<u64, anyhow::Result<PathBuf>> = HashMap::new();

        for id in ids {
            let mut existing_path = None;
            for ext in AUDIO_EXTENSIONS {
                let candidate = dir.join(format!("{}_{}.{}", id, br_tag, ext));
                if fs::metadata(&candidate).await.is_ok() {
                    existing_path = Some(candidate);
                    break;
                }
            }
            if let Some(path) = existing_path {
                local_files.insert(*id, Ok(path));
                continue;
            }

            let Some(track_url) = songs_url.remove(id) else {
                local_files.insert(*id, Err(anyhow::anyhow!("track url not found for id {}", id)));
                continue
            };


            let song_result = match track_url {
                Ok(track) => {
                    let result = async {
                        let url = track
                            .url
                            .as_ref()
                            .ok_or(anyhow::anyhow!("song url is empty for id {}", id))?;
                        let bytes = self.download_bytes(url).await?;
                        let extension = Self::detect_audio_extension(&bytes)?;
                        let song_path = dir.join(format!("{}_{}.{}", id, br_tag, extension));

                        let mut file = File::create(&song_path)
                            .await
                            .with_context(|| format!("failed to create song file: {}", song_path.display()))?;
                        file.write_all(&bytes)
                            .await
                            .with_context(|| format!("failed to write song file: {}", song_path.display()))?;

                        Ok(song_path)
                    }
                    .await;
                    result
                }
                Err(err) => Err(err),
            };

            local_files.insert(*id, song_result);
        }

        Ok(local_files)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use async_compat::CompatExt;
    static COOKIE_STR: &str =
        "MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/neapi/clientlog;NMTID=00OagHBmVl6aaGMgUVasO4InC-VHoEAAAGdm7Engw; Max-Age=315360000; Expires=Mon, 14 Apr 2036 13:46:11 GMT; Path=/;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/weapi/feedback;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/weapi/clientlog;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/api/clientlog;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/weapi/clientlog;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/openapi/clientlog;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/wapi/feedback;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/neapi/clientlog;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/wapi/feedback;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/wapi/clientlog;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/api/clientlog;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/eapi/feedback;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/weapi/feedback;MUSIC_R_U=004A5B8DBF3CF50BA23EBF1AA572AE137482BBA4CEAB7D4DF857210BBA405DACFC81EB66F24BBA99D598EF46D76C4691CD84AD42B13026353B54C7E3FB6B73A13DE2CE192E472F746472BA8BBFCFCC2CAE; Max-Age=15552000; Expires=Wed, 14 Oct 2026 13:46:11 GMT; Path=/api/login/token/refresh;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/api/feedback;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/eapi/feedback;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/neapi/feedback;MUSIC_SNS=; Max-Age=0; Expires=Fri, 17 Apr 2026 13:46:11 GMT; Path=/__csrf=09361ac2ee2594b1635df6f4e1f7b7fc; Max-Age=1296010; Expires=Sat, 02 May 2026 13:46:21 GMT; Path=/;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/wapi/clientlog;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/neapi/feedback;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/api/feedback;MUSIC_U=005DFC028EA62F9EA4075D12CFCE3E187CF9124F4840757933B28A4A794173DCC14FF7834C4893971EA75DEF3A957063CC3AD6D034F757B76AF51AF8F5B114AEAB4BC958EA6908B395A4A21D265AAB103C1BDEC9A81E6A613EDC6FB1142DBC5B21EBCAFC99564B2A46FDAD0F802EAF57FD0B7C5CE1A7D3812944F7AE0FBEF9B0C65C7AF3BE5CF73591CBF5E0660265160B7B40CE643A8D1F9BCA95496B1AFE30E6D68913ADD8391088F68C2913C155292556E32EC01988FCCCF49EC00E3916C5AAD7E39D895EC1E1DE96922A7B61BD50F094BC56838732252D6249A7FB3872CAEAA7C53245959BA41892F4A1DA76384B3BFE2A22119CB07158784F48A4DA19AFD3EBC2B1522DF3B511CA43BFB5BDC5E61303F7852101913803062A659E0339085B8B4593EA4C493AC4BF48B3CE5EDCC4FDA9DF79C65923A3DB17598A59F0B765D167DF2C23A6E66A447C882C6EFF3D0E66D0BD38D9A83722FA8E1CABED67D3D0697370ECC79FDD3F8883BCE8942BD780FB291825337FEA993E0A1FDBF219BB4B30099BEB52BEE9DE5F1C1648A04E549FAF5B93C110A1F431ACEE5E5BCF95302072; Max-Age=15552000; Expires=Wed, 14 Oct 2026 13:46:11 GMT; Path=/;MUSIC_R_U=004A5B8DBF3CF50BA23EBF1AA572AE137482BBA4CEAB7D4DF857210BBA405DACFC81EB66F24BBA99D598EF46D76C4691CD84AD42B13026353B54C7E3FB6B73A13DE2CE192E472F746472BA8BBFCFCC2CAE; Max-Age=15552000; Expires=Wed, 14 Oct 2026 13:46:11 GMT; Path=/eapi/login/token/refresh;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/openapi/clientlog;MUSIC_A_T=1775140562623; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/eapi/clientlog;MUSIC_R_T=1775140562754; Max-Age=2147483647; Expires=Wed, 05 May 2094 17:00:18 GMT; Path=/eapi/clientlog;";

    #[test]
    fn test_account() {
        futures::executor::block_on(async move {
            let api = NcmApi::new(COOKIE_STR);
            let res = api.user_account().compat().await.unwrap();
            println!("{:#?}", res);
        })
    }

    #[test]
    fn test_playlist() {
        futures::executor::block_on(async move {
            let api = NcmApi::new(COOKIE_STR);
            let res = api.playlist_detail(17607058970, None).compat().await.unwrap();

            println!("{:#?}", res);
        })
    }

    #[test]
    fn test_songs_detail() {
        futures::executor::block_on(async move {
            let api = NcmApi::new(COOKIE_STR);
            let res = api.songs_detail(&[740558, 26133345, 740611]).compat().await.unwrap();

            println!("{:#?}", res);
        })
    }

    #[test]
    fn test_like_list() {
        futures::executor::block_on(async move {
            let api = NcmApi::new(COOKIE_STR);
            let account = api.user_account().compat().await.unwrap();
            let like_ids = api.like_list(account.id).compat().await.unwrap();
            println!("{:#?}", like_ids);
        })
    }

    #[tokio::test]
    async fn test_songs_url() {
        let api = NcmApi::new(COOKIE_STR);
        let res = api.songs_url(&[740558, 26133345, 740611], MusicQuality::Higher).await.unwrap();
        println!("{:#?}", res);
    }

    #[test]
    fn test_songs_path() {
        futures::executor::block_on(async move {
            let api = NcmApi::new(COOKIE_STR);
            let res = api
                .songs_path(&[740558, 26133345, 740611],
                            MusicQuality::Standard,
                            PathBuf::from("D:\\code\\ncm-api-rust\\songs"))
                .compat().await.unwrap();

            println!("{:#?}", res);
        })
    }
}
