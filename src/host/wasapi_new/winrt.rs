use crate::DeviceType;
use std::iter::{Chain, Map};
use super::DeviceHandle;
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
type MapFn = fn(DeviceInformation) -> DeviceHandle;

pub fn to_aqs(PROPERTYKEY { fmtid, pid }: &PROPERTYKEY) -> HSTRING {
	format!("{{{fmtid:?}}} {pid}").into()
}

pub fn devices() -> WinResult<Devices> {
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

pub fn get_default<const INPUT: bool>() -> WinResult<DeviceHandle> {
	let fn_get_id =
		if INPUT { MediaDevice::GetDefaultAudioCaptureId }
		else     { MediaDevice::GetDefaultAudioRenderId };

	fn_get_id(AudioDeviceRole::Default)?
		.pipe_ref(DeviceInformation::CreateFromIdAsync)?
		.join()
		.map(|_info| todo!())
}

const PHYSICAL_DEVICE_LOCATION: &HSTRING = h!("System.Devices.PhysicalDeviceLocation");

pub struct Facade(pub IMapView<HSTRING, IInspectable>);

impl Facade {
	pub fn device_type(&self) -> WinResult<DeviceType> {
		let aeff = self.0
			.Lookup(&to_aqs(&PKEY_AudioEndpoint_FormFactor))?
			.cast::<IPropertyValue>()?
			.GetInt32()?
			.pipe(EndpointFormFactor);
		
		#[deny(nonstandard_style, reason = "typos would silently turn these into wildcard patterns")]
		#[expect(unused_qualifications, reason = "rustc bug?")]
		#[expect(clippy::match_same_arms, reason = "happenstance")]
		match aeff {
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