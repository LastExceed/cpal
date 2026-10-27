use std::fmt::{self, Debug, Display};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::vec;
use std::time::Duration;

use parking_lot::Mutex;
use tap::Pipe;
use wasapi::{AudioClient, AudioClock, Direction};

use crate::ErrorKind::{BackendError, Other, ResourceExhausted};
use crate::*;
use crate::traits::*;

use self::instance::{DeviceInstance, StreamInstance};
use self::utils::*;

mod instance;
mod processing;
mod winrt;
mod utils;

type Devices = vec::IntoIter<DeviceHandle>;
type SupportedConfigs = vec::IntoIter<SupportedStreamConfigRange>;

pub struct Host;

impl Host {
	pub const fn new() -> Result<Self, Error> {
		Ok(Self)
	}
}

impl HostTrait for Host {
	type Devices = Devices;
	type Device = DeviceHandle;

	fn is_available() -> bool {
		true
	}

	fn devices(&self) -> Result<Self::Devices, Error> {
		DeviceInstance
			::all()
    		.into_iter()
			.map(DeviceHandle::from)
			.collect::<Vec<_>>() // todo: redundant
			.into_iter()
			.pipe(Ok)
	}

	fn default_input_device(&self) -> Option<Self::Device> {
		DeviceInstance
			::system_default(Direction::Capture)
			.map(DeviceHandle::from)
	}
	
	fn default_output_device(&self) -> Option<Self::Device> {
		DeviceInstance
			::system_default(Direction::Render)
			.map(DeviceHandle::from)
	}
}

#[derive(Clone)]
pub struct DeviceHandle(Arc<DeviceInstance>);

impl Debug for DeviceHandle {
	fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
		todo!()
	}
}

impl PartialEq for DeviceHandle {
	fn eq(&self, _other: &Self) -> bool {
		todo!()
	}
}

impl Eq for DeviceHandle {}

impl Hash for DeviceHandle {
	fn hash<H: Hasher>(&self, _state: &mut H) {
		todo!()
	}
}

impl Display for DeviceHandle {
	fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
		todo!()
	}
}

impl From<DeviceInstance> for DeviceHandle {
	fn from(value: DeviceInstance) -> Self {
		value
			.pipe(Arc::new)
			.pipe(Self)
	}
}

impl DeviceTrait for DeviceHandle {
	type SupportedInputConfigs = SupportedConfigs;
	type SupportedOutputConfigs = SupportedConfigs;
	type Stream = StreamHandle;

	fn description(&self) -> Result<DeviceDescription, Error> {		
		self.0
    		.description_inner()
			.map_err(|win_err| Error::with_message(BackendError, format!("{win_err:?}")))
	}

	fn id(&self) -> Result<DeviceId, Error> {
		todo!()
	}

	fn supported_input_configs(&self) -> Result<Self::SupportedInputConfigs, Error> {
		self.0
			.configs(Direction::Capture)
			.map_err(|win_err| Error::with_message(BackendError, format!("{win_err:?}")))
	}

	fn supported_output_configs(&self) -> Result<Self::SupportedOutputConfigs, Error> {
		self.0
			.configs(Direction::Render)
			.map_err(|win_err| Error::with_message(BackendError, format!("{win_err:?}")))
	}

	fn default_input_config(&self) -> Result<SupportedStreamConfig, Error> {
		todo!()
	}

	fn default_output_config(&self) -> Result<SupportedStreamConfig, Error> {
		todo!()
	}

	fn build_input_stream_raw<D, E>(
		&self,
		config  : StreamConfig,
		format  : SampleFormat,
		data_cb : D,
		error_cb: E,
		_timeout: Option<Duration>,
	) -> Result<Self::Stream, Error>
	where
		D: FnMut(&Data, &CallbackInfo) + Send + 'static,
		E: FnMut(Error) + Send + 'static
	{
		self.0
			.build_stream_inner(
				config,
				format,
				data_cb,
				error_cb,
				Direction::Capture,
				AudioClient::get_audiocaptureclient,
				processing::capture::<D>
			)
			.map_err(|win_err| Error::with_message(BackendError, format!("{win_err:?}")))
	}

	fn build_output_stream_raw<D, E>(
		&self,
		config  : StreamConfig,
		format  : SampleFormat,
		data_cb : D,
		error_cb: E,
		_timeout: Option<Duration>,
	) -> Result<Self::Stream, Error>
	where
		D: FnMut(&mut Data, &CallbackInfo) + Send + 'static,
		E: FnMut(Error) + Send + 'static
	{
		self.0
			.build_stream_inner(
				config,
				format,
				data_cb,
				error_cb,
				Direction::Render,
				AudioClient::get_audiorenderclient,
				processing::render
			)
			.map_err(|win_err| Error::with_message(BackendError, format!("{win_err:?}")))
	}
}

pub struct StreamHandle(Mutex<StreamInstance>);

unsafe impl Send for StreamHandle {}
unsafe impl Sync for StreamHandle {}

impl Debug for StreamHandle {
	fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
		todo!()
	}
}

impl From<StreamInstance> for StreamHandle {
	fn from(value: StreamInstance) -> Self {
		value
			.pipe(Mutex::new)
			.pipe(Self)
	}
}
impl StreamTrait for StreamHandle {
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
		todo!()
	}

	fn now(&self) -> StreamInstant {
		todo!()
	}
}