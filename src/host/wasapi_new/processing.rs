use std::mem::MaybeUninit;

use wasapi::{AudioCaptureClient, AudioClient, AudioRenderClient, Handle};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::*;

use super::utils::*;

pub fn run_loop<'ac>(
	buffer_event  : &Handle,
	audio_client  : &'ac AudioClient,
	stream_state  : &WaitableStreamState,
	mut process_fn: impl FnMut(&'ac AudioClient) + Send + 'static
) {
	let mut previous_state = stream_state.get();
	
	loop {
		Event::wait_multiple([buffer_event.as_handle(), stream_state.event()], None).unwrap().unwrap();
		let mut current_state = stream_state.get();

		use StreamState::*;
		#[deny(non_snake_case, reason = "prevent accidental wildcard patterns from typos")]
		match current_state {
			Running => {
				if previous_state == Halted {
					audio_client.start_stream().unwrap();
				}
				process_fn(audio_client);
			}
			Draining if previous_state == Halted => {
				audio_client.reset_stream().unwrap();
				stream_state.set(Halted).unwrap();
				current_state = Halted;
			}
			Draining => {
				if audio_client.get_current_padding().unwrap() == 0 {
					audio_client.stop_stream().unwrap();
					audio_client.reset_stream().unwrap();
					stream_state.set(Halted).unwrap();
					current_state = Halted;
				}
			}
			Halted => {
				if previous_state != Halted {
					audio_client.stop_stream().unwrap();
				}
			}
			Exit => break
		}

		previous_state = current_state;
	}
}

pub fn capture(
	capturer: &AudioCaptureClient,
	data_callback: &mut (impl FnMut(&Data, &CallbackInfo) + Send + 'static),
	sample_format: SampleFormat,
	scratch: &mut Box<[u8]>
) {
	let (frame_count, buffer_info) = capturer.read_from_device(scratch).unwrap();
	let channel_count = 2; // todo
	let sample_count  = (frame_count * channel_count) as usize; // todo
	
	let scratch_ptr = scratch.as_mut_ptr();

	if buffer_info.flags.silent {
		scratch.fill(0);
	} else if sample_format == SampleFormat::I24 {
		shift_down(scratch_ptr, sample_count); // WASAPI stores I24 in the upper bits
	} else {
		//
	}

	let data = unsafe { Data::from_parts(scratch.as_mut_ptr().cast(), sample_count, sample_format) };
	let cbi = create_cbi(buffer_info.timestamp, buffer_info.flags.data_discontinuity);
	
	data_callback(&data, &cbi);
}

pub fn render(
	renderer     : &AudioRenderClient,
	data_callback: &mut (impl FnMut(&mut Data, &CallbackInfo) + Send + 'static),
	sample_format: SampleFormat,
	audio_client : &AudioClient,
	scratch      : &mut Box<[u8]>
) {
	let frame_count = audio_client.get_available_space_in_frames().unwrap();
	let channel_count = 2; // todo
	let sample_count  = (frame_count * channel_count) as usize; // todo
	
	let scratch_ptr = scratch.as_mut_ptr();
	
	if sample_format == SampleFormat::I24 {
		shift_up(scratch_ptr, sample_count); // WASAPI stores I24 in the upper bits
	}
	
	let mut data = unsafe { Data::from_parts(scratch_ptr.cast(), sample_count, sample_format) };
	let cbi = create_cbi(todo!("prediction"), false);
	
	data_callback(&mut data, &cbi);
	renderer.write_to_device(frame_count as _, scratch, None).unwrap();
}

fn create_cbi(device_hns: u64, xrun: bool) -> CallbackInfo {
	CallbackInfo {
		timestamp: StreamTimestamp {
			callback: StreamInstant::from_nanos(now_hns() * 100),
			device: StreamInstant::from_nanos(device_hns * 100)
		},
		xrun
	}
}

fn now_hns() -> u64 {
	let mut count = MaybeUninit::uninit();
	let mut freq = MaybeUninit::uninit();
	
	let hns = unsafe {
		QueryPerformanceCounter (count.as_mut_ptr()).unwrap();
		QueryPerformanceFrequency(freq.as_mut_ptr()).unwrap();
		
		count.assume_init() as i128
			* 10_000_000
			/ freq.assume_init() as i128
	};
	
	hns as _
}