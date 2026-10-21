use std::collections::HashSet;
use std::fmt::{self, Debug, Display};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::{thread, vec};
use std::time::Duration;

use parking_lot::Mutex;
use tap::Pipe;
use wasapi::{AudioClient, DeviceEnumerator, Direction, SampleType, ShareMode, StreamMode, WaveFormat};

use crate::ErrorKind::{BackendError, Other, ResourceExhausted};
use crate::*;
use crate::traits::*;

use self::utils::*;

mod processing;
mod winrt;
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
	type Devices = Devices;
	type Device = Device;

	fn is_available() -> bool {
		true
	}

	fn devices(&self) -> Result<Self::Devices, Error> {
		let enumerator = DeviceEnumerator::new().unwrap();
		
		let capture_collection = enumerator.get_device_collection(&Direction::Capture).unwrap();
		let  render_collection = enumerator.get_device_collection(&Direction::Render ).unwrap();
		
		let capture_iter = capture_collection.into_iter();
		let  render_iter =  render_collection.into_iter();

		capture_iter
			.chain(render_iter)
			.filter_map(Result::ok)
			.map(|native_device| Device { native_device, provisional_client: Mutex::default() })
			.collect::<Vec<_>>()
			.into_iter()
			.pipe(Ok)
	}

	fn default_input_device(&self) -> Option<Self::Device> {
		Device::system_default(Direction::Capture)
	}
	
	fn default_output_device(&self) -> Option<Self::Device> {
		Device::system_default(Direction::Render)
	}
}

pub struct Device {
	native_device: wasapi::Device,
	provisional_client: Mutex<Option<SendSync<AudioClient>>>
}

impl Debug for Device {
	fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
		todo!()
	}
}

impl PartialEq for Device {
	fn eq(&self, _other: &Self) -> bool {
		todo!()
	}
}

impl Eq for Device {}

impl Hash for Device {
	fn hash<H: Hasher>(&self, _state: &mut H) {
		todo!()
	}
}

impl Display for Device {
	fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
		todo!()
	}
}

impl From<wasapi::Device> for Device {
	fn from(value: wasapi::Device) -> Self {
		Self {
			native_device: value,
			provisional_client: Mutex::default()
		}
	}
}

// SAFETY:
// WASAPI's COM interfaces implement IMarshal,
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Device {
	fn system_default(direction: Direction) -> Option<Self> {
		DeviceEnumerator
			::new()
			.unwrap()
			.get_default_device(&direction)
			.ok()
			.map(Self::from)
	}
	
	fn ensure_client(&self) {
		let mut guard = self.provisional_client.lock();
		if guard.is_some() {
			return;
		}
		*guard = self
			.native_device
			.get_iaudioclient()
			.unwrap()
			.pipe(SendSync)
			.pipe(Some);
	}
	
	pub fn description_inner(&self) -> WinResult<DeviceDescription> {
		DeviceDescriptionBuilder
			::new(self.native_device.get_description().unwrap())
    		.driver(self.native_device.get_interface_friendlyname().unwrap())
    		.direction(self.native_device.get_direction().into())
    		// .device_type(todo!())
    		// .interface_type(todo!())
			.build()
			.pipe(Ok)
	}
	
	fn configs(&self, direction: Direction) -> Result<SupportedConfigs, Error> {
		use SampleFormat::*;
		
		if self.native_device.get_direction() != direction {
			return Err(todo!());
		}
		
		self.ensure_client();
		let guard = self.provisional_client.lock();
		let client = guard.as_ref().unwrap();

		self.native_device
			.get_data_ranges()
			.unwrap()
			.into_iter()
			.flat_map(move |range| {
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
			})
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

	fn build_stream_inner<FnProcess, FnCreateFnProcess>(
		&self,
		config: StreamConfig,
		sample_format: SampleFormat,
		direction: Direction,
		fn_create_fn_process: FnCreateFnProcess
	) -> Result<Stream, Error>
	where 
		FnProcess        : FnMut (&AudioClient) + Send + 'static,
		FnCreateFnProcess: FnOnce(&Stream) -> FnProcess
	{
		self.ensure_client();
		let mut client = self.provisional_client.lock().take().unwrap();

		let waveformat = get_waveformat();
		let period_frames = match config.buffer_size {
			BufferSize::Fixed(n) => n,
			BufferSize::Default => client.get_shared_mode_engine_period(&waveformat).unwrap().default
		};
		client
			.initialize_client(
				&waveformat,
				&direction,
				&StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 }
			)
			.unwrap();
		
		let buffer_event = client.set_get_eventhandle().unwrap().pipe(SendSync);
		let total_frames = client.get_buffer_size().unwrap();
		let stream_state = WaitableStreamState::new().unwrap().pipe(Arc::new);

		let stream = Stream {
			total_frames,
			period_frames,
			client,
			state: Arc::clone(&stream_state)
		};
		
		let scratch_len = stream.total_frames as usize * config.channels as usize * sample_format.sample_size();
		let mut scratch = vec![0_u8; scratch_len].into_boxed_slice();

		let fn_process = fn_create_fn_process(&stream);

		thread::spawn(move || {
			let buffer_event = buffer_event;
			processing::run_loop(
				&buffer_event,
				&client,
				&stream_state,
				fn_process
			);
		});
		
		Ok(stream)
	}
}

impl DeviceTrait for Device {
	type SupportedInputConfigs = SupportedConfigs;
	type SupportedOutputConfigs = SupportedConfigs;
	type Stream = Stream;

	fn description(&self) -> Result<DeviceDescription, Error> {		
		self.description_inner()
			.map_err(|win_err| Error::with_message(BackendError, format!("{win_err:?}")))
	}

	fn id(&self) -> Result<DeviceId, Error> {
		todo!()
	}

	fn supported_input_configs(&self) -> Result<Self::SupportedInputConfigs, Error> {
		self.configs(Direction::Capture)
	}

	fn supported_output_configs(&self) -> Result<Self::SupportedOutputConfigs, Error> {
		self.configs(Direction::Render)
	}

	fn default_input_config(&self) -> Result<SupportedStreamConfig, Error> {
		todo!()
	}

	fn default_output_config(&self) -> Result<SupportedStreamConfig, Error> {
		todo!()
	}

	fn build_input_stream_raw<D, E>(
		&self,
		config: StreamConfig,
		sample_format: SampleFormat,
		mut data_callback: D,
		error_callback: E,
		timeout: Option<Duration>,
	) -> Result<Self::Stream, Error>
	where
		D: FnMut(&Data, &CallbackInfo) + Send + 'static,
		E: FnMut(Error) + Send + 'static
	{
		self.build_stream_inner(
			config,
			sample_format,
			Direction::Capture,
			|stream: &Stream| {
				let capturer = stream
					.client
					.get_audiocaptureclient()
					.unwrap()
					.pipe(SendSync);

				let scratch_len = stream.total_frames as usize * config.channels as usize * sample_format.sample_size();
				let mut scratch = vec![0_u8; scratch_len].into_boxed_slice();
				
				move |_client| processing::capture(&capturer, &mut data_callback, sample_format, &mut scratch)
			})
	}

	fn build_output_stream_raw<D, E>(
		&self,
		config: StreamConfig,
		sample_format: SampleFormat,
		mut data_callback: D,
		error_callback: E,
		timeout: Option<Duration>,
	) -> Result<Self::Stream, Error>
	where
		D: FnMut(&mut Data, &CallbackInfo) + Send + 'static,
		E: FnMut(Error) + Send + 'static
	{
		self.build_stream_inner(config, Direction::Render, |stream: &Stream| {
			let renderer     = stream.client.get_audiorenderclient().unwrap().pipe(SendSync);
			
			let scratch_len = stream.total_frames as usize * config.channels as usize * sample_format.sample_size();
			let mut scratch = vec![0_u8; scratch_len].into_boxed_slice();
			
			move |client| processing::render(&renderer, &mut data_callback, sample_format, client, &mut scratch)
		})
	}
}

pub struct Stream {
	total_frames : u32,
	period_frames: u32,
	client       : SendSync<AudioClient>,
	state        : Arc<WaitableStreamState>
}

unsafe impl Send for Stream {}
unsafe impl Sync for Stream {}

impl Debug for Stream {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		todo!()
	}
}

impl StreamTrait for Stream {
	fn start(&self) -> Result<(), Error> {
		self.state
			.set(StreamState::Running)
			.context("set stream state")
	}

	fn pause(&self) -> Result<(), Error> {
		self.state
			.set(StreamState::Halted)
			.context("set stream state")
	}

	fn stop(&self, timeout: Option<Duration>) -> Result<(), Error> {
		self.state
			.set(StreamState::Draining)
			.context("set stream state")?;

		let wait_completed = self
			.state
			.event()
			.wait_single(timeout)
			.context("wait_single")?;
		
		if !wait_completed {
			return Err(Error::with_message(ResourceExhausted, "timeout expired"));
		}
		
		if self.state.get() != StreamState::Halted {
			return Err(Error::with_message(Other, "draining interrupted"));
		}
		
		Ok(())
	}

	fn buffer_size(&self) -> Result<FrameCount, Error> {
		self.client
			.get_current_padding()
			.map(|padding| self.total_frames - padding + self.period_frames)
			.map_err(|_| todo!())
	}

	fn now(&self) -> StreamInstant {
		todo!()
	}
}

impl Drop for Stream {
	fn drop(&mut self) {
		_ = self.state.set(StreamState::Exit);
	}
}