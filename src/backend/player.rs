use std::{
	cell::RefCell,
	collections::{HashMap, VecDeque},
	fs::{self, File},
	io::{BufReader, Write},
	path::{Path, PathBuf},
	rc::Rc,
	time,
};

use anyhow::{Context, Result};
use async_compat::CompatExt;
use rand::seq::SliceRandom;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use serde::{Deserialize, Serialize};

use crate::{api, Config, NcmApi};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayMode {
	Sequence,
	LoopOne,
	LoopAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShuffleState {
	#[default]
	Disabled,
	Enabled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TrackSource {
	Missing,
	Ready(PathBuf),
	Unplayable,
	Downloading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayStatus {
	Playing(time::Duration),
	Paused(time::Duration),
	Downloading,
	Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerStatusKind {
	Playing,
	Paused,
	Downloading,
	Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurrentSongStatus {
	None,
	Downloading,
	Position(time::Duration),
	Unplayable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayOrderState {
	pub play_mode: PlayMode,
	pub shuffle_state: ShuffleState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStateFrame {
	pub player_status: PlayerStatusKind,
	pub current_song_status: CurrentSongStatus,
	pub play_order: PlayOrderState,
}

#[derive(Clone, Debug)]
enum CurrentTrackDecision {
	NoSong,
	NeedsDownload(u64),
	Downloading,
	Unplayable,
	Ready(PathBuf),
}

#[derive(Clone, Debug)]
struct PlayerRuntimeConfig {
	insert_prefetch_count: usize,
	predict_prefetch_count: usize,
	history_capacity: usize,
	persist_every_ticks: u64,
	default_output_check_every_ticks: u64,
	player_state_path: PathBuf,
}

impl PlayerRuntimeConfig {
	fn from_config(config: &Config) -> Self {
		let gap_ms = config.player_event_loop_gap_ms.max(1);
		let interval_ms = Config::PLAYER_DEFAULT_OUTPUT_CHECK_INTERVAL_MS.max(1);
		let default_output_check_every_ticks = interval_ms.div_ceil(gap_ms).max(1);

		Self {
			insert_prefetch_count: config.insert_prefetch_count,
			predict_prefetch_count: config.predict_prefetch_count,
			history_capacity: config.history_capacity,
			persist_every_ticks: config.persist_every_ticks,
			default_output_check_every_ticks,
			player_state_path: config.player_state_path.clone(),
		}
	}
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum PersistedPlayStatus {
	Playing(u64),
	Paused(u64),
	Downloading,
	Stopped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PlayerPersistState {
	playlist: Vec<u64>,
	current_index: Option<usize>,
	mode: PlayMode,
	#[serde(default)]
	shuffle_state: ShuffleState,
	original_playlist: Option<Vec<u64>>,
	history: Vec<u64>,
	status: PersistedPlayStatus,
}

pub struct PlayerCore {
	_stream: OutputStream,
	stream_handle: OutputStreamHandle,
	sink: Sink,

	playlist: Vec<u64>,
	songs: Rc<RefCell<HashMap<u64, TrackSource>>>,
	mode: PlayMode,
	shuffle_state: ShuffleState,
	original_playlist: Option<Vec<u64>>,
	history: VecDeque<u64>,

	current_index: Option<usize>,
	ncm_api: NcmApi,
	quality: api::MusicQuality,
	runtime_config: PlayerRuntimeConfig,

	cache_dir: PathBuf,
	error_count: usize,
	last_status: PlayStatus,
	last_default_output_device_name: Option<String>,
	default_output_switch_pending: bool,
	tick_counter: u64,
}

impl PlayerCore {
	fn create_output_stream_and_sink() -> Result<(OutputStream, OutputStreamHandle, Sink, String)> {
		let device = rodio::cpal::default_host()
			.default_output_device()
			.context("Cannot find default audio output device")?;
		let device_name = device
			.name()
			.unwrap_or_else(|_| "<default-output-device>".to_string());

		let (_stream, stream_handle) = OutputStream::try_from_device(&device)
			.context("Cannot create audio output stream on default device")?;
		let sink = Sink::try_new(&stream_handle).context("Cannot create sink")?;

		Ok((_stream, stream_handle, sink, device_name))
	}

	fn current_default_output_device_name() -> Option<String> {
		let device = rodio::cpal::default_host().default_output_device()?;
		Some(
			device
				.name()
				.unwrap_or_else(|_| "<default-output-device>".to_string()),
		)
	}

	fn rebind_to_default_output(&mut self) -> Result<()> {
		let (new_stream, new_stream_handle, new_sink, device_name) =
			Self::create_output_stream_and_sink()?;

		let old_sink = std::mem::replace(&mut self.sink, new_sink);
		old_sink.stop();
		self._stream = new_stream;
		self.stream_handle = new_stream_handle;
		self.last_default_output_device_name = Some(device_name);
		Ok(())
	}

	fn start_song_on_current_sink(
		&mut self,
		path: &Path,
		seek_to: Option<time::Duration>,
		pause_after_start: bool,
	) -> Result<()> {
		let file = File::open(path).context("Cannot open audio file")?;
		let source = Decoder::new(BufReader::new(file)).context("Cannot decode audio file")?;
		self.sink.append(source);
		self.sink.play();
		if let Some(position) = seek_to {
			if let Err(e) = self.sink.try_seek(position) {
				eprintln!("Error seeking to {:?} after output rebind: {}", position, e);
			}
		}
		if pause_after_start {
			self.sink.pause();
		}
		Ok(())
	}

	fn maybe_pause_for_default_output_change_on_tick(&mut self) {
		let check_interval = self.runtime_config.default_output_check_every_ticks.max(1);
		let next_tick = self.tick_counter.saturating_add(1);
		if next_tick % check_interval != 0 {
			return;
		}

		let Some(current_default_name) = Self::current_default_output_device_name() else {
			return;
		};

		if self.last_default_output_device_name.as_deref() == Some(current_default_name.as_str()) {
			return;
		}

		self.default_output_switch_pending = true;
		if !self.sink.empty() && !self.sink.is_paused() {
			self.sink.pause();
			self.last_status = PlayStatus::Paused(self.playback_position());
		}
	}

	fn needs_default_output_rebind(&self) -> bool {
		if self.default_output_switch_pending {
			return true;
		}

		let Some(current_default_name) = Self::current_default_output_device_name() else {
			return false;
		};

		self.last_default_output_device_name.as_deref() != Some(current_default_name.as_str())
	}

	fn maybe_rebind_default_output_for_resume(&mut self) {
		if !self.needs_default_output_rebind() {
			return;
		}

		let resume_position = self.playback_position();
		let had_active_playback = self.sink.is_paused() || !self.sink.empty();
		let current_track_path = if had_active_playback {
			match self.inspect_current_track() {
				CurrentTrackDecision::Ready(path) => Some(path),
				_ => None,
			}
		} else {
			None
		};

		if let Err(e) = self.rebind_to_default_output() {
			eprintln!("Error rebinding to default output device: {}", e);
			return;
		}
		self.default_output_switch_pending = false;

		if let Some(path) = current_track_path {
			if let Err(e) = self.start_song_on_current_sink(&path, Some(resume_position), true) {
				eprintln!("Error restoring paused playback after output rebind: {}", e);
				self.last_status = PlayStatus::Stopped;
				return;
			}
			self.last_status = PlayStatus::Paused(self.playback_position());
		}
	}

	fn maybe_rebind_default_output_for_new_playback(&mut self) {
		if !self.needs_default_output_rebind() {
			return;
		}

		if let Err(e) = self.rebind_to_default_output() {
			eprintln!("Error rebinding to default output device: {}", e);
			return;
		}
		self.default_output_switch_pending = false;
	}

	pub fn new(ncm_api: NcmApi, config: &Config) -> Result<Self> {
		let (_stream, stream_handle, sink, default_device_name) =
			Self::create_output_stream_and_sink()?;
		let runtime_config = PlayerRuntimeConfig::from_config(config);
		let mut core = Self {
			_stream,
			stream_handle,
			sink,
			songs: Rc::new(RefCell::new(HashMap::new())),
			playlist: Vec::new(),
			mode: PlayMode::Sequence,
			shuffle_state: ShuffleState::Disabled,
			original_playlist: None,
			history: VecDeque::with_capacity(runtime_config.history_capacity.max(1)),
			current_index: None,
			ncm_api,
			quality: api::MusicQuality::Standard,
			runtime_config,
			cache_dir: config.cache_dir.clone(),
			error_count: 0,
			last_status: PlayStatus::Stopped,
			last_default_output_device_name: Some(default_device_name),
			default_output_switch_pending: false,
			tick_counter: 0,
		};
		core.load_persisted_state();
		Ok(core)
	}

	fn load_persisted_state(&mut self) {
		let Ok(raw) = fs::read_to_string(&self.runtime_config.player_state_path) else {
			return;
		};
		let Ok(state) = serde_json::from_str::<PlayerPersistState>(&raw) else {
			return;
		};

		self.playlist = state.playlist;
		self.current_index = state
			.current_index
			.filter(|idx| *idx < self.playlist.len());
		self.mode = state.mode;
		self.shuffle_state = state.shuffle_state;
		self.original_playlist = state.original_playlist;
		if self.shuffle_state == ShuffleState::Enabled && self.original_playlist.is_none() {
			self.original_playlist = Some(self.playlist.clone());
		}
		self.history.clear();
		for id in state.history.into_iter().take(self.runtime_config.history_capacity) {
			self.history.push_back(id);
		}
		self.songs.borrow_mut().clear();
		self.last_status = match state.status {
			PersistedPlayStatus::Playing(ms) => PlayStatus::Playing(time::Duration::from_millis(ms)),
			PersistedPlayStatus::Paused(ms) => PlayStatus::Paused(time::Duration::from_millis(ms)),
			PersistedPlayStatus::Downloading => PlayStatus::Stopped,
			PersistedPlayStatus::Stopped => PlayStatus::Stopped,
		};

		self.pause();
	}

	pub fn get_current_id(&self) -> Option<u64> {
		let index = self.current_index?;
		Some(*self.playlist.get(index)?)
	}

	fn download_songs(&self, song_id: &[u64]){
		let songs = self.songs.clone();
		let cache_dir = self.cache_dir.join(Config::MUSIC_CACHE_SUBDIR);
		let quality = self.quality;
		let ncm_api = self.ncm_api.clone();
		let song_id = song_id.to_vec();

		let _ = slint::spawn_local(async move {
			song_id.iter().for_each(|id| {
				songs.borrow_mut().insert(*id, TrackSource::Downloading);
			});

			let Ok(path) = ncm_api.songs_path(&song_id, quality, cache_dir)
			.compat().await
			else {
				return ;
			};
			path.into_iter().for_each(|(id, path)| {
				let track_source = match path {
					Ok(path) => TrackSource::Ready(path),
					Err(_) => TrackSource::Unplayable,
				};
				songs.borrow_mut().insert(id, track_source);
			});
		});
	}

	fn stop_current_playback(&self) {
		if !self.sink.empty() || self.sink.is_paused() {
			self.sink.stop();
		}
	}

	fn take_insert_prefetch_ids(&self, song_ids: &[u64]) -> Vec<u64> {
		song_ids
			.iter()
			.take(self.runtime_config.insert_prefetch_count)
			.copied()
			.collect()
	}

	fn predicted_download_ids(&self) -> Vec<u64> {
		let Some(current_idx) = self.current_index else {
			return Vec::new();
		};
		let lookahead = self.runtime_config.predict_prefetch_count;
		if lookahead == 0 || self.playlist.is_empty() {
			return Vec::new();
		}

		let start = current_idx.saturating_add(1);
		let mut ready_count = 0usize;
		for idx in start..self.playlist.len() {
			let Some(song_id) = self.playlist.get(idx).copied() else {
				continue;
			};
			if matches!(self.songs.borrow().get(&song_id), Some(TrackSource::Ready(_))) {
				ready_count += 1;
			}
		}

		if ready_count >= lookahead {
			return Vec::new();
		}

		let mut ids = Vec::new();
		for idx in start..self.playlist.len() {
			let Some(song_id) = self.playlist.get(idx).copied() else {
				continue;
			};
			if matches!(self.songs.borrow().get(&song_id), Some(TrackSource::Ready(_) | TrackSource::Downloading)) {
				continue;
			}
			if !ids.contains(&song_id) {
				ids.push(song_id);
			}
			if ids.len() >= lookahead {
				break;
			}
		}
		ids
	}

	fn push_history(&mut self, song_id: u64) {
		if self.history.back().copied() == Some(song_id) {
			return;
		}
		self.history.push_back(song_id);
		while self.history.len() > self.runtime_config.history_capacity.max(1) {
			self.history.pop_front();
		}
	}

	fn persisted_status(&self) -> PersistedPlayStatus {
		match self.status_snapshot() {
			PlayStatus::Playing(duration) => PersistedPlayStatus::Playing(duration.as_millis() as u64),
			PlayStatus::Paused(duration) => PersistedPlayStatus::Paused(duration.as_millis() as u64),
			PlayStatus::Downloading => PersistedPlayStatus::Downloading,
			PlayStatus::Stopped => PersistedPlayStatus::Stopped,
		}
	}

	fn persist_state(&self) {
		if self.runtime_config.persist_every_ticks == 0 {
			return;
		}
		let state = PlayerPersistState {
			playlist: self.playlist.clone(),
			current_index: self.current_index,
			mode: self.mode,
			shuffle_state: self.shuffle_state,
			original_playlist: self.original_playlist.clone(),
			history: self.history.iter().copied().collect(),
			status: self.persisted_status(),
		};


		let Some(parent) = self.runtime_config.player_state_path.parent() else {
			return;
		};
		if fs::create_dir_all(parent).is_err() {
			return;
		}
		let Ok(serialized) = serde_json::to_string(&state) else {
			return;
		};

		let target_path = &self.runtime_config.player_state_path;
		let tmp_name = match target_path.file_name().and_then(|name| name.to_str()) {
			Some(name) => format!("{}{}", name, Config::PLAYER_STATE_TMP_SUFFIX),
			None => return,
		};
		let tmp_path = parent.join(tmp_name);

		let Ok(mut tmp_file) = fs::File::create(&tmp_path) else {
			return;
		};
		if tmp_file.write_all(serialized.as_bytes()).is_err() {
			let _ = fs::remove_file(&tmp_path);
			return;
		}
		if tmp_file.sync_all().is_err() {
			let _ = fs::remove_file(&tmp_path);
			return;
		}
		drop(tmp_file);

		if fs::rename(&tmp_path, target_path).is_err() {
			let _ = fs::remove_file(target_path);
			let _ = fs::rename(&tmp_path, target_path);
		}
	}


	pub fn replace_playlist(&mut self, new_playlist: Vec<u64>) {
		self.handle_replace_playlist(new_playlist);
	}

	pub fn insert_songs(&mut self, song_ids: &[u64]) {
		self.handle_insert_songs(song_ids);
	}

	pub fn shuffle_playlist(&mut self) {
		self.handle_shuffle_playlist();
	}

	pub fn restore_playlist_order(&mut self) {
		self.handle_restore_playlist_order();
	}

	pub fn set_play_mode(&mut self, mode: PlayMode) {
		self.handle_set_play_mode(mode);
	}

	pub fn toggle_play_mode(&mut self) {
		let new_mode = match self.mode {
			PlayMode::Sequence => PlayMode::LoopAll,
			PlayMode::LoopAll => PlayMode::LoopOne,
			PlayMode::LoopOne => PlayMode::Sequence,
		};
		self.handle_set_play_mode(new_mode);
	}

	pub fn play_mode(&self) -> PlayMode {
		self.mode
	}

	pub fn next_song(&mut self) -> PlayStatus {
		self.handle_next();
		self.status_snapshot()
	}

	pub fn prev_song(&mut self) -> PlayStatus {
		self.handle_prev();
		self.status_snapshot()
	}

	pub fn play(&mut self, song_id: u64) -> PlayStatus {
		self.handle_play_song(song_id);
		self.status_snapshot()
	}

	pub fn seek_to_duration(&mut self, position: time::Duration) -> PlayStatus {
		self.handle_seek(position);
		self.status_snapshot()
	}

	pub fn toggle_pause_resume(&mut self) -> PlayStatus {
		self.handle_toggle_pause_resume();
		self.status_snapshot()
	}

	pub fn pause(&mut self) -> PlayStatus {
		self.handle_pause();
		self.status_snapshot()
	}

	pub fn resume(&mut self) -> PlayStatus {
		self.handle_resume();
		self.status_snapshot()
	}

	fn reshuffle_from_backup(&mut self, keep_current_song: bool) {
		if self.playlist.is_empty() {
			return;
		}

		let current_song = if keep_current_song {
			self.current_song_id()
		} else {
			None
		};

		if let Some(original) = self.original_playlist.clone() {
			self.playlist = original;
		}

		if self.playlist.len() >= 2 {
			let mut rng = rand::thread_rng();
			self.playlist.shuffle(&mut rng);
		}

		if let Some(song_id) = current_song {
			self.current_index = self.playlist.iter().position(|id| *id == song_id);
		}
	}

	fn restore_playlist_order_inner(&mut self) {
		if self.shuffle_state != ShuffleState::Enabled {
			return;
		}

		let current_song = self.current_song_id();
		if let Some(original) = self.original_playlist.clone() {
			self.playlist = original;
			self.current_index = current_song
				.and_then(|song_id| self.playlist.iter().position(|id| *id == song_id));
		}

		self.original_playlist = None;
		self.shuffle_state = ShuffleState::Disabled;
	}

	fn handle_replace_playlist(&mut self, new_playlist: Vec<u64>) {
		self.stop_current_playback();
		self.current_index = None;
		self.playlist = new_playlist;
		self.shuffle_state = ShuffleState::Disabled;
		self.original_playlist = None;
		self.error_count = 0;
		self.last_status = PlayStatus::Stopped;
	}

	fn handle_insert_songs(&mut self, song_ids: &[u64]) {
		if song_ids.is_empty() {
			return;
		}

		if self.playlist.is_empty() {
			self.playlist.extend(song_ids.iter().copied());
			self.current_index = Some(0);
		} else {
			let insert_pos = self
				.current_index
				.map(|idx| (idx + 1).min(self.playlist.len()))
				.unwrap_or(self.playlist.len());
			self.playlist.splice(insert_pos..insert_pos, song_ids.iter().copied());
			if self.current_index.is_none() {
				self.current_index = Some(0);
			}
		}

		let current_song = self.current_song_id();
		if let Some(original) = self.original_playlist.as_mut() {
			let insert_pos = current_song
				.and_then(|id| original.iter().position(|origin_id| *origin_id == id).map(|i| i + 1))
				.unwrap_or(original.len());
			for (offset, id) in song_ids.iter().enumerate() {
				original.insert((insert_pos + offset).min(original.len()), *id);
			}
		}

		let to_prefetch = self.take_insert_prefetch_ids(&song_ids);
		if !to_prefetch.is_empty() {
			self.handle_download(to_prefetch);
		}
	}

	fn handle_shuffle_playlist(&mut self) {
		if self.playlist.is_empty() {
			return;
		}

		if self.shuffle_state == ShuffleState::Disabled {
			self.original_playlist = Some(self.playlist.clone());
			self.shuffle_state = ShuffleState::Enabled;
		}

		self.reshuffle_from_backup(true);
	}

	fn handle_restore_playlist_order(&mut self) {
		self.restore_playlist_order_inner();
	}

	fn handle_set_play_mode(&mut self, mode: PlayMode) {
		self.mode = mode;
	}

	fn handle_next(&mut self) {
		self.current_index = self.next_index();
		self.handle_start_current();
	}

	fn handle_prev(&mut self) {
		self.current_index = self.prev_index();
		self.handle_start_current();
	}

	fn handle_play_song(&mut self, song_id: u64) {
		self.maybe_rebind_default_output_for_new_playback();

		let previous_song = self.current_song_id();
		if let Some(index) = self.playlist.iter().position(|id| *id == song_id) {
			self.current_index = Some(index);
		} else {
			let insert_pos = self
				.current_index
				.map(|idx| (idx + 1).min(self.playlist.len()))
				.unwrap_or(self.playlist.len());
			self.playlist.insert(insert_pos, song_id);
			self.current_index = Some(insert_pos);
			if let Some(original) = self.original_playlist.as_mut() {
				let original_insert = previous_song
					.and_then(|id| original.iter().position(|origin_id| *origin_id == id).map(|i| i + 1))
					.unwrap_or(original.len());
				original.insert(original_insert.min(original.len()), song_id);
			}
		}
		self.handle_start_current();
	}

	fn handle_seek(&mut self, position: time::Duration) {
		if self.playlist.is_empty() {
			self.current_index = None;
			self.last_status = PlayStatus::Stopped;
			return;
		}

		if self.current_index.is_none() {
			self.current_index = Some(0);
		}

		if self.sink.empty() {
			self.handle_start_current();
			self.handle_apply_seek(position);
		} else {
			self.handle_apply_seek(position);
		}
	}

	fn handle_apply_seek(&mut self, position: time::Duration) {
		if let Err(e) = self.sink.try_seek(position) {
			eprintln!("Error seeking to {:?}: {}", position, e);
		}
		self.last_status = if self.sink.is_paused() {
			PlayStatus::Paused(self.playback_position())
		} else {
			PlayStatus::Playing(self.playback_position())
		};
	}

	fn handle_toggle_pause_resume(&mut self) {
		if self.playlist.is_empty() {
			self.last_status = PlayStatus::Stopped;
			return;
		}
		if self.sink.is_paused() {
			self.maybe_rebind_default_output_for_resume();
			self.sink.play();
			self.last_status = PlayStatus::Playing(self.playback_position());
		} else if self.sink.empty() {
			self.maybe_rebind_default_output_for_new_playback();
			if self.current_index.is_none() {
				self.current_index = Some(0);
			}
			self.handle_start_current();
		} else {
			self.sink.pause();
			self.last_status = PlayStatus::Paused(self.playback_position());
		}
	}

	fn handle_pause(&mut self) {
		if self.playlist.is_empty() {
			self.last_status = PlayStatus::Stopped;
			return;
		}
		self.sink.pause();
		self.last_status = PlayStatus::Paused(self.playback_position());
	}

	fn handle_resume(&mut self) {
		if self.playlist.is_empty() {
			self.last_status = PlayStatus::Stopped;
			return;
		}

		self.maybe_rebind_default_output_for_resume();

		if self.sink.empty() {
			self.maybe_rebind_default_output_for_new_playback();
			if self.current_index.is_none() {
				self.current_index = Some(0);
			}
			self.handle_start_current();
		} else {
			self.sink.play();
			self.last_status = PlayStatus::Playing(self.playback_position());
		}
	}

	fn handle_tick(&mut self) {
		if self.playlist.is_empty() {
			self.current_index = None;
			self.last_status = PlayStatus::Stopped;
			return;
		}

		if self.current_index.is_none() {
			self.current_index = Some(0);
			self.handle_start_current();
			return;
		}

		if self.sink.is_paused() {
			self.last_status = PlayStatus::Paused(self.playback_position());
			return;
		}

		if self.sink.empty() {
			if !matches!(self.last_status, PlayStatus::Downloading)
				&& !matches!(self.inspect_current_track(), CurrentTrackDecision::Downloading)
				&& !matches!(self.inspect_current_track(), CurrentTrackDecision::NeedsDownload(_))
				&& !matches!(self.play_mode(), PlayMode::LoopOne) 
			{
				self.current_index = self.next_index();
			}
			self.handle_start_current();
			return;
		}

		self.last_status = PlayStatus::Playing(self.playback_position());
	}

	fn handle_start_current(&mut self) {
		match self.inspect_current_track() {
			CurrentTrackDecision::NoSong => {
				self.last_status = PlayStatus::Stopped;
			}
			CurrentTrackDecision::NeedsDownload(song_id) => {
				self.songs.borrow_mut().insert(song_id, TrackSource::Missing);
				self.stop_current_playback();
				self.last_status = PlayStatus::Downloading;
				let mut song_ids = vec![song_id];
				song_ids.extend(self.predicted_download_ids());
				song_ids.sort_unstable();
				song_ids.dedup();
				self.handle_download(song_ids);
			}
			CurrentTrackDecision::Downloading => {
				self.stop_current_playback();
				self.last_status = PlayStatus::Downloading;
			}
			CurrentTrackDecision::Unplayable => self.handle_advance_after_failure(),
			CurrentTrackDecision::Ready(path) => {
				let current_song = self.current_song_id();
				if let Err(e) = self.play_song(&path) {
					if let Some(song_id) = current_song {
						eprintln!("Error playing song {}: {}", song_id, e);
						self.songs.borrow_mut().insert(song_id, TrackSource::Unplayable);
					}
					self.handle_advance_after_failure();
					return;
				}
				self.error_count = 0;
				if let Some(song_id) = current_song {
					self.push_history(song_id);
				}
				self.last_status = PlayStatus::Playing(self.playback_position());
				let song_ids = self.predicted_download_ids();
				if !song_ids.is_empty() {
					self.handle_download(song_ids);
				}
			}
		}
	}

	fn handle_download(&mut self, song_ids: Vec<u64>) {
		if song_ids.is_empty() {
			return;
		}
		self.download_songs(&song_ids);
		self.last_status = PlayStatus::Downloading;
	}

	fn handle_advance_after_failure(&mut self) {
		self.error_count += 1;
		if self.error_count < self.playlist.len() {
			self.current_index = self.next_index();
			self.handle_start_current();
		} else {
			self.last_status = PlayStatus::Stopped;
		}
	}

	fn current_song_id(&self) -> Option<u64> {
		self.current_index.and_then(|idx| self.playlist.get(idx).copied())
	}

	fn inspect_current_track(&self) -> CurrentTrackDecision {
		let Some(song_id) = self.current_song_id() else {
			return CurrentTrackDecision::NoSong;
		};

		let track_source = self
			.songs
			.borrow()
			.get(&song_id)
			.cloned()
			.unwrap_or(TrackSource::Missing);

		match track_source {
			TrackSource::Ready(path) => CurrentTrackDecision::Ready(path),
			TrackSource::Downloading => CurrentTrackDecision::Downloading,
			TrackSource::Unplayable => CurrentTrackDecision::Unplayable,
			TrackSource::Missing => CurrentTrackDecision::NeedsDownload(song_id),
		}
	}

	fn next_index(&mut self) -> Option<usize> {
		match self.current_index {
			// Some(idx) if self.mode == PlayMode::LoopOne => Some(idx),
			Some(idx) if idx + 1 < self.playlist.len() => Some(idx + 1),
			Some(_) if self.shuffle_state == ShuffleState::Enabled && !self.playlist.is_empty() => {
				self.reshuffle_from_backup(false);
				Some(0)
			}
			Some(_) if self.mode == PlayMode::LoopAll => Some(0),
			Some(_) => None,
			None if !self.playlist.is_empty() => Some(0),
			None => None,
		}
	}

	fn prev_index(&self) -> Option<usize> {
		if self.playlist.is_empty() {
			return None;
		}

		match self.current_index {
			Some(idx) if idx > 0 => Some(idx - 1),
			Some(0) if self.mode == PlayMode::LoopAll => Some(self.playlist.len() - 1),
			Some(0) => Some(0),
			Some(_) => Some(0),
			None => Some(0),
		}
	}

	fn play_song(&mut self, path: &Path) -> Result<()> {
		self.stop_current_playback();
		self.start_song_on_current_sink(path, None, false)
	}

	fn playback_position(&self) -> time::Duration {
		self.sink.get_pos()
	}

	fn status_snapshot(&self) -> PlayStatus {
		if self.playlist.is_empty() {
			return PlayStatus::Stopped;
		}

		if self.sink.is_paused() {
			return PlayStatus::Paused(self.playback_position());
		}

		if !self.sink.empty() {
			return PlayStatus::Playing(self.playback_position());
		}

		self.last_status
	}

	fn player_status_kind_snapshot(&self) -> PlayerStatusKind {
		match self.status_snapshot() {
			PlayStatus::Playing(_) => PlayerStatusKind::Playing,
			PlayStatus::Paused(_) => PlayerStatusKind::Paused,
			PlayStatus::Downloading => PlayerStatusKind::Downloading,
			PlayStatus::Stopped => PlayerStatusKind::Stopped,
		}
	}

	fn current_song_status_snapshot(&self) -> CurrentSongStatus {
		match self.inspect_current_track() {
			CurrentTrackDecision::NoSong => CurrentSongStatus::None,
			CurrentTrackDecision::NeedsDownload(_) | CurrentTrackDecision::Downloading => {
				CurrentSongStatus::Downloading
			}
			CurrentTrackDecision::Unplayable => CurrentSongStatus::Unplayable,
			CurrentTrackDecision::Ready(_) => CurrentSongStatus::Position(self.playback_position()),
		}
	}

	fn state_frame_snapshot(&self) -> PlayerStateFrame {
		PlayerStateFrame {
			player_status: self.player_status_kind_snapshot(),
			current_song_status: self.current_song_status_snapshot(),
			play_order: PlayOrderState {
				play_mode: self.mode,
				shuffle_state: self.shuffle_state,
			},
		}
	}

	pub fn event_loop(&mut self) -> PlayerStateFrame {
		self.maybe_pause_for_default_output_change_on_tick();
		self.handle_tick();
		self.tick_counter = self.tick_counter.saturating_add(1);
		if self.runtime_config.persist_every_ticks > 0
			&& self.tick_counter % self.runtime_config.persist_every_ticks == 0
		{
			self.persist_state();
		}
		self.state_frame_snapshot()
	}
}

