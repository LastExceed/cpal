use std::fmt::{self, Display};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::{thread, vec};
use std::time::Duration;

use tap::Pipe;
use windows::Devices::Enumeration::DeviceInformation;
use windows::Win32::Media::Audio::_AUDCLNT_BUFFERFLAGS;

use crate::ErrorKind::BackendError;
use crate::*;
use crate::traits::*;

use self::utils::*;

mod enumerate;
mod properties;
mod utils;

pub struct Host;

impl Host {
	pub const fn new() -> Result<Self, crate::Error> {
		Ok(Self)
	}
}

impl HostTrait for Host {
	type Devices = enumerate::Devices;

	type Device = Device;

	fn is_available() -> bool {
		true
	}

	fn devices(&self) -> Result<Self::Devices, Error> {
		enumerate::devices()
			.map_err(|win_err| Error::with_message(BackendError, format!("{win_err:?}")))
	}

	fn default_input_device(&self) -> Option<Self::Device> {
		enumerate::get_default::<true>().ok()
	}
	
	fn default_output_device(&self) -> Option<Self::Device> {
		enumerate::get_default::<false>().ok()
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
	info: DeviceInformation,
	input: bool
}

impl Hash for Device {
	fn hash<H: Hasher>(&self, state: &mut H) {
		todo!()
	}
}

impl Display for Device {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		todo!()
	}
}

impl Device {
	
	pub fn description_inner(&self) -> WinResult<DeviceDescription> {
		let props       = self.info.Properties()?.pipe(properties::Facade);
		let name        = self.info.Name()?.to_string_lossy();
		let direction   = if self.input { DeviceDirection::Input } else { DeviceDirection::Output };
		// let device_type = props.device_type()?;
		
		let mut builder = DeviceDescriptionBuilder
			::new(name)
			.direction(direction);
			// .device_type(device_type);
		
		if let Some(addr) = props.physical_location()? {
			builder = builder.address(addr);
		}
		
		// .interface_type(crate::InterfaceType::Hdmi)
		// .manufacturer("")
		// .driver("")
			
		Ok(builder.build())
	}
}

impl DeviceTrait for Device {
	type SupportedInputConfigs = vec::IntoIter<SupportedStreamConfigRange>;
	type SupportedOutputConfigs = vec::IntoIter<SupportedStreamConfigRange>;
	type Stream = Stream;

	fn description(&self) -> Result<DeviceDescription, Error> {		
		self.description_inner()
			.map_err(|win_err| Error::with_message(BackendError, format!("{win_err:?}")))
	}

	fn id(&self) -> Result<DeviceId, Error> {
		todo!()
	}

	fn supported_input_configs(&self) -> Result<Self::SupportedInputConfigs, Error> {
		todo!()
	}

	fn supported_output_configs(&self) -> Result<Self::SupportedOutputConfigs, Error> {
		todo!()
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
		let path = self.info.Id().context("path")?;
		let client = Client::activate(&path).context("ac3 activate")?;
		
		let (format, period_frames) = client.get_current_shared_mode_engine_period().context("periods")?;
		client.initialize_shared_audio_stream(period_frames, format.cast_const()).context("init")?;
		let event = client.set_event_handle().context("set event")?;
		let total_frame_count = client.get_buffer_size().context("buf size")?;
		let waitable_stream_state = WaitableStreamState::new().context("signal")?.pipe(Arc::new);
		let capturer = client.get_capture_client().context("get cap")?;
		
		let stream = Stream {
			client: client.clone(), // COM pointers are essentially arcs
			total_frame_count,
			period_frames,
			state: Arc::clone(&waitable_stream_state)
		};
		
		thread::spawn(move || {
			while waitable_stream_state.wait_any() != StreamState::Exit {
				input_processing_loop(&event, &client, sample_format, &mut data_callback, &capturer, &waitable_stream_state);	
			}
		});
		
		Ok(stream)
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
		let path = self.info.Id().context("path")?;
		let client = Client::activate(&path).context("ac3 activate")?;

		let (format, period_frames) = client.get_current_shared_mode_engine_period().context("periods")?;
		client.initialize_shared_audio_stream(period_frames, format.cast_const()).context("init")?;
		let event = client.set_event_handle().context("set event")?;
		let total_frame_count = client.get_buffer_size().context("buf size")?;
		let waitable_stream_state = WaitableStreamState::new().context("signal")?.pipe(Arc::new);
		let renderer = client.get_render_client().context("get render")?;
		
		let stream = Stream {
			client: client.clone(), // COM pointers are essentially arcs
			total_frame_count,
			period_frames,
			state: Arc::clone(&waitable_stream_state)
		};
		
		thread::spawn(move || {
			while waitable_stream_state.wait_any() != StreamState::Exit {
				output_processing_loop(&event, &client, total_frame_count, sample_format, &mut data_callback, &renderer, &waitable_stream_state);	
			}
		});
		
		Ok(stream)
	}
}

fn input_processing_loop(
	event: &Event,
	client: &Client,
	sample_format: SampleFormat,
	data_callback: &mut (impl FnMut(&Data, &CallbackInfo) + Send + 'static),
	capturer: &Capturer,
	waitable_stream_state: &WaitableStreamState
) {
	loop {
		event.wait(None).unwrap();
		
		if waitable_stream_state.get() != StreamState::Running {
			break;
		}
		
		capture(capturer, sample_format, data_callback);
	}
	
	client.stop().unwrap();
	client.reset().unwrap();
}

fn output_processing_loop(
	event: &Event,
	client: &Client,
	total_frame_count: u32,
	sample_format: SampleFormat,
	data_callback: &mut (impl FnMut(&mut Data, &CallbackInfo) + Send + 'static),
	renderer: &Renderer,
	waitable_stream_state: &WaitableStreamState
) {
	loop {
		event.wait(None).unwrap();
		let padding = client.get_current_padding().unwrap();
		
		use StreamState::*;
		#[deny(non_snake_case, reason = "prevent accidental wildcard patterns from typos")]
		match waitable_stream_state.get() {
			Running => render(total_frame_count - padding, sample_format, data_callback, renderer),
			
			Draining if padding == 0 => {
				waitable_stream_state.set(Halted);
				break;
			},
			
			Draining => (),
			
			Halted | Exit => return,
		};
	}
	
	client.stop().unwrap();
	client.reset().unwrap();
}

fn capture(
	capturer: &Capturer,
	sample_format: SampleFormat,
	data_callback: &mut (impl FnMut(&Data, &CallbackInfo) + Send + 'static),
) {
	let fn_capture = |ptr: *mut u8, n_frames: u32, flags: _AUDCLNT_BUFFERFLAGS, dev_pos: u64, qpc_pos: u64| {
		if n_frames == 0 {
			return;
		}
		let sample_count = n_frames * 2; // todo
		let data = unsafe { Data::from_parts(ptr.cast(), sample_count as _, sample_format) };
		let cbi = CallbackInfo {
			timestamp: StreamTimestamp { callback: StreamInstant::ZERO, device: StreamInstant::ZERO },
			xrun: false,
		};
		
		data_callback(&data, &cbi);
	};
	capturer.with_buffer(fn_capture).unwrap();
}

fn render(
	frame_count  : u32,
	sample_format: SampleFormat,
	data_callback: &mut (impl FnMut(&mut Data, &CallbackInfo) + Send + 'static),
	renderer     : &Renderer
) {
	let sample_count = frame_count * 2; // todo
	
	let fn_render = |ptr: *mut u8| {
		let mut data = unsafe { Data::from_parts(ptr.cast(), sample_count as _, sample_format) };
		let cbi = CallbackInfo {
			timestamp: StreamTimestamp { callback: StreamInstant::ZERO, device: StreamInstant::ZERO },
			xrun: false,
		};

		data_callback(&mut data, &cbi);
		
		if sample_format == SampleFormat::I24 {
			// shift_up(ptr, sample_count);
		}
	};
	
	renderer.with_buffer(frame_count, fn_render).unwrap();
}

pub struct Stream {
	client: Client,
	total_frame_count: u32,
	period_frames: u32,
	state: Arc<WaitableStreamState>
}

impl StreamTrait for Stream {
	fn start(&self) -> Result<(), Error> {
		self.state.set(StreamState::Running);
		self.client.start().context("start")?;
		Ok(())
	}

	fn pause(&self) -> Result<(), Error> {
		self.client.stop().context("stop")?;
		self.state.set(StreamState::Halted);
		Ok(())
	}

	fn stop(&self, timeout: Option<Duration>) -> Result<(), Error> {
		self.state.set(StreamState::Draining);
		if self.state.wait_value(StreamState::Halted, timeout) {
			Ok(())
		} else {
			todo!("timed out")
		}
	}

	fn buffer_size(&self) -> Result<FrameCount, Error> {
		self.client
			.get_current_padding()
			.context("pad")
			.map(|padding| self.total_frame_count - padding + self.period_frames)
	}

	fn now(&self) -> StreamInstant {
		todo!()
	}
}