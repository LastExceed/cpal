use std::collections::HashSet;
use std::fmt::{self, Display};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::{thread, vec};
use std::time::Duration;

use parking_lot::Mutex;
use tap::Pipe;
use wasapi::Direction;
use windows::Devices::Enumeration::DeviceInformation;
use windows_core::Interface;

use crate::ErrorKind::{BackendError, Other, ResourceExhausted};
use crate::*;
use crate::traits::*;

use self::native::*;
use self::utils::*;

mod enumerate;
mod processing;
mod properties;
mod native;
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

#[derive(Debug, Clone)]
pub struct Device {
	info: DeviceInformation,
	input: bool,
	preliminary_client: Arc<Mutex<Option<Client>>>
}

impl PartialEq for Device {
	fn eq(&self, other: &Self) -> bool {
		self.info == other.info
		&& self.input == other.input
		&& Arc::ptr_eq(&self.preliminary_client, &other.preliminary_client)
	}
}

impl Eq for Device {}

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
	
	fn activate_client(&self) -> Result<Client, Error> {
		let path = self.info.Id().context("path")?;
		Client::activate(&path).context("ac3 activate")
	}
	
	fn configs(&self) -> Result<(), Error> {
		let temp_client = self.activate_client()?;
		let x = wasapi::AudioClient::from_iaudioclient(temp_client.0.cast().cast().unwrap(), if self.input { Direction::Capture } else { Direction::Render });

		let mut all = HashSet::new();
		
		use SampleFormat::*;
		for &sample_rate in COMMON_SAMPLE_RATES {
			for cpal_format in [I16, I24, I32, F32] {
				for channel_mask in [0b1, 0b11, 0b111, 0b1111, 0b11111] {
					let wave_format = create_waveformatextensible(sample_rate, channel_mask, cpal_format);
					if let Some(x) = temp_client.is_format_supported(&raw const wave_format.Format)? {
						all.insert(x);
					}
				}
			}
		}
		
		Ok(())
	}

	fn build_stream_inner<FnProcess, FnCreateFnProcess>(&self, fn_create_fn_process: FnCreateFnProcess) -> Result<Stream, Error>
	where 
		FnProcess        : FnMut (&Client) + Send + 'static,
		FnCreateFnProcess: FnOnce(&Stream) -> FnProcess
	{
		let client = self.activate_client()?;

		let (format, period_frames) = client.get_current_shared_mode_engine_period().context("periods")?;
		
		client.initialize_shared_audio_stream(period_frames, format.cast_const()).context("init")?;
		
		let buffer_event = client.set_event_handle().context("set event")?;
		let total_frames = client.get_buffer_size().context("buf size")?;
		let stream_state = WaitableStreamState::new().context("signal")?.pipe(Arc::new);
		
		let stream = Stream {
			total_frames,
			period_frames,
			client: client.clone(), // COM pointers are essentially arcs
			state: Arc::clone(&stream_state)
		};
		
		let fn_process = fn_create_fn_process(&stream);

		thread::spawn(move || {
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
		self.build_stream_inner(|stream: &Stream| {
			let capturer = stream.client.get_capturer().unwrap();

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
		self.build_stream_inner(|stream: &Stream| {
			let renderer     = stream.client.get_renderer().unwrap();
			let total_frames = stream.total_frames;
			
			move |client| processing::render(&renderer, &mut data_callback, sample_format, client, total_frames)
		})
	}
}

#[derive(Debug)]
pub struct Stream {
	total_frames : u32,
	period_frames: u32,
	client       : Client,
	state        : Arc<WaitableStreamState>
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
			.context("pad")
			.map(|padding| self.total_frames - padding + self.period_frames)
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