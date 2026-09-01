use tap::Pipe;
use windows::Foundation::IPropertyValue;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::*;
use windows_collections::IMapView;
use windows_core::{Array, HSTRING, IInspectable, Interface, h};
use crate::{DeviceDescription, DeviceType};
use super::utils::{WinResult, to_aqs};

const PHYSICAL_DEVICE_LOCATION: &HSTRING = h!("System.Devices.PhysicalDeviceLocation");

pub struct Facade(pub IMapView<HSTRING, IInspectable>);

impl Facade {
	pub fn device_type(&self) -> WinResult<DeviceType> {
		let eff = self.0
			.Lookup(&to_aqs(&PKEY_AudioEndpoint_FormFactor))?
			.cast::<IPropertyValue>()?
			.GetInt32()?
			.pipe(EndpointFormFactor);
		
		#[deny(non_snake_case, reason = "typos would silently turn these into wildcard patterns")]
		match eff {
			RemoteNetworkDevice       => DeviceType::Unknown,
			Speakers                  => DeviceType::Speaker,
			LineLevel                 => DeviceType::Unknown,
			Headphones                => DeviceType::Headphones,
			Microphone                => DeviceType::Microphone,
			Headset                   => DeviceType::Headset,
			Handset                   => DeviceType::Handset,
			UnknownDigitalPassthrough => DeviceType::Unknown,
			SPDIF                     => DeviceType::Unknown,
			DigitalAudioDisplayDevice => DeviceType::Unknown,
			UnknownFormFactor         => DeviceType::Unknown,
			_                         => DeviceType::Unknown
		}
		.pipe(Ok)
	}
	
	pub fn physical_location(&self) -> WinResult<Option<String>> {
		match self.0.Lookup(PHYSICAL_DEVICE_LOCATION) {
			Err(error) if error.code() == S_OK => Ok(None), // https://github.com/microsoft/windows-rs/issues/4858
			Err(error) => Err(error),
			
			Ok(inspectable) => {
				let mut buf = Array::new();
				inspectable.cast::<IPropertyValue>()?.GetUInt8Array(&mut buf)?;
				Ok(Some(format!("{:x?}", buf.as_slice())))
			},
		}
	}
}