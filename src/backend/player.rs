use std::{
	cell::RefCell, collections::HashMap, fs::File, io::BufReader, path::{Path, PathBuf}, rc::Rc, time
};

use anyhow::{Context, Result};
use rand::seq::SliceRandom;
use rodio::cpal::traits::HostTrait;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

use crate::NcmApi;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayMode {
	Sequence,
	LoopOne,
	LoopAll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TrackSource {
	Missing,
	Ready(PathBuf),
	Unplayable,
	Downloading,
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlayResult {
	Started,
	Downloading,
	NoSong,
}

pub enum PlayStatus {
	Playing(time::Duration),
	Paused(time::Duration),
	Downloading,
	Stopped,
}

pub struct PlayerCore {
	_stream: OutputStream,
	stream_handle: OutputStreamHandle,
	sink: Sink,

	playlist: Vec<u64>,
	songs: Rc<RefCell<HashMap<u64, TrackSource>>>,
	mode: PlayMode,

	current_index: Option<usize>,
	ncm_api: NcmApi,
	quality: ncm_api_rust::api::MusicQuality,
	
	cache_dir: PathBuf,
	error_count: usize,
}

impl PlayerCore {
	fn create_output_stream_and_sink() -> Result<(OutputStream, OutputStreamHandle, Sink)> {
		let device = rodio::cpal::default_host()
			.default_output_device()
			.context("Cannot find default audio output device")?;

		let (_stream, stream_handle) = OutputStream::try_from_device(&device)
			.context("Cannot create audio output stream on default device")?;
		let sink = Sink::try_new(&stream_handle).context("Cannot create sink")?;

		Ok((_stream, stream_handle, sink))
	}

	fn rebind_to_default_output(&mut self) -> Result<()> {
		self.sink.stop();
		let (_stream, stream_handle, sink) = Self::create_output_stream_and_sink()?;
		self._stream = _stream;
		self.stream_handle = stream_handle;
		self.sink = sink;
		Ok(())
	}

	pub fn new(ncm_api: NcmApi, cache_dir: &Path) -> Result<Self> {
		let (_stream, stream_handle, sink) = Self::create_output_stream_and_sink()?;

		Ok(Self {
			_stream,
			stream_handle,
			sink,
			songs: Rc::new(RefCell::new(HashMap::new())),
			playlist: Vec::new(),	
			current_index: None,
			mode: PlayMode::Sequence,
			ncm_api,
			quality: ncm_api_rust::api::MusicQuality::Standard,
			cache_dir: cache_dir.to_path_buf(),
			error_count: 0,
		})
	}

	pub fn get_current_id(&self) -> Option<u64> {
		let index = self.current_index?;
		Some(self.playlist.get(index)?.clone())
	}

	fn download_songs(&self, song_id: &[u64]){
		let songs = self.songs.clone();
		let cache_dir = self.cache_dir.join("music");
		let quality = self.quality;
		let ncm_api = self.ncm_api.clone();
		let song_id = song_id.to_vec();

		let _ = slint::spawn_local(async move {
			song_id.iter().for_each(|id| {
				songs.borrow_mut().insert(*id, TrackSource::Downloading);
			});

			let Ok(path) = ncm_api.songs_path(&song_id, quality, cache_dir)
			.await
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


	pub fn replace_playlist(&mut self, new_playlist: Vec<u64>) {
		self.playlist = new_playlist;
		self.current_index = None;
		self.error_count = 0;
	}

	pub fn insert_songs(&mut self, song_ids: &[u64]) {
		if song_ids.is_empty() {
			return;
		}

		self.download_songs(song_ids);
		
		if self.playlist.is_empty() {
			self.playlist.extend_from_slice(song_ids);
			self.current_index = Some(0);
			return;
		}

		let insert_pos = self
			.current_index
			.map(|idx| (idx + 1).min(self.playlist.len()))
			.unwrap_or(self.playlist.len());

		self.playlist.splice(insert_pos..insert_pos, song_ids.iter().copied());

		if self.current_index.is_none() {
			self.current_index = Some(0);
		}
	}

	pub fn shuffle_playlist(&mut self) {
		if self.playlist.len() < 2 {
			return;
		}

		let current_song = self
			.current_index
			.and_then(|idx| self.playlist.get(idx).copied());
		let mut rng = rand::thread_rng();
		self.playlist.shuffle(&mut rng);

		if let Some(song_id) = current_song {
			self.current_index = self.playlist.iter().position(|id| *id == song_id);
		}
	}

	pub fn set_play_mode(&mut self, mode: PlayMode) {
		self.mode = mode;
	}

	pub fn play_mode(&self) -> PlayMode {
		self.mode
	}

	pub fn next_song(&mut self) -> PlayStatus {
		let result = self.play_next();
		self.map_play_result_to_status(result)
	}

	pub fn prev_song(&mut self) -> PlayStatus {
		if self.playlist.is_empty() {
			self.current_index = None;
			return PlayStatus::Stopped;
		}

		let prev_index = match self.current_index {
			Some(idx) if idx > 0 => Some(idx - 1),
			Some(0) if self.mode == PlayMode::LoopAll => Some(self.playlist.len() - 1),
			Some(0) => Some(0),
			Some(_) => Some(0),
			None => Some(0),
		};

		self.current_index = prev_index;
		let result = self.play_current();
		self.map_play_result_to_status(result)
	}

	pub fn play(&mut self, song_id: u64) -> PlayStatus {
		if let Some(index) = self.playlist.iter().position(|id| *id == song_id) {
			self.current_index = Some(index);
			let result = self.play_current();
			return self.map_play_result_to_status(result);
		}

		self.download_songs(&[song_id]);

		let insert_pos = self
			.current_index
			.map(|idx| (idx + 1).min(self.playlist.len()))
			.unwrap_or(self.playlist.len());
		self.playlist.insert(insert_pos, song_id);

		self.current_index = Some(insert_pos);
		let result = self.play_current();
		self.map_play_result_to_status(result)
	}

	pub fn seek_to_duration(&mut self, position: time::Duration) -> PlayStatus {
		if self.playlist.is_empty() {
			self.current_index = None;
			return PlayStatus::Stopped;
		}

		if self.current_index.is_none() {
			self.current_index = Some(0);
			let result = self.play_current();
			match result {
				PlayResult::Started => {}
				_ => return self.map_play_result_to_status(result),
			}
		} else if self.sink.empty() {
			let result = self.play_current();
			match result {
				PlayResult::Started => {}
				_ => return self.map_play_result_to_status(result),
			}
		}

		if let Err(e) = self.sink.try_seek(position) {
			eprintln!("Error seeking to {:?}: {}", position, e);
		}

		if self.sink.is_paused() {
			PlayStatus::Paused(self.playback_position())
		} else {
			PlayStatus::Playing(self.playback_position())
		}
	}

	pub fn toggle_pause_resume(&mut self) -> PlayStatus {
		if self.playlist.is_empty() {
			return PlayStatus::Stopped;
		}

		if self.sink.is_paused() {
			self.sink.play();
			return PlayStatus::Playing(self.playback_position());
		}

		if self.sink.empty() {
			if self.current_index.is_none() {
				self.current_index = Some(0);
			}
			let result = self.play_current();
			return self.map_play_result_to_status(result);
		}

		self.sink.pause();
		PlayStatus::Paused(self.playback_position())
	}

	pub fn pause(&mut self) -> PlayStatus {
		if self.playlist.is_empty() {
			return PlayStatus::Stopped;
		}

		self.sink.pause();
		PlayStatus::Paused(self.playback_position())
	}

	pub fn resume(&mut self) -> PlayStatus {
		if self.playlist.is_empty() {
			return PlayStatus::Stopped;
		}

		if self.sink.empty() {
			if self.current_index.is_none() {
				self.current_index = Some(0);
			}
			let result = self.play_current();
			return self.map_play_result_to_status(result);
		}

		self.sink.play();
		PlayStatus::Playing(self.playback_position())
	}


	fn play_next(&mut self) -> PlayResult {
		let next_index = match self.current_index {
			Some(idx) if self.mode == PlayMode::LoopOne => Some(idx),
			Some(idx) if idx + 1 < self.playlist.len() => Some(idx + 1),
			Some(_) if self.mode == PlayMode::LoopAll => Some(0),
			Some(_) => None,
			None if !self.playlist.is_empty() => Some(0),
			None => None,
		};
		self.current_index = next_index;
		self.play_current()
	}

	fn play_error(&mut self) -> PlayResult {
		self.error_count += 1;
		
		if self.error_count < self.playlist.len() {
			self.play_next()
		} else {
			PlayResult::NoSong
		}
	}

	fn play_current(&mut self) -> PlayResult {
		let Some(&song_id) = self.current_index
			.and_then(|idx| self.playlist.get(idx)) else {			
			return PlayResult::NoSong;
		};

		let track_source = self
			.songs
			.borrow()
			.get(&song_id)
			.cloned()
			.unwrap_or(TrackSource::Missing);

		match track_source {
			TrackSource::Ready(path) => {
				if let Err(e) = self.play_song(&path) {
					eprintln!("Error playing song {}: {}", song_id, e);
					self.songs.borrow_mut().insert(song_id, TrackSource::Unplayable);
					return self.play_next();
				}
				self.error_count = 0;
				PlayResult::Started
			}
			TrackSource::Downloading => PlayResult::Downloading,
			TrackSource::Unplayable => self.play_error(),
			TrackSource::Missing => {
				self.songs.borrow_mut().insert(song_id, TrackSource::Missing);
				self.download_songs(&[song_id]);
				PlayResult::Downloading
			}
		}
	}

	fn play_song(&mut self, path: &Path) -> Result<()> {
		self.rebind_to_default_output()?;

		let file = File::open(path).context("Cannot open audio file")?;
		let source = Decoder::new(BufReader::new(file)).context("Cannot decode audio file")?;
		self.sink.append(source);
		self.sink.play();
		Ok(())
	}

	fn playback_position(&self) -> time::Duration {
		self.sink.get_pos()
	}

	fn map_play_result_to_status(&self, result: PlayResult) -> PlayStatus {
		match result {
			PlayResult::Started => PlayStatus::Playing(self.playback_position()),
			PlayResult::Downloading => PlayStatus::Downloading,
			PlayResult::NoSong => PlayStatus::Stopped,
		}
	}

	pub fn event_loop(&mut self) -> PlayStatus {
		if self.playlist.is_empty() {
			self.current_index = None;
			return PlayStatus::Stopped;
		}

		if self.current_index.is_none() {
			self.current_index = Some(0);
			let result = self.play_current();
			return self.map_play_result_to_status(result);
		}

		if self.sink.is_paused() {
			return PlayStatus::Paused(self.playback_position());
		}

		if self.sink.empty() {
			let result = self.play_next();
			return self.map_play_result_to_status(result);
		}

		PlayStatus::Playing(self.playback_position())
	}
}

