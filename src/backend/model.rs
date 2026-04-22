use core::time;

use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct QrCode {
    pub unikey: String,
    pub url: String,
}
fn null_to_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct Account {
    pub id: u64,
    pub nickname: String,
    pub avatar_img_id: u64,
    pub avatar_url: String,
    pub background_img_id: u64,
    pub background_url: String,
    pub description: String,
    pub detail_description: String,
    pub remark_name: Option<String>,
    pub followed: bool,
    pub vip_type: i32,
}

impl TryFrom<Value> for Account {
    type Error = anyhow::Error;

    fn try_from(v: Value) -> Result<Self, Self::Error> {
        use anyhow::anyhow;
        let profile = v.get("profile")
            .ok_or(anyhow!("profile not found"))?;
        let account = v.get("account")
            .unwrap_or(&v);

        Ok(Account {
            id: profile
                .get("userId")
                .and_then(Value::as_u64)
                .or_else(|| account.get("id").and_then(Value::as_u64))
                .ok_or_else(|| anyhow!("account not found"))?,
            nickname: profile
                .get("nickname")
                .and_then(Value::as_str)
                .ok_or(anyhow!("nickname not found"))?
                .to_string(),
            avatar_img_id: profile
                .get("avatarImgId")
                .and_then(Value::as_u64)
                .ok_or(anyhow!("avatarImgId not found"))?,
            avatar_url: profile
                .get("avatarUrl")
                .and_then(Value::as_str)
                .ok_or(anyhow!("avatarUrl not found"))?
                .to_string(),
            background_img_id: profile
                .get("backgroundImgId")
                .and_then(Value::as_u64)
                .ok_or(anyhow!("backgroundImgId not found"))?,
            background_url: profile
                .get("backgroundUrl")
                .and_then(Value::as_str)
                .ok_or(anyhow!("backgroundUrl not found"))?
                .to_string(),
            description: profile
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            detail_description: profile
                .get("detailDescription")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            remark_name: profile
                .get("remarkName")
                .and_then(Value::as_str)
                .map(str::to_string),
            followed: profile
                .get("followed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            vip_type: profile
                .get("vipType")
                .and_then(Value::as_i64)
                .map(|x| x as i32)
                .unwrap_or(0),
        })
    }
}


#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDetail{
    pub id: u64,
    pub name: String,
    pub cover_img_id: u64,
    pub cover_img_url: String,
    // #[serde(default)]
    // pub description: String
    #[serde(default, deserialize_with = "null_to_empty_string")]
    pub description: String,
    #[serde(skip)]
    pub creator: Account,
    pub create_time: u64,
    pub play_count: u64,
    pub subscribed: bool,
    pub track_count: i32,
    #[serde(skip)]
    pub track_ids: Vec<u64>
}

impl TryFrom<Value> for PlaylistDetail {
    type Error = anyhow::Error;
    fn try_from(v:Value) -> Result<Self, Self::Error> {
        let track_ids: anyhow::Result<Vec<u64>> = v["trackIds"]
            .as_array()
            .ok_or(anyhow::anyhow!("trackIds not found"))?
            .iter()
            .map(|x| {
                x["id"]
                    .as_u64()
                    .ok_or(anyhow::anyhow!("trackId id not found"))
            })
            .collect();

        let creator = Account {
            id: v["creator"]["userId"]
                .as_u64()
                .ok_or(anyhow::anyhow!("creator userId not found"))?,
            nickname: v["creator"]["nickname"]
                .as_str()
                .ok_or(anyhow::anyhow!("nickname nickname not found"))?
                .to_string(),
            avatar_url: v["creator"]["avatarUrl"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("creator avatarUrl not found"))?
                .to_string(),
            avatar_img_id: v["creator"]["avatarImgId"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("creator avatarImgId not found"))?,
            background_img_id: v["creator"]["backgroundImgId"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("creator backgroundImgId not found"))?,
            background_url: v["creator"]["backgroundUrl"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("creator backgroundUrl not found"))?
                .to_string(),
            description: v["creator"]["description"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("creator description not found"))?
                .to_string(),
            detail_description: v["creator"]["detailDescription"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("creator detailDescription not found"))?
                .to_string(),
            remark_name: v["creator"]["remarkName"].as_str().map(str::to_string),
            followed: v["creator"]["followed"].as_bool().unwrap_or(false),
            vip_type: v["creator"]["vipType"].as_i64().map(|x| x as i32).unwrap_or(0),
        };

        let mut playlist_details:Self = serde_json::from_value(v)
            .with_context(|| format!("Failed to parse playlist details"))?;

        playlist_details.creator = creator;
        playlist_details.track_ids = track_ids?;

        Ok(playlist_details)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackDetail {
    #[serde(rename = "al")]
    pub album: AlbumInTrack,
    #[serde(rename = "ar")]
    pub artist: Vec<ArtistInTrack>,
    pub id: u64,
    pub name: String,
    #[serde(rename = "tns", default)]
    pub translation: Vec<String>,
    #[serde(rename = "dt")]
    pub duration: u32,
    pub fee: i32
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumInTrack {
    pub id: u64,
    pub name: String,
    pub pic: u64,
    pub pic_url: String,
    #[serde(rename = "tns")]
    pub translation: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistInTrack {
    pub id: u64,
    pub name: String,
    #[serde(rename = "tns")]
    pub translation: Vec<String>,
}

impl TryFrom<Value> for TrackDetail {
    type Error = anyhow::Error;
    fn try_from(v:Value) -> Result<Self, Self::Error> {
        Ok(serde_json::from_value(v)?)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackUrl {
    #[serde(rename = "br")]
    pub bitrate: Option<i32>,
    pub url: Option<String>,
    pub id: u64,
}

impl TryFrom<Value> for TrackUrl {
    type Error = anyhow::Error;
    fn try_from(v:Value) -> Result<Self, Self::Error> {
        Ok(serde_json::from_value(v)?)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistShortInfo {
    pub id: u64,
    pub name: String,
    pub cover_img_url: String,
    pub cover_img_id: u64,
    #[serde(rename = "userId")]
    pub creator_id: u64,
    pub subscribed: bool,
}

impl TryFrom<Value> for PlaylistShortInfo {
    type Error = anyhow::Error;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        Ok(serde_json::from_value(v)?)
    }
}

#[derive(Debug, Clone)]
pub struct UserPlaylists {
    pub created: Vec<PlaylistShortInfo>,
    pub subscribed: Vec<PlaylistShortInfo>,
}
