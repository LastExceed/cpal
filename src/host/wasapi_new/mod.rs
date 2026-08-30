use std::fmt::Display;

use crate::traits::{DeviceTrait, HostTrait, StreamTrait};

pub struct Host;

impl HostTrait for Host {
	type Devices = !;

	type Device = Device;

	fn is_available() -> bool {
		true
	}

	fn devices(&self) -> Result<Self::Devices, crate::Error> {
		todo!()
	}

	fn default_input_device(&self) -> Option<Self::Device> {
		todo!()
	}

	fn default_output_device(&self) -> Option<Self::Device> {
		todo!()
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Device;

impl Display for Device {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		todo!()
	}
}

impl DeviceTrait for Device {
	type SupportedInputConfigs = !;

	type SupportedOutputConfigs = !;

	type Stream = Stream;

	fn description(&self) -> Result<crate::DeviceDescription, crate::Error> {
		todo!()
	}

	fn id(&self) -> Result<crate::DeviceId, crate::Error> {
		todo!()
	}

	fn supported_input_configs(&self) -> Result<Self::SupportedInputConfigs, crate::Error> {
		todo!()
	}

	fn supported_output_configs(&self) -> Result<Self::SupportedOutputConfigs, crate::Error> {
		todo!()
	}

	fn default_input_config(&self) -> Result<crate::SupportedStreamConfig, crate::Error> {
		todo!()
	}

	fn default_output_config(&self) -> Result<crate::SupportedStreamConfig, crate::Error> {
		todo!()
	}

	fn build_input_stream_raw<D, E>(
		&self,
		config: crate::StreamConfig,
		sample_format: crate::SampleFormat,
		data_callback: D,
		error_callback: E,
		timeout: Option<std::time::Duration>,
	) -> Result<Self::Stream, crate::Error>
	where
		D: FnMut(&crate::Data, &crate::CallbackInfo) + Send + 'static,
		E: FnMut(crate::Error) + Send + 'static {
		todo!()
	}

	fn build_output_stream_raw<D, E>(
		&self,
		config: crate::StreamConfig,
		sample_format: crate::SampleFormat,
		data_callback: D,
		error_callback: E,
		timeout: Option<std::time::Duration>,
	) -> Result<Self::Stream, crate::Error>
	where
		D: FnMut(&mut crate::Data, &crate::CallbackInfo) + Send + 'static,
		E: FnMut(crate::Error) + Send + 'static {
		todo!()
	}
}

struct Stream;

impl StreamTrait for Stream {
	fn start(&self) -> Result<(), crate::Error> {
		todo!()
	}

	fn pause(&self) -> Result<(), crate::Error> {
		todo!()
	}

	fn stop(&self, timeout: Option<std::time::Duration>) -> Result<(), crate::Error> {
		todo!()
	}

	fn buffer_size(&self) -> Result<crate::FrameCount, crate::Error> {
		todo!()
	}

	fn now(&self) -> crate::StreamInstant {
		todo!()
	}
}