use std::fmt::Display;
use std::hash::Hash;
use std::thread;

use tap::Pipe;
use windows::Devices::Enumeration::DeviceInformation;

use crate::ErrorKind::BackendError;
use crate::*;
use crate::traits::*;

use self::utils::*;

mod enumerate;
mod properties;
mod utils;

pub struct Host;

impl Host {
	pub fn new() -> Result<Self, crate::Error> {
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
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		todo!()
	}
}

impl Display for Device {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		todo!()
	}
}

impl Device {
	
	pub fn description_inner(&self) -> WinResult<DeviceDescription> {
		let props       = self.info.Properties()?.pipe(properties::Facade);
		let name        = self.info.Name()?.to_string_lossy();
		let direction   = if self.input { DeviceDirection::Input } else { DeviceDirection::Output };
		let device_type = props.device_type()?;
		
		let mut builder = DeviceDescriptionBuilder
			::new(name)
			.direction(direction)
			.device_type(device_type);
		
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
	type SupportedInputConfigs = std::vec::IntoIter<SupportedStreamConfigRange>;
	type SupportedOutputConfigs = std::vec::IntoIter<SupportedStreamConfigRange>;
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
		data_callback: D,
		error_callback: E,
		timeout: Option<std::time::Duration>,
	) -> Result<Self::Stream, Error>
	where
		D: FnMut(&Data, &CallbackInfo) + Send + 'static,
		E: FnMut(Error) + Send + 'static
	{
		todo!()
	}

	fn build_output_stream_raw<D, E>(
		&self,
		config: StreamConfig,
		sample_format: SampleFormat,
		mut data_callback: D,
		error_callback: E,
		timeout: Option<std::time::Duration>,
	) -> Result<Self::Stream, Error>
	where
		D: FnMut(&mut Data, &CallbackInfo) + Send + 'static,
		E: FnMut(Error) + Send + 'static
	{
		let path = self.info.Id().unwrap();
		let client = Client::activate(&path).context("ac3 activate")?;

		let (format, period_frames) = client.get_current_shared_mode_engine_period().context("periods")?;
		client.initialize_shared_audio_stream(period_frames, format.cast_const()).context("init")?;
		let event = client.set_event_handle().unwrap();
		let total_frame_count = client.get_buffer_size().context("buf size")?;
		let signaled_stream_state = SignaledStreamState::new().context("signal")?;
		let renderer = client.get_render_client().context("get render")?;
		
		let stream = Stream {
			client: client.clone(),
			total_frame_count: 0,
			period_frames,
			stream_state: signaled_stream_state
		};
		
		// let status = "";
		
		thread::spawn(move || {
			loop {
				signaled_stream_state.wait_value(StreamState::Running, None).unwrap();
				processing_loop(event, &client, total_frame_count, sample_format, &mut data_callback, &renderer, &signaled_stream_state);	
			}
		});
		
		Ok(stream)
	}
}

fn processing_loop(
	event: Event,
	client: &Client,
	total_frame_count: u32,
	sample_format: SampleFormat,
	data_callback: &mut (impl FnMut(&mut Data, &CallbackInfo) + Send + 'static),
	renderer: &Renderer,
	signaled_stream_state: &SignaledStreamState
) {
	loop {
		event.wait(None).unwrap();
		let pad = client.get_current_padding().unwrap();
		
		match signaled_stream_state.get() {
			StreamState::Halted => break,
			
			StreamState::Draining if pad == 0 => {
				client.stop().unwrap();
				signaled_stream_state.set(StreamState::Halted).unwrap();
				client.reset().unwrap();
				break;
			},
			
			StreamState::Draining => continue,
			
			StreamState::Running => render(total_frame_count - pad, sample_format, data_callback, renderer),
		};
	}
}

fn render(
	frame_count: u32,
	sample_format   : SampleFormat,
	data_callback   : &mut (impl FnMut(&mut Data, &CallbackInfo) + Send + 'static),
	renderer        : &Renderer
) {
	let sample_count = frame_count * 2; // todo
	
	let func = |ptr: *mut u8| {
		let mut data = unsafe { Data::from_parts(ptr.cast(), sample_count as _, sample_format) };
		let cbi = CallbackInfo {
			timestamp: StreamTimestamp { callback: StreamInstant::ZERO, device: StreamInstant::ZERO },
			xrun: false,
		};

		data_callback(&mut data, &cbi);
		
		if sample_format == SampleFormat::I24 {
			shift_up(ptr, sample_count);
		}
	};
	
	renderer.with_buffer(frame_count, func).unwrap();
}

// fn foo(ptr: *mut u8, sample_count: u32, sample_format: SampleFormat) {
// 	let mut data = unsafe { Data::from_parts(ptr.cast(), sample_count as _, sample_format) };
// 	let cbi = CallbackInfo {
// 		timestamp: StreamTimestamp { callback: StreamInstant::ZERO, device: StreamInstant::ZERO },
// 		xrun: false,
// 	};

// 	data_callback(&mut data, &cbi);
	
// 	if sample_format == SampleFormat::I24 {
// 		shift_up(ptr, sample_count);
// 	}
// }

pub struct Stream {
	client: Client,
	total_frame_count: u32,
	period_frames: u32,
	stream_state: SignaledStreamState
}

impl StreamTrait for Stream {
	fn start(&self) -> Result<(), Error> {
		self.client.start().context("start")
	}

	fn pause(&self) -> Result<(), Error> {
		self.client.stop().context("stop")
	}

	fn stop(&self, _timeout: Option<std::time::Duration>) -> Result<(), Error> {
		self.client.stop().context("stop")?;
		self.client.reset().context("reset")
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