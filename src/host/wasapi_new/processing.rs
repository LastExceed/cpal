use crate::*;

use super::native::*;
use super::utils::*;

pub fn run_loop<'client>(
	buffer_event  : &Event,
	client        : &'client Client,
	stream_state  : &WaitableStreamState,
	mut process_fn: impl FnMut(&'client Client) + Send + 'static
) {
	let mut previous_state = stream_state.get();
	
	loop {
		Event::wait_multiple([buffer_event, stream_state.event()], None).unwrap().unwrap();
		let mut current_state = stream_state.get();

		use StreamState::*;
		#[deny(non_snake_case, reason = "prevent accidental wildcard patterns from typos")]
		match current_state {
			Running => {
				if previous_state == Halted {
					client.start().unwrap();
				}
				process_fn(client);
			}
			Draining if previous_state == Halted => {
				client.reset().unwrap();
				stream_state.set(Halted).unwrap();
				current_state = Halted;
			}
			Draining => {
				if client.get_current_padding().unwrap() == 0 {
					client.stop().unwrap();
					client.reset().unwrap();
					stream_state.set(Halted).unwrap();
					current_state = Halted;
				}
			}
			Halted => {
				if previous_state != Halted {
					client.stop().unwrap();
				}
			}
			Exit => break
		}

		previous_state = current_state;
	}
}

pub fn capture(
	capturer: &Capturer,
	data_callback: &mut (impl FnMut(&Data, &CallbackInfo) + Send + 'static),
	sample_format: SampleFormat,
	scratch: &mut Box<[u8]>
) {
	let fn_capture = |packet: &CapturePacket| {
		if packet.n_frames == 0 {
			return;
		}

		let channel_count = 2; // todo
		let sample_count  = (packet.n_frames * channel_count) as usize; // todo

		let data_ptr =
			if packet.flags.silent() {
				scratch.fill(0);
				scratch.as_mut_ptr().cast()
			} else if sample_format == SampleFormat::I24 {
				shift_down(packet.ptr, sample_count, scratch); // WASAPI stores I24 in the upper bits
				scratch.as_mut_ptr().cast()
			} else {
				packet.ptr.cast()
			};

		let data = unsafe { Data::from_parts(data_ptr, sample_count, sample_format) };
		let cbi = CallbackInfo {
			timestamp: StreamTimestamp {
				callback: StreamInstant::from_nanos(packet.qpc_pos),
				device: StreamInstant::from_nanos(packet.dev_pos)
			},
			xrun: packet.flags.data_discontinuity(),
		};
		
		data_callback(&data, &cbi);
	};
	capturer.with_buffer(fn_capture).unwrap();
}

pub fn render(
	renderer     : &Renderer,
	data_callback: &mut (impl FnMut(&mut Data, &CallbackInfo) + Send + 'static),
	sample_format: SampleFormat,
	client       : &Client,
	total_frames : u32
) {
	let frame_count = total_frames - client.get_current_padding().unwrap();
	let sample_count = frame_count * 2; // todo
	
	let fn_render = |ptr: *mut u8| {
		let mut data = unsafe { Data::from_parts(ptr.cast(), sample_count as _, sample_format) };
		let cbi = CallbackInfo {
			timestamp: StreamTimestamp { callback: StreamInstant::ZERO, device: StreamInstant::ZERO },
			xrun: false,
		};

		data_callback(&mut data, &cbi);
		
		if sample_format == SampleFormat::I24 {
			shift_up(ptr, sample_count as _);
		}
	};
	
	renderer.with_buffer(frame_count, fn_render).unwrap();
}