use std::collections::HashSet;
use std::fmt::{self, Debug, Display};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::{thread, vec};
use std::time::Duration;

use parking_lot::Mutex;
use tap::Pipe;
use wasapi::{AudioClient, AudioClock, Device as MMDevice, DeviceEnumerator, Direction, SampleType, ShareMode, StreamMode, WasapiError, WaveFormat};
use windows::Devices::Enumeration::DeviceInformation;
use windows_core::Interface;

use crate::ErrorKind::{BackendError, ResourceExhausted};
use crate::*;
use crate::traits::*;

use self::processing::FnProcess;
use self::utils::*;

mod processing;
mod device;
mod utils;

type Devices = vec::IntoIter<Device>;
type SupportedConfigs = vec::IntoIter<SupportedStreamConfigRange>;

pub struct Host;

impl Host {
	pub const fn new() -> Result<Self, Error> {
		Ok(Self)
	}
}

impl HostTrait for Host {
	type Devices = device::Devices;
	type Device = Device;

	fn is_available() -> bool {
		true
	}

	fn devices(&self) -> Result<Self::Devices, Error> {
		device::all().map_err(convert_error)
	}

	fn default_input_device(&self) -> Option<Self::Device> {
		device::default::<true>()
			.map(Device)
    		.ok()
	}
	
	fn default_output_device(&self) -> Option<Self::Device> {
		device::default::<false>()
			.map(Device)
    		.ok()
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device(DeviceInformation);

impl Hash for Device {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.0.as_raw().hash(state);
	}
}

impl Display for Device {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		if let Ok(name) = self.0.Name() {
			Display::fmt(&name, formatter)
		} else {
			Display::fmt("(device name lookup failed)", formatter)
		}
	}
}

impl Device {
	fn description_inner(&self) -> WinResult<DeviceDescription> {
		let name = self.0.Name()?.to_string_lossy();
		let facade = device::Facade(self.0.Properties()?);
		let (device_type, interface_type) = facade.kind()?;
		
		DeviceDescriptionBuilder
			::new(name.clone())
    		.driver(name)
    		// .direction(todo!())
    		.device_type(device_type)
    		.interface_type(interface_type)
			.build()
			.pipe(Ok)
	}
	
	fn mm_device(&self) -> Result<MMDevice, WasapiError> {
		let enumerator = DeviceEnumerator::new()?;
		self.0
			.Id()
			.map_err(WasapiError::from)?
			.to_string_lossy()
			.pipe(|id| enumerator.get_device(&id))
	}
	
	fn default_config(&self) -> Result<SupportedStreamConfig, Error> {
		let waveformat = self
			.mm_device()?
			.get_iaudioclient()?
			.get_mixformat()?;
		
		SupportedStreamConfig {
			channels: waveformat.get_nchannels(),
			sample_rate: waveformat.get_samplespersec(), // misleading name, should be framespersec
			buffer_size: SupportedBufferSize::default(),
			sample_format: match waveformat.get_validbitspersample() {
					8 => SampleFormat::I8,
				16 => SampleFormat::I16,
				24 => SampleFormat::I24,
				32 if waveformat.get_subformat()? == SampleType::Float => SampleFormat::F32,
				32 => SampleFormat::I32,
				_  => return Err(Error::with_message(BackendError, "sound server reported unexpected mix format"))
			}
		}.pipe(Ok)
	}

	fn configs(&self, direction: Direction) -> Result<SupportedConfigs, WasapiError> {
		use SampleFormat::*;
		
		let mm_device = self.mm_device()?;
		
		if mm_device.get_direction() != direction {
			return Err(WasapiError::IllegalDeviceDirection(direction as _));
		}
		
		let client = mm_device.get_iaudioclient()?;

		mm_device
			.get_data_ranges()?
			.into_iter()
			.flat_map(move |range| 
				COMMON_SAMPLE_RATES
				.into_iter()
				.copied()
				.skip_while(move |rate| *rate < range.min_samplerate)
				.take_while(move |rate| *rate <= range.max_samplerate)
				.flat_map(move |sample_rate|
					(1..=range.max_channels)
					.flat_map(move |channel_count|
						[I8, I16, I24, I32, F32]
						.into_iter()
						.filter(move |format|
							range
							.sample_type()
							.is_none_or(|ty|
								(ty == SampleType::Int)
									== format.is_int()
							)
						)
						.map(move |format| (format, format.sample_size() * 8))
						.skip_while(move |(_format, store_bits)| range.min_bits_per_sample >  *store_bits as u32)
						.take_while(move |(_format, store_bits)| range.max_bits_per_sample >= *store_bits as u32)
						.map(move |(format, store_bits)| WaveFormat::new(
							store_bits,
							format.bits_per_sample() as _,
							&SampleType::Int,
							sample_rate as _,
							channel_count as _,
							None
						))
					)
				)
			)
			.collect::<HashSet<_>>() // dedup to avoid redundant requests to the audio client
			.into_iter()
			.filter_map(|waveformat|
				client
				.is_supported(&waveformat, &ShareMode::Shared)
				.map(|ret| ret.unwrap_or(waveformat))
				.ok()
			)
			.collect::<HashSet<_>>() // dedup again, since `.is_supported()` might have reported the same "closest match" multiple times
			.into_iter()
			.map(SupportedStreamConfigRange::from)
			.collect::<Vec<_>>() 
			.into_iter()
			.pipe(Ok)
	}
	
	fn build_stream_inner<Processor, DataCallback, ErrorCallback>(
		&self,
		config          : StreamConfig,
		sample_format   : SampleFormat,
		mut data_cb     : DataCallback,
		error_cb        : ErrorCallback,
		direction       : Direction, // const generics when?
		fn_new_processor: fn(&AudioClient) -> Result<Processor, WasapiError>, // unique function ZSTs when?
		fn_process      : FnProcess<Processor, DataCallback> // unique function ZSTs when?
	) -> Result<Stream, WasapiError>
	where
		Processor    : 'static,
		DataCallback : Send + 'static,
		ErrorCallback: FnMut(Error) + Send + 'static
	{
		let mm_device = self.mm_device()?;
		let mut audio_client = mm_device.get_iaudioclient()?;
		
		let waveformat = get_waveformat();
		let period_frames = match config.buffer_size {
			BufferSize::Fixed(n) => n,
			BufferSize::Default => audio_client.get_shared_mode_engine_period(&waveformat)?.default
		};

		audio_client.initialize_client(
			&waveformat,
			&direction,
			&StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 }
		)?;
		
		let processor    = audio_client.pipe_ref(fn_new_processor)?.pipe(SendSync);
		let audio_client = audio_client.pipe(SendSync).pipe(Arc::new);
		let audio_clock  = audio_client.get_audioclock()?.pipe(SendSync).pipe(Arc::new);
		let buffer_event = audio_client.set_get_eventhandle()?.pipe(SendSync);
		let total_frames = audio_client.get_buffer_size()?;
		let mut scratch  = ScratchBuffer::new(total_frames as usize * config.channels as usize, sample_format);
		let stream_state = WaitableStreamState::new()?.pipe(Arc::new);

		let audio_client2 = Arc::clone(&audio_client);
		let stream_state2 = Arc::clone(&stream_state);
		thread::spawn(move || {
			_ = processing::run_loop(
				&buffer_event,
				&audio_client2,
				&processor,
				&stream_state2,
				&mut scratch,
				&mut data_cb,
				fn_process,
				sample_format,
				config.channels as _
			)
			.map_err(error_cb);
		});

		StreamInner {
			total_frames,
			period_frames,
			audio_client,
			audio_clock,
			stream_state,
			last_hns: u64::MIN
		}
		.pipe(Stream::from)
		.pipe(Ok)
	}
}

impl DeviceTrait for Device {
	type SupportedInputConfigs = SupportedConfigs;
	type SupportedOutputConfigs = SupportedConfigs;
	type Stream = Stream;

	fn description(&self) -> Result<DeviceDescription, Error> {
		self.description_inner()
    		.map_err(convert_error)
	}

	fn id(&self) -> Result<DeviceId, Error> {
		self.0
			.Id()
			.map_err(convert_error)?
			.to_string_lossy()
			.into_boxed_str()
			.pipe(|id| DeviceId(HostId::WasapiNew, id))
			.pipe(Ok)
	}

	fn supported_input_configs(&self) -> Result<Self::SupportedInputConfigs, Error> {
		self.configs(Direction::Capture)
			.map_err(convert_error)
	}

	fn supported_output_configs(&self) -> Result<Self::SupportedOutputConfigs, Error> {
		self.configs(Direction::Render)
			.map_err(convert_error)
	}

	fn default_input_config(&self) -> Result<SupportedStreamConfig, Error> {
		self.default_config()
	}

	fn default_output_config(&self) -> Result<SupportedStreamConfig, Error> {
		self.default_config()
	}

	fn build_input_stream_raw<DataCallback, ErrorCallback>(
		&self,
		config  : StreamConfig,
		format  : SampleFormat,
		data_cb : DataCallback,
		error_cb: ErrorCallback,
		_timeout: Option<Duration>,
	) -> Result<Self::Stream, Error>
	where
		DataCallback: FnMut(&Data, &CallbackInfo) + Send + 'static,
		ErrorCallback: FnMut(Error) + Send + 'static
	{
		self.build_stream_inner(
				config,
				format,
				data_cb,
				error_cb,
				Direction::Capture,
				AudioClient::get_audiocaptureclient,
				processing::capture
			)
			.map_err(convert_error)
	}

	fn build_output_stream_raw<DataCallback, ErrorCallback>(
		&self,
		config  : StreamConfig,
		format  : SampleFormat,
		data_cb : DataCallback,
		error_cb: ErrorCallback,
		_timeout: Option<Duration>,
	) -> Result<Self::Stream, Error>
	where
		DataCallback: FnMut(&mut Data, &CallbackInfo) + Send + 'static,
		ErrorCallback: FnMut(Error) + Send + 'static
	{
		self.build_stream_inner(
				config,
				format,
				data_cb,
				error_cb,
				Direction::Render,
				AudioClient::get_audiorenderclient,
				processing::render
			)
			.map_err(convert_error)
	}
}

#[derive(Debug)]
pub struct Stream(Mutex<StreamInner>);

unsafe impl Send for Stream {}
unsafe impl Sync for Stream {}

impl From<StreamInner> for Stream {
	fn from(value: StreamInner) -> Self {
		value
			.pipe(Mutex::new)
			.pipe(Self)
	}
}
impl StreamTrait for Stream {
	fn start(&self) -> Result<(), Error> {
		self.0.lock().start()
	}

	fn pause(&self) -> Result<(), Error> {
		self.0.lock().pause()
	}

	fn stop(&self, timeout: Option<Duration>) -> Result<(), Error> {
		self.0.lock().stop(timeout)
	}

	fn buffer_size(&self) -> Result<FrameCount, Error> {
		self.0.lock().buffer_size()
	}

	fn now(&self) -> StreamInstant {
		self.0.lock().now()
	}
}

struct StreamInner {
	total_frames : u32,
	period_frames: u32,
	audio_client : Arc<SendSync<AudioClient>>,
	audio_clock  : Arc<SendSync<AudioClock>>,
	stream_state : Arc<WaitableStreamState>,
	last_hns    : u64
}

impl Debug for StreamInner {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		todo!() // https://github.com/HEnquist/wasapi-rs/pull/86
	}
}

impl StreamInner {
	pub fn start(&mut self) -> Result<(), Error> {
		if self.stream_state.get() == StreamState::Halted {
			self.audio_client.start_stream().context("start stream")?;
		}

		self.stream_state
			.set(StreamState::Running, true)
			.context("set stream state")
	}
	
	pub fn pause(&mut self) -> Result<(), Error> {
		if self.stream_state.get() != StreamState::Halted {
			self.audio_client.stop_stream().context("stop stream")?;
		}

		self.stream_state
			.set(StreamState::Halted, true)
			.context("set stream state")
	}
	
	pub fn stop(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
		if self.stream_state.get() == StreamState::Halted {
			return Ok(());
		}
		
		self.stream_state
			.set(StreamState::Halted, false)
			.context("set stream state")?;

		self.stream_state
    		.event()
			.wait_single(timeout)
			.context("wait drain")?
			.ok_or(Error::with_message(ResourceExhausted, "timeout expired"))
	}

	pub fn buffer_size(&self) -> Result<FrameCount, Error> {
		self.audio_client
			.get_current_padding()
			.map(|padding| self.total_frames - padding + self.period_frames)
			.map_err(convert_error)
	}

	pub fn now(&mut self) -> StreamInstant {
		self.last_hns = self
			.audio_clock
			.get_position()
			.map(|(_pos, hns)| hns)
			.unwrap_or(u64::MIN)
			.max(self.last_hns);
		
		StreamInstant::from_nanos(self.last_hns * 100)
	}
}

impl Drop for StreamInner {
	fn drop(&mut self) {
		_ = self.stream_state.set(StreamState::Exit, false);
	}
}