use std::{
	cell::RefCell,
	collections::{HashMap, VecDeque},
	fs::{self, File},
	io::{BufReader, Write},
	path::{Path, PathBuf},
	rc::Rc,
	sync::{
		atomic::{AtomicU64, Ordering},
		Arc,
	},
	time,
};

use anyhow::{Context, Result};
use async_compat::CompatExt;
use rand::seq::SliceRandom;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sample, Sink, Source};
use serde::{Deserialize, Serialize};

use crate::{api, Config, NcmApi};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaylistBase {
	Album(u64),
	Artist(u64),
	Playlist(u64),
	None,
}

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

/// 当前歌曲在本地缓存层面的可用状态。
///
/// 这里描述的是“这首歌能不能立刻交给 rodio 播放”，而不是播放器整体状态：
/// - `Missing`：尚未发起下载，或当前运行时还不知道本地文件路径；
/// - `Downloading`：下载任务已发起，正在等待异步结果；
/// - `Ready(PathBuf)`：已经拿到本地可播放文件；
/// - `Unplayable`：下载或解码前置步骤失败，本轮播放流程应跳过它。
#[derive(Clone, Debug, PartialEq, Eq)]
enum TrackSource {
	Missing,
	Ready(PathBuf),
	Unplayable,
	Downloading,
}

/// 播放器对外暴露的完整运行状态。
///
/// 设计约束：
/// - `Playing` / `Paused` 自带当前播放位置，避免再维护一份平行的 position 字段；
/// - `Downloading` 表示当前曲目尚未可播放，正在等待下载或下载结果；
/// - `Stopped` 表示没有可继续的播放流程，例如空列表、全部失败、显式停播等。
///
/// 这样 UI 和业务层只需要匹配一个枚举，就能同时拿到“状态种类 + 必要数据”。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayStatus {
	Playing(time::Duration),
	Paused(time::Duration),
	Downloading,
	Stopped,
}

/// 播放顺序相关的独立状态。
///
/// 这部分与播放进行到哪一秒无关，只描述“列表如何走下一首”。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayOrderState {
	pub play_mode: PlayMode,
	pub shuffle_state: ShuffleState,
}

/// 每一帧事件循环输出给 UI 的状态快照。
///
/// 约定：
/// - `play_status` 是播放层的唯一真值来源；
/// - `play_order` 是与播放位置正交的列表控制状态；
/// - 不再单独暴露播放位置，因为它已经包含在 `PlayStatus` 中。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStateFrame {
	pub play_status: PlayStatus,
	pub play_order: PlayOrderState,
}

/// 给 rodio 的音频源套一层结束信号。
///
/// `Sink::empty()` 只能说明当前队列里没有可消费样本，不能严格证明“当前曲目已经自然播放结束”。
/// 为了避免刚 append 后的短暂空窗被误判为播完，这里在底层 `Source` 真正迭代到 `None` 时写入完成 epoch，
/// 再由 `tick` 消费这个事件来决定是否自动切下一首。
struct EndSignalSource<S> {
	inner: S,
	finished_epoch: Arc<AtomicU64>,
	epoch: u64,
	signaled: bool,
}

impl<S> EndSignalSource<S> {
	/// 绑定底层源与完成信号槽，构造一次播放实例对应的结束监听器。
	fn new(inner: S, finished_epoch: Arc<AtomicU64>, epoch: u64) -> Self {
		Self {
			inner,
			finished_epoch,
			epoch,
			signaled: false,
		}
	}
}

impl<S> Iterator for EndSignalSource<S>
where
	S: Source,
	S::Item: Sample,
{
	type Item = S::Item;

	/// 透传底层样本；当底层迭代自然结束时只上报一次 finished epoch。
	fn next(&mut self) -> Option<Self::Item> {
		let item = self.inner.next();
		if item.is_none() && !self.signaled {
			self.signaled = true;
			self.finished_epoch.store(self.epoch, Ordering::Release);
		}
		item
	}
}

impl<S> Source for EndSignalSource<S>
where
	S: Source,
	S::Item: Sample,
{
	/// 直接透传底层源的当前帧长度信息。
	fn current_frame_len(&self) -> Option<usize> {
		self.inner.current_frame_len()
	}

	/// 直接透传底层源的声道数。
	fn channels(&self) -> u16 {
		self.inner.channels()
	}

	/// 直接透传底层源的采样率。
	fn sample_rate(&self) -> u32 {
		self.inner.sample_rate()
	}

	/// 直接透传底层源的总时长。
	fn total_duration(&self) -> Option<time::Duration> {
		self.inner.total_duration()
	}

	/// 直接透传 seek 请求，不改变结束信号语义。
	fn try_seek(&mut self, pos: time::Duration) -> Result<(), rodio::source::SeekError> {
		self.inner.try_seek(pos)
	}
}

/// 由配置文件派生出的播放器运行时参数。
///
/// 该结构的目标是把事件循环真正关心的参数收敛成一组稳定值，避免在热路径里反复读取
/// 原始配置对象，也便于统一写下各种“至少为 1”之类的约束。
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
	/// 从外部配置提炼事件循环热路径所需参数，并应用最小值约束。
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

/// 写入磁盘时使用的播放状态。
///
/// 持久化格式只记录最小必要信息：
/// - `Playing/Paused` 保存毫秒级位置；
/// - `Downloading/Stopped` 不带额外载荷。
///
/// 注意：恢复时不会直接回到真正的“正在播放”，因为程序重启后 sink 里没有活动音频流，
/// 因此 `Playing(ms)` 会被降级成“停在该位置的已选中曲目”。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum PersistedPlayStatus {
	Playing(u64),
	Paused(u64),
	Downloading,
	Stopped,
}

/// 播放器写入本地文件的完整快照。
///
/// 目标是恢复“播放上下文”，而不是完整复活运行中的音频设备状态：
/// - 播放列表、当前索引、播放模式与乱序备份会被恢复；
/// - 当前曲目的缓存状态不会被恢复，启动后由运行时重新探测；
/// - 播放中的状态会被规范化成“已选中/可恢复”，由用户后续显式继续播放。
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

/// 播放器核心状态机。
///
/// 可以把它看成三层状态的组合：
/// 1. `playlist/current_index/mode/shuffle_state`：描述列表如何移动；
/// 2. `songs`：描述每首歌是否可立即播放；
/// 3. `sink + last_status + *_epoch`：描述当前音频输出与事件循环状态。
///
/// 设计原则：
/// - 能从 `sink` 推断的“正在播/暂停在几秒”尽量动态推断；
/// - 无法从 `sink` 推断的离散状态（如等待下载）才缓存到 `last_status`；
/// - 自然播放完成依赖 `EndSignalSource` 回调置位，而不是依赖 `sink.empty()` 猜测。
pub struct PlayerCore {
	_stream: OutputStream,
	stream_handle: OutputStreamHandle,
	sink: Sink,

	playlist: Vec<u64>,
	playlist_base: PlaylistBase,

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
	/// 当前曲目的离散状态缓存。
	///
	/// 这个字段只记录无法从 `sink` 直接推断出的状态，主要是：
	/// - `Downloading`：当前曲目尚不可播，事件循环应继续等待下载完成；
	/// - `Stopped`：当前没有可继续的播放流程；
	/// - `Playing` / `Paused`：作为最近一次显式状态迁移的回退值。
	///
	/// 重要约束：后台预取下载绝不能把这里覆盖成 `Downloading`，否则会让 tick 误以为
	/// “当前曲目仍在等待下载”，进而错误重启已经开始播放的歌曲。
	last_status: PlayStatus,
	last_default_output_device_name: Option<String>,
	default_output_switch_pending: bool,
	tick_counter: u64,
	finished_playback_epoch: Arc<AtomicU64>,
	active_playback_epoch: u64,
	handled_finished_epoch: u64,
	/// 延迟 seek 位置：当当前曲目尚未加载到 sink（例如先下载后播放）时先缓存，
	/// 待 `Ready` 后真正开始播放时再应用。
	pending_seek_after_start: Option<time::Duration>,
}

impl PlayerCore {
	/// 创建并绑定到当前默认输出设备。
	///
	/// 返回值除了 rodio 需要的 stream / handle / sink 外，还会返回设备名，供后续轮询检测
	/// “默认输出设备是否发生切换”。
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

	/// 查询当前系统默认输出设备名。
	///
	/// 这里返回 `Option<String>`，是因为在某些极端场景下系统可能暂时没有默认输出设备。
	fn current_default_output_device_name() -> Option<String> {
		let device = rodio::cpal::default_host().default_output_device()?;
		Some(
			device
				.name()
				.unwrap_or_else(|_| "<default-output-device>".to_string()),
		)
	}

	/// 重新绑定到新的默认输出设备。
	///
	/// 这是“设备切换恢复”的底层动作：创建新 sink，停止旧 sink，并刷新缓存的默认设备名。
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

	/// 在当前 sink 上开始播放一个本地文件。
	///
	/// 这个函数承担两件事：
	/// 1. 把解码后的 `Source` 包上一层 `EndSignalSource`，用于自然结束回调；
	/// 2. 根据需要在启动后 seek，并可选择马上进入暂停态（用于恢复暂停中的歌曲）。
	fn start_song_on_current_sink(
		&mut self,
		path: &Path,
		seek_to: Option<time::Duration>,
		pause_after_start: bool,
	) -> Result<()> {
		let file = File::open(path).context("Cannot open audio file")?;
		let source = Decoder::new(BufReader::new(file)).context("Cannot decode audio file")?;
		self.active_playback_epoch = self.active_playback_epoch.wrapping_add(1);
		if self.active_playback_epoch == 0 {
			self.active_playback_epoch = 1;
		}
		let signaled = EndSignalSource::new(
			source,
			Arc::clone(&self.finished_playback_epoch),
			self.active_playback_epoch,
		);
		self.sink.append(signaled);
		self.sink.play();
		if let Some(position) = seek_to
			&& let Err(e) = self.sink.try_seek(position)
		{
			eprintln!("Error seeking to {:?} after output rebind: {}", position, e);
		}
		if pause_after_start {
			self.sink.pause();
		}
		Ok(())
	}

	/// 在事件循环里周期性检测默认输出设备是否发生切换。
	///
	/// 一旦发现系统默认设备变了，会先打上 `default_output_switch_pending` 标记；若此时正在播，
	/// 还会先把 sink 暂停下来，等待用户下一次继续播放时完成 rebind。
	fn maybe_pause_for_default_output_change_on_tick(&mut self) {
		let check_interval = self.runtime_config.default_output_check_every_ticks.max(1);
		let next_tick = self.tick_counter.saturating_add(1);
		if !next_tick.is_multiple_of(check_interval) {
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

	/// 判断是否需要重新绑定到新的默认输出设备。
	fn needs_default_output_rebind(&self) -> bool {
		if self.default_output_switch_pending {
			return true;
		}

		let Some(current_default_name) = Self::current_default_output_device_name() else {
			return false;
		};

		self.last_default_output_device_name.as_deref() != Some(current_default_name.as_str())
	}

	/// 为“恢复播放/继续播放”场景处理默认输出设备切换。
	///
	/// 如果当前曲目之前处于暂停中且能定位到本地文件，就会在新设备上恢复该曲目，并保持暂停态，
	/// 这样调用方再执行 `sink.play()` 时就能从原位置继续。
	fn maybe_rebind_default_output_for_resume(&mut self) {
		if !self.needs_default_output_rebind() {
			return;
		}

		let resume_position = self.playback_position();
		let had_active_playback = self.sink.is_paused() || !self.sink.empty();
		let current_track_path = if had_active_playback {
			match self.current_track_source() {
				Some((_, TrackSource::Ready(path))) => Some(path),
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

	/// 为“启动新播放”场景处理默认输出设备切换。
	///
	/// 与 resume 不同，这里不尝试恢复旧曲目，只确保后续新播放会走到新的默认设备上。
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

	/// 构造播放器并尽量恢复上一次持久化的播放上下文。
	pub fn new(ncm_api: NcmApi, config: &Config) -> Result<Self> {
		let (_stream, stream_handle, sink, default_device_name) =
			Self::create_output_stream_and_sink()?;
		let runtime_config = PlayerRuntimeConfig::from_config(config);
		let finished_playback_epoch = Arc::new(AtomicU64::new(0));
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
			finished_playback_epoch,
			active_playback_epoch: 0,
			handled_finished_epoch: 0,
			pending_seek_after_start: None,
			playlist_base: PlaylistBase::None,
		};
		core.load_persisted_state();
		Ok(core)
	}

	/// 从磁盘恢复播放上下文。
	///
	/// 恢复策略是“恢复选择与位置，但不偷偷自动播”：
	/// - `Paused(ms)` 保持为暂停位置；
	/// - `Playing(ms)` 也会降级成暂停位置，因为新进程里并没有活动 sink；
	/// - `Downloading` 不恢复成运行中的下载状态，而是退回 `Stopped`，等待后续显式播放触发。
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
			PersistedPlayStatus::Playing(ms) => {
				let position = time::Duration::from_millis(ms);
				self.pending_seek_after_start = Some(position);
				PlayStatus::Paused(position)
			}
			PersistedPlayStatus::Paused(ms) => {
				let position = time::Duration::from_millis(ms);
				self.pending_seek_after_start = Some(position);
				PlayStatus::Paused(position)
			}
			PersistedPlayStatus::Downloading => PlayStatus::Stopped,
			PersistedPlayStatus::Stopped => PlayStatus::Stopped,
		};

		if self.current_index.is_none() {
			self.last_status = PlayStatus::Stopped;
			self.pending_seek_after_start = None;
		}

		self.stop_current_playback();
	}

	/// 返回当前播放列表指针指向的歌曲 id。
	pub fn get_current_id(&self) -> Option<u64> {
		let index = self.current_index?;
		Some(*self.playlist.get(index)?)
	}

	/// 异步下载一批歌曲，并把结果回写到 `songs` 表中。
	///
	/// 这里只更新“文件可用性”状态，不直接驱动播放状态机；真正是否继续播放、是否切到下一首，
	/// 由事件循环在后续 tick 中根据当前曲目状态决定。
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

	/// 停止当前 sink 中的音频。
	///
	/// 这是一个“只碰音频输出，不改业务状态”的底层动作；调用方负责自己维护 `last_status`。
	fn stop_current_playback(&self) {
		if !self.sink.empty() || self.sink.is_paused() {
			self.sink.stop();
		}
	}

	/// 新插入歌曲时，只对前若干首做立即预取，避免一次性把全部待播曲目都拉起下载。
	fn take_insert_prefetch_ids(&self, song_ids: &[u64]) -> Vec<u64> {
		song_ids
			.iter()
			.take(self.runtime_config.insert_prefetch_count)
			.copied()
			.collect()
	}

	/// 预测接下来可能需要的歌曲下载列表。
	///
	/// 规则：
	/// - 从当前曲目之后开始向后看；
	/// - 已经 `Ready`/`Downloading` 的歌曲不会重复加入；
	/// - 最多补齐 `predict_prefetch_count` 首潜在候选。
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

	/// 把当前成功开始播放的歌曲写入历史队列，用于后续潜在的“最近播放”语义。
	fn push_history(&mut self, song_id: u64) {
		if self.history.back().copied() == Some(song_id) {
			return;
		}
		self.history.push_back(song_id);
		while self.history.len() > self.runtime_config.history_capacity.max(1) {
			self.history.pop_front();
		}
	}

	/// 把当前运行时状态投影为可持久化的状态。
	fn persisted_status(&self) -> PersistedPlayStatus {
		match self.status_snapshot() {
			PlayStatus::Playing(duration) => PersistedPlayStatus::Playing(duration.as_millis() as u64),
			PlayStatus::Paused(duration) => PersistedPlayStatus::Paused(duration.as_millis() as u64),
			PlayStatus::Downloading => PersistedPlayStatus::Downloading,
			PlayStatus::Stopped => PersistedPlayStatus::Stopped,
		}
	}

	/// 按配置周期把播放上下文安全地写入磁盘。
	///
	/// 采用“先写临时文件再 rename”的方式，尽量避免中途崩溃导致状态文件损坏。
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


	/// 返回当前播放列表的语义来源（歌单/专辑/歌手等）。
	pub fn get_playlist_base(&self) -> &PlaylistBase {
		&self.playlist_base
	}

	/// 用新列表整体替换当前播放列表，并重置与旧列表相关的运行状态。
	pub fn replace_playlist(&mut self, new_playlist: Vec<u64>, playlist_base: PlaylistBase) {
		self.handle_replace_playlist(new_playlist);
		self.playlist_base = playlist_base;
	}

	/// 把一批歌曲插入到“当前歌曲之后”。
	pub fn insert_songs(&mut self, song_ids: &[u64]) {
		self.handle_insert_songs(song_ids);
	}

	pub fn toggle_shuffle(&mut self) {
		match self.shuffle_state {
			ShuffleState::Disabled => {
				self.handle_shuffle_playlist();
				self.shuffle_state = ShuffleState::Enabled;
			}
			ShuffleState::Enabled => {
				self.handle_restore_playlist_order();
				self.shuffle_state = ShuffleState::Disabled;
			}
		}
	}

	/// 打乱当前播放列表。
	pub fn shuffle_playlist(&mut self) {
		self.handle_shuffle_playlist();
	}

	/// 恢复播放列表到乱序前的原始顺序。
	pub fn restore_playlist_order(&mut self) {
		self.handle_restore_playlist_order();
	}

	/// 直接设置播放模式。
	pub fn set_play_mode(&mut self, mode: PlayMode) {
		self.handle_set_play_mode(mode);
	}

	/// 在顺序播放 / 列表循环 / 单曲循环之间轮换。
	pub fn toggle_play_mode(&mut self) {
		let new_mode = match self.mode {
			PlayMode::Sequence => PlayMode::LoopAll,
			PlayMode::LoopAll => PlayMode::LoopOne,
			PlayMode::LoopOne => PlayMode::Sequence,
		};
		self.handle_set_play_mode(new_mode);
	}

	/// 读取当前播放模式。
	pub fn play_mode(&self) -> PlayMode {
		self.mode
	}

	/// 切到下一首并返回最新快照状态。
	pub fn next_song(&mut self) -> PlayStatus {
		self.handle_next();
		self.status_snapshot()
	}

	/// 切到上一首并返回最新快照状态。
	pub fn prev_song(&mut self) -> PlayStatus {
		self.handle_prev();
		self.status_snapshot()
	}

	/// 指定播放某个歌曲 id；如果它不在列表中，会插入到当前歌曲之后。
	pub fn play(&mut self, song_id: u64) -> PlayStatus {
		self.handle_play_song(song_id);
		self.status_snapshot()
	}

	/// seek 到指定位置；若当前还没启动实际音频，会尝试先启动当前曲目。
	pub fn seek_to_duration(&mut self, position: time::Duration) -> PlayStatus {
		self.handle_seek(position);
		self.status_snapshot()
	}

	/// 在暂停与继续之间切换。
	pub fn toggle_pause_resume(&mut self) -> PlayStatus {
		self.handle_toggle_pause_resume();
		self.status_snapshot()
	}

	/// 显式暂停。
	pub fn pause(&mut self) -> PlayStatus {
		self.handle_pause();
		self.status_snapshot()
	}

	/// 显式继续播放。
	pub fn resume(&mut self) -> PlayStatus {
		self.handle_resume();
		self.status_snapshot()
	}

	/// 基于原始顺序重新洗牌，并尽量保持当前歌曲不变。
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

	/// 从 `original_playlist` 恢复播放顺序。
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

	/// 内部实现：替换播放列表并清空旧播放上下文。
	fn handle_replace_playlist(&mut self, new_playlist: Vec<u64>) {
		self.stop_current_playback();
		self.current_index = None;
		self.playlist = new_playlist;
		self.shuffle_state = ShuffleState::Disabled;
		self.original_playlist = None;
		self.error_count = 0;
		self.last_status = PlayStatus::Stopped;
		self.pending_seek_after_start = None;
	}

	/// 内部实现：把歌曲插到当前歌曲之后，并按需预取一小段歌曲。
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

		let to_prefetch = self.take_insert_prefetch_ids(song_ids);
		if !to_prefetch.is_empty() {
			let block_current_track = self.download_requests_current_track(&to_prefetch);
			self.handle_download(to_prefetch, block_current_track);
		}
	}

	/// 内部实现：开启乱序播放。
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

	/// 内部实现：关闭乱序播放并恢复原顺序。
	fn handle_restore_playlist_order(&mut self) {
		self.restore_playlist_order_inner();
	}

	/// 内部实现：设置播放模式。
	fn handle_set_play_mode(&mut self, mode: PlayMode) {
		self.mode = mode;
	}

	/// 内部实现：跳到下一首。
	fn handle_next(&mut self) {
		self.pending_seek_after_start = None;
		self.current_index = self.next_index();
		self.handle_start_current();
	}

	/// 内部实现：跳到上一首。
	fn handle_prev(&mut self) {
		self.pending_seek_after_start = None;
		self.current_index = self.prev_index();
		self.handle_start_current();
	}

	/// 内部实现：定位并开始播放目标歌曲。
	fn handle_play_song(&mut self, song_id: u64) {
		self.maybe_rebind_default_output_for_new_playback();
		self.pending_seek_after_start = None;

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

	/// 内部实现：seek。
	///
	/// 如果当前 sink 还没装载音频，会先尝试启动当前曲目；只有当启动后已经进入可播放/可暂停
	/// 状态时，才会真正执行 seek，避免在“等待下载”时把状态误改成 `Playing/Paused`。
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
			if matches!(self.status_snapshot(), PlayStatus::Playing(_) | PlayStatus::Paused(_)) {
				self.handle_apply_seek(position);
			} else {
				// 当前曲目尚未真正装载到 sink（例如仍在下载），延迟到开始播放后再 seek。
				self.pending_seek_after_start = Some(position);
			}
		} else {
			self.handle_apply_seek(position);
		}
	}

	/// 对当前 sink 执行真正的 seek，并同步刷新最近状态。
	fn handle_apply_seek(&mut self, position: time::Duration) {
		self.pending_seek_after_start = None;
		if let Err(e) = self.sink.try_seek(position) {
			eprintln!("Error seeking to {:?}: {}", position, e);
		}
		self.last_status = if self.sink.is_paused() {
			PlayStatus::Paused(self.playback_position())
		} else {
			PlayStatus::Playing(self.playback_position())
		};
	}

	/// 内部实现：在暂停/继续之间切换。
	///
	/// 如果当前 sink 为空但 `last_status` 里缓存了暂停位置（例如从磁盘恢复后），会在启动当前曲目后
	/// 重新 seek 到该位置，而不是从头开始。
	fn handle_toggle_pause_resume(&mut self) {
		if self.playlist.is_empty() {
			self.last_status = PlayStatus::Stopped;
			return;
		}

		let resume_position = self.resume_position_from_last_status();

		if self.current_track_requires_download() {
			self.pending_seek_after_start = resume_position;
			self.handle_start_current();
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
			self.maybe_restore_position_after_start(resume_position);
		} else {
			self.sink.pause();
			self.last_status = PlayStatus::Paused(self.playback_position());
		}
	}

	/// 内部实现：暂停当前播放。
	fn handle_pause(&mut self) {
		if self.playlist.is_empty() {
			self.last_status = PlayStatus::Stopped;
			return;
		}
		self.sink.pause();
		self.last_status = PlayStatus::Paused(self.playback_position());
	}

	/// 内部实现：继续播放。
	///
	/// 与 toggle 的空 sink 分支类似，这里也会尝试恢复此前缓存的暂停位置。
	fn handle_resume(&mut self) {
		if self.playlist.is_empty() {
			self.last_status = PlayStatus::Stopped;
			return;
		}

		let resume_position = self.resume_position_from_last_status();

		if self.current_track_requires_download() {
			self.pending_seek_after_start = resume_position;
			self.handle_start_current();
			return;
		}

		self.maybe_rebind_default_output_for_resume();

		if self.sink.empty() {
			self.maybe_rebind_default_output_for_new_playback();
			if self.current_index.is_none() {
				self.current_index = Some(0);
			}
			self.handle_start_current();
			self.maybe_restore_position_after_start(resume_position);
		} else {
			self.sink.play();
			self.last_status = PlayStatus::Playing(self.playback_position());
		}
	}

	/// 事件循环主入口。
	///
	/// 执行顺序大致为：
	/// 1. 处理空列表/缺失当前索引；
	/// 2. 处理暂停态；
	/// 3. 处理“当前曲目等待下载”的迁移；
	/// 4. 处理“当前曲目自然播放完成”的迁移；
	/// 5. 否则在有活动音频时更新为 `Playing(pos)`。
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

		if self.handle_download_transition_on_tick() {
			return;
		}

		if self.take_finished_playback_event() {
			if self.mode != PlayMode::LoopOne {
				self.current_index = self.next_index();
			}
			self.handle_start_current();
			return;
		}

		if !self.sink.empty() {
			self.last_status = PlayStatus::Playing(self.playback_position());
		}
	}

	/// 处理“当前曲目正在等待下载”这一状态的后续迁移。
	///
	/// 返回 `true` 表示当前 tick 已经完成了下载相关处理，调用方不应再继续执行其它分支。
	fn handle_download_transition_on_tick(&mut self) -> bool {
		if !matches!(self.last_status, PlayStatus::Downloading) {
			return false;
		}

		match self.current_track_source() {
			None => {
				self.last_status = PlayStatus::Stopped;
				true
			}
			Some((_, TrackSource::Missing | TrackSource::Downloading)) => {
				self.last_status = PlayStatus::Downloading;
				true
			}
			Some((_, TrackSource::Ready(_) | TrackSource::Unplayable)) => {
				self.handle_start_current();
				true
			}
		}
	}

	/// 消费一次“自然播放完成”事件。
	///
	/// 只有当：
	/// - 最近状态确实是 `Playing`；
	/// - 完成事件的 epoch 与当前活跃播放实例一致；
	/// - 该事件还没被处理过；
	/// 才认为当前曲目真正播放结束。
	fn take_finished_playback_event(&mut self) -> bool {
		if !matches!(self.last_status, PlayStatus::Playing(_)) {
			return false;
		}

		let finished_epoch = self.finished_playback_epoch.load(Ordering::Acquire);
		if finished_epoch == 0 {
			return false;
		}
		if finished_epoch != self.active_playback_epoch {
			return false;
		}
		if finished_epoch == self.handled_finished_epoch {
			return false;
		}

		self.handled_finished_epoch = finished_epoch;
		true
	}

	/// 根据当前曲目的 `TrackSource` 决定下一步怎么推进。
	///
	/// 这是真正的“当前曲目启动状态机”：
	/// - `Missing`：发起下载并进入 `Downloading`；
	/// - `Downloading`：继续等待；
	/// - `Unplayable`：跳过；
	/// - `Ready`：实际启动 rodio 播放，并顺手触发预测预取。
	fn handle_start_current(&mut self) {
		match self.current_track_source() {
			None => {
				self.last_status = PlayStatus::Stopped;
			}
			Some((song_id, TrackSource::Missing)) => {
				self.songs.borrow_mut().insert(song_id, TrackSource::Missing);
				self.stop_current_playback();
				self.last_status = PlayStatus::Downloading;
				let mut song_ids = vec![song_id];
				song_ids.extend(self.predicted_download_ids());
				song_ids.sort_unstable();
				song_ids.dedup();
				self.handle_download(song_ids, true);
			}
			Some((_, TrackSource::Downloading)) => {
				self.stop_current_playback();
				self.last_status = PlayStatus::Downloading;
			}
			Some((_, TrackSource::Unplayable)) => self.handle_advance_after_failure(),
			Some((song_id, TrackSource::Ready(path))) => {
				if let Err(e) = self.play_song(&path) {
					eprintln!("Error playing song {}: {}", song_id, e);
					self.songs.borrow_mut().insert(song_id, TrackSource::Unplayable);
					self.handle_advance_after_failure();
					return;
				}
				self.error_count = 0;
				self.push_history(song_id);
				self.try_apply_pending_seek_after_start();
				self.last_status = PlayStatus::Playing(self.playback_position());
				let song_ids = self.predicted_download_ids();
				if !song_ids.is_empty() {
					self.handle_download(song_ids, false);
				}
			}
		}
	}

	/// 发起下载请求。
	///
	/// `block_current_track = true` 表示当前曲目本身正在等待下载，此时需要把 `last_status`
	/// 置为 `Downloading`，让 tick 继续走“等待当前曲目可播放”的分支。
	///
	/// `block_current_track = false` 表示只是后台预取下一批歌曲；这种情况下不能改写
	/// 当前播放状态，否则会把正在播放的歌曲错误标记为下载中。
	fn handle_download(&mut self, song_ids: Vec<u64>, block_current_track: bool) {
		if song_ids.is_empty() {
			return;
		}
		self.download_songs(&song_ids);
		if block_current_track {
			self.last_status = PlayStatus::Downloading;
		}
	}

	/// 当前曲目播放失败后的推进策略：优先尝试下一首，超过失败阈值后停播。
	fn handle_advance_after_failure(&mut self) {
		self.error_count += 1;
		if self.error_count < self.playlist.len() {
			self.current_index = self.next_index();
			self.handle_start_current();
		} else {
			self.last_status = PlayStatus::Stopped;
		}
	}

	/// 读取当前索引对应的歌曲 id。
	fn current_song_id(&self) -> Option<u64> {
		self.current_index.and_then(|idx| self.playlist.get(idx).copied())
	}

	/// 查询当前歌曲在缓存层面的状态。
	fn current_track_source(&self) -> Option<(u64, TrackSource)> {
		let song_id = self.current_song_id()?;

		let source = self
			.songs
			.borrow()
			.get(&song_id)
			.cloned()
			.unwrap_or(TrackSource::Missing);

		Some((song_id, source))
	}

	/// 当前曲目是否仍依赖下载结果才能进入可播放状态。
	fn current_track_requires_download(&self) -> bool {
		matches!(
			self.current_track_source(),
			Some((_, TrackSource::Missing | TrackSource::Downloading))
		)
	}

	/// 判断一次下载请求是否会阻塞当前曲目启动。
	///
	/// 只有当：
	/// - 当前曲目本身处于 `Missing/Downloading`；
	/// - 且这次下载请求包含当前曲目；
	/// 才应该把播放器状态设置成 `Downloading`。
	fn download_requests_current_track(&self, song_ids: &[u64]) -> bool {
		let Some((current_song_id, current_source)) = self.current_track_source() else {
			return false;
		};

		matches!(current_source, TrackSource::Missing | TrackSource::Downloading)
			&& song_ids.contains(&current_song_id)
	}

	/// 计算下一首的索引。
	///
	/// 行为：
	/// - 普通顺序播放：走到末尾后返回 `None`；
	/// - 列表循环：末尾回到 0；
	/// - 乱序：在末尾时基于原始列表重新洗牌，并从新顺序的第 0 首开始；
	/// - 单曲循环不在这里处理，而是在 tick 中直接保持当前索引不动。
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

	/// 计算上一首的索引。
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

	/// 以“从头开始”的语义播放一个本地文件。
	fn play_song(&mut self, path: &Path) -> Result<()> {
		self.stop_current_playback();
		self.start_song_on_current_sink(path, None, false)
	}

	/// 读取 rodio 当前记录的播放位置。
	fn playback_position(&self) -> time::Duration {
		self.sink.get_pos()
	}

	/// 从 `last_status` 中提取一个“待恢复的暂停位置”。
	///
	/// 仅当当前 sink 为空时才返回该位置；如果 sink 里已经有活动音频，就应该信任实际 sink 的位置，
	/// 而不是旧缓存。
	fn resume_position_from_last_status(&self) -> Option<time::Duration> {
		if self.sink.empty() {
			if let PlayStatus::Paused(position) = self.last_status {
				return Some(position);
			}
		}

		None
	}

	/// 在空 sink 场景重新启动当前曲目后，尝试恢复到之前缓存的位置。
	fn maybe_restore_position_after_start(&mut self, position: Option<time::Duration>) {
		let Some(position) = position else {
			return;
		};

		if matches!(self.status_snapshot(), PlayStatus::Playing(_) | PlayStatus::Paused(_)) {
			self.handle_apply_seek(position);
		}
	}

	/// 尝试把延迟 seek 应用到已启动曲目；失败时保留请求供后续 tick 重试。
	fn try_apply_pending_seek_after_start(&mut self) {
		let Some(position) = self.pending_seek_after_start.take() else {
			return;
		};

		if let Err(e) = self.sink.try_seek(position) {
			eprintln!("Error restoring deferred seek to {:?}: {}", position, e);
			// 某些源在刚开始时暂不可 seek，保留请求给后续 tick 重试。
			self.pending_seek_after_start = Some(position);
		}
	}

	/// 根据当前 sink、暂停态和最近一次状态迁移，生成对外可消费的播放状态。
	///
	/// 优先级：
	/// 1. 空列表时必定为 `Stopped`；
	/// 2. sink 处于暂停态时为 `Paused(pos)`；
	/// 3. sink 仍有音频时为 `Playing(pos)`；
	/// 4. 其余情况回退到 `last_status`，用于保留 `Downloading` / `Stopped` 等离散状态。
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

	/// 在每次 tick 尾部检查并重试延迟 seek，避免首帧不可 seek 时丢失用户定位请求。
	fn maybe_retry_pending_seek_on_tick(&mut self) {
		if self.pending_seek_after_start.is_none() {
			return;
		}
		if matches!(self.status_snapshot(), PlayStatus::Playing(_) | PlayStatus::Paused(_)) {
			self.try_apply_pending_seek_after_start();
		}
	}

	/// 组装一帧发给 UI 的状态。
	///
	/// 这里只做只读快照，不推进任何状态机，方便 UI 在任意 tick 读取一致结果。
	fn state_frame_snapshot(&self) -> PlayerStateFrame {
		PlayerStateFrame {
			play_status: self.status_snapshot(),
			play_order: PlayOrderState {
				play_mode: self.mode,
				shuffle_state: self.shuffle_state,
			},
		}
	}

	/// 单次播放器事件循环：检测输出设备切换、推进状态机、重试延迟 seek，并按周期持久化。
	pub fn event_loop(&mut self) -> PlayerStateFrame {
		self.maybe_pause_for_default_output_change_on_tick();
		self.handle_tick();
		self.maybe_retry_pending_seek_on_tick();
		self.tick_counter = self.tick_counter.saturating_add(1);
		if self.runtime_config.persist_every_ticks > 0
			&& self.tick_counter.is_multiple_of(self.runtime_config.persist_every_ticks)
		{
			self.persist_state();
		}
		self.state_frame_snapshot()
	}
}
