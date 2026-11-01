use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::error::Error;
use std::fmt::Debug;
use std::mem::transmute;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU8, Ordering};
use std::slice;
use std::time::Duration;
use tap::Pipe;
use wasapi::{Direction, WasapiError, WaveFormat};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_EVENT, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject};

use crate::ErrorKind::BackendError;
use crate::{DeviceDirection, SampleFormat, SupportedStreamConfigRange};

pub type WinResult<T> = windows_core::Result<T>;

pub fn convert_error(error: impl Error) -> crate::Error {
	crate::Error::with_message(BackendError, format!("{error:?}"))
}

pub struct ScratchBuffer {
	pub ptr: *mut u8,
	pub layout: Layout
}

unsafe impl Send for ScratchBuffer {}
unsafe impl Sync for ScratchBuffer {}

impl ScratchBuffer {
	pub fn new(sample_count: usize, format: SampleFormat) -> Self {
		let size   = sample_count * format.sample_size();
		let align  = format.sample_size();
		let layout = Layout::from_size_align(size, align).unwrap();
		let ptr    = unsafe { alloc_zeroed(layout) };
		
		Self { ptr, layout }
	}
	
	pub fn slice(&mut self) -> &mut [u8] {
		unsafe {
			slice::from_raw_parts_mut(
				self.ptr,
				self.layout.size()
			)
		}
	}
	
	fn slice_i32(&mut self) -> &mut [i32] {
		unsafe {
			slice::from_raw_parts_mut(
				self.ptr.cast(),
				self.layout.size() / size_of::<i32>()
			)
		}
	}
	
	#[expect(clippy::cast_ptr_alignment, reason = "guaranteed by WASAPI")]
	pub fn shift_up(&mut self, sample_count: usize) {
		for sample in &mut self.slice_i32()[..sample_count] {
			*sample <<= 8;
		}
	}

	#[expect(clippy::cast_ptr_alignment, reason = "guaranteed by WASAPI")]
	pub fn shift_down(&mut self, sample_count: usize) {
		for sample in &mut self.slice_i32()[..sample_count] {
			*sample >>= 8;
		}
	}
}

impl Drop for ScratchBuffer {
	fn drop(&mut self) {
		unsafe { dealloc(self.ptr, self.layout) };
	}
}

#[derive(Debug)]
pub struct Event(HANDLE);

unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Event {
	pub fn new() -> WinResult<Self> {
		unsafe { CreateEventW(None, false, true, None) }
			.map(Self)
	}
	
	pub const fn as_raw(&self) -> &HANDLE {
		&self.0
	}
	
	pub fn set_signaled(&self) -> WinResult<()> {
		unsafe { SetEvent(self.0) }
	}
	
	pub fn wait_single(&self, timeout: Option<Duration>) -> WinResult<bool> {
		let millis = timeout.map_or(INFINITE, |duration| duration.as_millis() as _);

		match unsafe { WaitForSingleObject(self.0, millis) } {
			WAIT_OBJECT_0 => Ok(true),
			WAIT_TIMEOUT => Ok(false),
			WAIT_FAILED => Err(windows_core::Error::from_thread()),
			_          => unreachable!()
		}
	}
	
	pub fn wait_multiple(events: &[HANDLE], timeout: Option<Duration>) -> WinResult<Option<usize>> {
		let dwmilliseconds = timeout.map_or(INFINITE, |duration| duration.as_millis() as _);
		let wait_event = unsafe { WaitForMultipleObjects(events, false, dwmilliseconds) };
		
		#[deny(nonstandard_style, reason = "prevent accidental wildcards")]
		match wait_event {
			WAIT_FAILED => Err(windows_core::Error::from_thread()),
			WAIT_TIMEOUT => Ok(None),
			WAIT_EVENT(i) => Ok(Some(i as _))
		}
	}
}

impl Drop for Event {
	fn drop(&mut self) {
		_ = unsafe { CloseHandle(self.0) };
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
	
	pub fn set(&self, value: StreamState, signal: bool) -> WinResult<()> {
		self.atomic.store(value as _, Ordering::Relaxed);
		if signal {
			self.event.set_signaled()?;
		}
		Ok(())
	}
	
	#[must_use]
	pub const fn event(&self) -> &Event {
		&self.event
	}
}

impl From<WasapiError> for crate::Error {
	fn from(value: WasapiError) -> Self {
		todo!()
	}
}

impl From<Direction> for DeviceDirection {
	fn from(value: Direction) -> Self {
		match value {
			Direction::Capture => DeviceDirection::Input,
			Direction::Render  => DeviceDirection::Output,
		}
	}
}

impl From<WaveFormat> for SupportedStreamConfigRange {
	fn from(_value: WaveFormat) -> Self {
		todo!()
	}
}

pub fn get_waveformat() -> WaveFormat {
	todo!()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SendSync<T>(pub T);

unsafe impl<T> Send for SendSync<T> {}
unsafe impl<T> Sync for SendSync<T> {}

impl<T> Deref for SendSync<T> {
	type Target = T;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl<T> DerefMut for SendSync<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.0
	}
}