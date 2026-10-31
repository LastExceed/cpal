use crate::{DeviceType, InterfaceType};
use std::iter::{Chain, Map};
use super::Device;
use super::utils::WinResult;
use tap::Pipe;
use windows_collections::{IIterable, IIterator};
use windows_collections::IMapView;
use windows_core::{Array, HSTRING, IInspectable, Interface, h};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Foundation::IPropertyValue;
use windows::Media::Devices::{AudioDeviceRole, MediaDevice};
use windows::Win32::Foundation::{PROPERTYKEY, S_OK};
use windows::Win32::Media::Audio;
use windows::Win32::Media::Audio::*;

pub type Devices = Chain<PerDirectionIter, PerDirectionIter>;
type PerDirectionIter = Map<IIterator<DeviceInformation>, MapFn>;
type MapFn = fn(DeviceInformation) -> Device;

pub fn to_aqs(PROPERTYKEY { fmtid, pid }: &PROPERTYKEY) -> HSTRING {
	format!("{{{fmtid:?}}} {pid}").into()
}

pub fn all() -> WinResult<Devices> {
	let extra_props: IIterable<HSTRING> = vec![
		to_aqs(&PKEY_AudioEndpoint_FormFactor),
		to_aqs(&PKEY_AudioEndpoint_JackSubType),
	].into();
	
	let infos_capture = device_infos::<true >(&extra_props)?;
	let infos_render  = device_infos::<false>(&extra_props)?;
	
	infos_capture
		.chain(infos_render)
		.pipe(Ok)
}

fn device_infos<const INPUT: bool>(extra_props: &IIterable<HSTRING>) -> WinResult<PerDirectionIter> {
	let fn_get_selector =
		if INPUT { MediaDevice::GetAudioCaptureSelector }
		else     { MediaDevice::GetAudioRenderSelector };
	
	let map_fn: MapFn = |_info| todo!(); // coerces unique function type to fn ptr to keep the iterator type nameable

	DeviceInformation
		::FindAllAsyncAqsFilterAndAdditionalProperties(&fn_get_selector()?, extra_props)?
		.join()?
		.into_iter()
		.map(map_fn)
		.pipe(Ok)
}

pub fn default<const INPUT: bool>() -> WinResult<DeviceInformation> {
	let fn_get_id =
		if INPUT { MediaDevice::GetDefaultAudioCaptureId }
		else     { MediaDevice::GetDefaultAudioRenderId };

	fn_get_id(AudioDeviceRole::Default)?
		.pipe_ref(DeviceInformation::CreateFromIdAsync)?
		.join()
}

const PHYSICAL_DEVICE_LOCATION: &HSTRING = h!("System.Devices.PhysicalDeviceLocation");

pub struct Facade(pub IMapView<HSTRING, IInspectable>);

impl Facade {
	pub fn kind(&self) -> WinResult<(DeviceType, InterfaceType)> {
		let aeff = self.0
			.Lookup(&to_aqs(&PKEY_AudioEndpoint_FormFactor))?
			.cast::<IPropertyValue>()?
			.GetInt32()?
			.pipe(EndpointFormFactor);

		// InterfaceType::BuiltIn => todo!(),
		// InterfaceType::Usb => todo!(),
		// InterfaceType::Bluetooth => todo!(),
		// InterfaceType::Pci => todo!(),
		// InterfaceType::FireWire => todo!(),
		// InterfaceType::Thunderbolt => todo!(),
		// InterfaceType::Hdmi => todo!(),
		// InterfaceType::Line => todo!(),
		// InterfaceType::Spdif => todo!(),
		// InterfaceType::Network => todo!(),
		// InterfaceType::Virtual => todo!(),
		// InterfaceType::DisplayPort => todo!(),
		// InterfaceType::Aggregate => todo!(),
		// InterfaceType::Unknown => todo!(),

		#[deny(nonstandard_style, reason = "prevent accidental wildcards")]
		#[expect(unused_qualifications, reason = "rustc bug?")]
		#[expect(clippy::match_same_arms, reason = "happenstance")]
		match aeff {
			Audio::Speakers                  => (DeviceType::Speaker   , InterfaceType::Unknown),
			Audio::Headphones                => (DeviceType::Headphones, InterfaceType::Unknown),
			Audio::Microphone                => (DeviceType::Microphone, InterfaceType::Unknown),
			Audio::Headset                   => (DeviceType::Headset   , InterfaceType::Unknown),
			Audio::Handset                   => (DeviceType::Handset   , InterfaceType::Unknown),
			Audio::RemoteNetworkDevice       => (DeviceType::Unknown   , InterfaceType::Network),
			Audio::LineLevel                 => (DeviceType::Unknown   , InterfaceType::Line),
			Audio::SPDIF                     => (DeviceType::Unknown   , InterfaceType::Spdif),
			Audio::DigitalAudioDisplayDevice => (DeviceType::Unknown   , InterfaceType::DisplayPort),
			Audio::UnknownDigitalPassthrough => (DeviceType::Unknown   , InterfaceType::Unknown),
			Audio::UnknownFormFactor         => (DeviceType::Unknown   , InterfaceType::Unknown),
			_                                => (DeviceType::Unknown   , InterfaceType::Unknown)
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