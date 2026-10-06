use std::mem::transmute;
use std::sync::atomic::{AtomicU8, Ordering};
use std::slice;
use tap::Pipe;

use super::native::Event;

pub type WinResult<T> = windows_core::Result<T>;

#[expect(clippy::cast_ptr_alignment, reason = "guaranteed by WASAPI")]
pub fn shift_up(ptr: *mut u8, sample_count: usize) {
	let slice = unsafe { slice::from_raw_parts_mut(ptr.cast::<i32>(), sample_count) };
	for sample in slice {
		*sample <<= 8;
	}
}

#[expect(clippy::cast_ptr_alignment, reason = "guaranteed by WASAPI")]
pub fn shift_down(ptr: *mut u8, sample_count: usize, scratch: &mut Box<[u8]>) {
	let buffer_slice = unsafe { slice::from_raw_parts(ptr.cast::<i32>(), sample_count) };
	let scratch_slice = unsafe { slice::from_raw_parts_mut(scratch.as_mut_ptr().cast::<i32>(), sample_count) };
	
	scratch_slice.copy_from_slice(buffer_slice);

	for sample in scratch_slice {
		*sample >>= 8;
	}
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamState {
	Running,
	Draining,
	Halted,
	Exit
}

#[derive(Debug)]
pub struct WaitableStreamState {
	atomic: AtomicU8,
	event: Event
}

impl WaitableStreamState {
	pub fn new() -> WinResult<Self> {
		Self {
			atomic: AtomicU8::new(StreamState::Halted as _),
			event: Event::new()?
		}.pipe(Ok)
	}
	
	pub fn get(&self) -> StreamState {
		unsafe { transmute(self.atomic.load(Ordering::Relaxed)) }
	}
	
	pub fn set(&self, value: StreamState) -> WinResult<()> {
		self.atomic.store(value as _, Ordering::Relaxed);
		self.event.set()
	}
	
	#[must_use]
	pub const fn event(&self) -> &Event {
		&self.event
	}
}