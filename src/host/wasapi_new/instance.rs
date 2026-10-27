use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use tap::Pipe;
use wasapi::{AudioClient, AudioClock, DeviceEnumerator, Direction, SampleType, ShareMode, StreamMode, WasapiError, WaveFormat};

use crate::ErrorKind::ResourceExhausted;
use crate::*;

use super::processing::FnProcess;
use super::{StreamHandle, SupportedConfigs, processing};
use super::utils::{ScratchBuffer, SendSync, StreamState, WaitableStreamState, WinResult, get_waveformat};

pub struct DeviceInstance {
	native_device: wasapi::Device,
	provisional_client: Mutex<Option<SendSync<AudioClient>>>
}

// SAFETY:
// WASAPI's COM interfaces implement IMarshal,
unsafe impl Send for DeviceInstance {}
unsafe impl Sync for DeviceInstance {}

impl From<wasapi::Device> for DeviceInstance {
	fn from(value: wasapi::Device) -> Self {
		Self {
			native_device: value,
			provisional_client: Mutex::default()
		}
	}
}

impl DeviceInstance {	
	pub fn system_default(direction: Direction) -> Option<Self> {
		DeviceEnumerator
			::new()
			.unwrap()
			.get_default_device(&direction)
			.ok()
			.map(Self::from)
	}
	
	pub fn all() -> Vec<Self> {
		let enumerator = DeviceEnumerator::new().unwrap();
		
		let capture_collection = enumerator.get_device_collection(&Direction::Capture).unwrap();
		let  render_collection = enumerator.get_device_collection(&Direction::Render ).unwrap();
		
		let capture_iter = capture_collection.into_iter();
		let  render_iter =  render_collection.into_iter();

		capture_iter
			.chain(render_iter)
			.filter_map(Result::ok)
			.map(Self::from)
			.collect()
	}
	
	pub fn ensure_client(&self) -> Result<(), WasapiError> {
		let mut guard = self.provisional_client.lock();
		if guard.is_some() {
			return Ok(());
		}
		*guard = self
			.native_device
			.get_iaudioclient()?
			.pipe(SendSync)
			.pipe(Some);
		
		Ok(())
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
	
	pub fn configs(&self, direction: Direction) -> Result<SupportedConfigs, WasapiError> {
		use SampleFormat::*;
		
		if self.native_device.get_direction() != direction {
			return Err(todo!());
		}
		
		self.ensure_client()?;
		let guard = self.provisional_client.lock();
		let client = guard.as_ref().unwrap();

		self.native_device
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

	pub fn build_stream_inner<Processor, DataCallback, ErrorCallback>(
		&self,
		config          : StreamConfig,
		sample_format   : SampleFormat,
		mut data_cb     : DataCallback,
		error_cb        : ErrorCallback,
		direction       : Direction, // const generics when?
		fn_new_processor: fn(&AudioClient) -> Result<Processor, WasapiError>, // unique function ZSTs when?
		fn_process      : FnProcess<Processor, DataCallback> // unique function ZSTs when?
	) -> Result<StreamHandle, WasapiError>
	where
		Processor    : 'static,
		DataCallback : Send + 'static,
		ErrorCallback: FnMut(Error) + Send + 'static
	{
		self.ensure_client()?;
		let mut audio_client = self
			.provisional_client
			.lock()
			.take()
			.unwrap();

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
		
		let audio_client = Arc::new(audio_client);
		let processor    = fn_new_processor(&audio_client)?.pipe(SendSync);
		let audio_clock  = audio_client.get_audioclock()?.pipe(SendSync).pipe(Arc::new);
		
		let buffer_event = audio_client.set_get_eventhandle()?.pipe(SendSync);
		let stream_state = WaitableStreamState::new()?.pipe(Arc::new);
		
		let total_frames = audio_client.get_buffer_size()?;
		let mut scratch  = ScratchBuffer::new(total_frames as usize * config.channels as usize, sample_format);

		// must clone *before* moving into the thread::spawn() closure
		let audio_client2 = Arc::clone(&audio_client);
		let stream_state2 = Arc::clone(&stream_state);
		
		thread::spawn(move || {
			_ = processing::run_loop(
				&buffer_event,
				&audio_client2,
				&processor,
				&stream_state2,
				&mut data_cb,
				fn_process,
				sample_format,
				&mut scratch
			)
			.map_err(error_cb);
		});

		StreamInstance {
			total_frames,
			period_frames,
			audio_client,
			audio_clock,
			stream_state
		}
		.pipe(StreamHandle::from)
		.pipe(Ok)
	}
}

pub struct StreamInstance {
	total_frames : u32,
	period_frames: u32,
	audio_client : Arc<SendSync<AudioClient>>,
	audio_clock  : Arc<SendSync<AudioClock>>,
	stream_state : Arc<WaitableStreamState>
}

impl StreamInstance {
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
			.map_err(|_| todo!())
	}

	pub fn now(&self) -> StreamInstant {
		todo!()//self.audio_clock.get_position()
	}
}

impl Drop for StreamInstance {
	fn drop(&mut self) {
		_ = self.stream_state.set(StreamState::Exit, false);
	}
}