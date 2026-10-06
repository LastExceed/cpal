use std::iter::{Chain, Map};

use tap::Pipe;
use windows::Devices::Enumeration::DeviceInformation;
use windows::Media::Devices::{AudioDeviceRole, MediaDevice};
use windows::Win32::Media::Audio::{PKEY_AudioEndpoint_FormFactor, PKEY_AudioEndpoint_JackSubType};
use windows_collections::{IIterable, IIterator};
use windows_core::HSTRING;

use super::Device;
use super::native::to_aqs;
use super::utils::*;

pub type Devices = Chain<PerDirectionIter, PerDirectionIter>;
type PerDirectionIter = Map<IIterator<DeviceInformation>, MapFn>;
type MapFn = fn(DeviceInformation) -> Device;

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
	let fn_get_selector = if INPUT {
		MediaDevice::GetAudioCaptureSelector
	} else {
		MediaDevice::GetAudioRenderSelector
	};
	
	let map_fn: MapFn = |info| Device { info, input: INPUT }; // coerce unique function type to fn ptr to keep the iterator type nameable

	DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&fn_get_selector()?, extra_props)?
		.join()?
		.into_iter()
		.map(map_fn)
		.pipe(Ok)
}

pub fn get_default<const INPUT: bool>() -> WinResult<Device> {
	let id_getter =
		if INPUT { MediaDevice::GetDefaultAudioCaptureId }
		else     { MediaDevice::GetDefaultAudioRenderId };

	id_getter(AudioDeviceRole::Default)?
		.pipe_ref(DeviceInformation::CreateFromIdAsync)?
		.join()
		.map(|info| Device { info, input: INPUT })
}