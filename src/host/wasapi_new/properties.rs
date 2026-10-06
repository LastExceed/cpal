use tap::Pipe;
use windows::Foundation::IPropertyValue;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio;
use windows::Win32::Media::Audio::*;
use windows_collections::IMapView;
use windows_core::{Array, HSTRING, IInspectable, Interface, h};
use crate::DeviceType;
use super::native::to_aqs;
use super::utils::WinResult;

const PHYSICAL_DEVICE_LOCATION: &HSTRING = h!("System.Devices.PhysicalDeviceLocation");

pub struct Facade(pub IMapView<HSTRING, IInspectable>);

impl Facade {
	pub fn device_type(&self) -> WinResult<DeviceType> {
		let eff = self.0
			.Lookup(&to_aqs(&PKEY_AudioEndpoint_FormFactor))?
			.cast::<IPropertyValue>()?
			.GetInt32()?
			.pipe(EndpointFormFactor);
		
		#[deny(nonstandard_style, reason = "typos would silently turn these into wildcard patterns")]
		#[expect(unused_qualifications, reason = "rustc bug?")]
		#[expect(clippy::match_same_arms, reason = "happenstance")]
		match eff {
			Audio::RemoteNetworkDevice       => DeviceType::Unknown,
			Audio::Speakers                  => DeviceType::Speaker,
			Audio::LineLevel                 => DeviceType::Unknown,
			Audio::Headphones                => DeviceType::Headphones,
			Audio::Microphone                => DeviceType::Microphone,
			Audio::Headset                   => DeviceType::Headset,
			Audio::Handset                   => DeviceType::Handset,
			Audio::UnknownDigitalPassthrough => DeviceType::Unknown,
			Audio::SPDIF                     => DeviceType::Unknown,
			Audio::DigitalAudioDisplayDevice => DeviceType::Unknown,
			Audio::UnknownFormFactor         => DeviceType::Unknown,
			_                                => DeviceType::Unknown
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