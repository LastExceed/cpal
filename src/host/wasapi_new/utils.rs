use std::mem::transmute;
use std::ops::Deref;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::slice;
use tap::Pipe;
use wasapi::{Direction, WaveFormat};
use windows::Win32::Foundation::{E_POINTER, PROPERTYKEY};
use windows::Win32::Media::Audio::*;
use windows_core::{HRESULT, HSTRING, IUnknown, implement};

use crate::{DeviceDirection, SupportedStreamConfigRange};

pub type WinResult<T> = windows_core::Result<T>;

#[expect(clippy::cast_ptr_alignment, reason = "guaranteed by WASAPI")]
pub fn shift_up(ptr: *mut u8, sample_count: usize) {
	let slice = unsafe { slice::from_raw_parts_mut(ptr.cast::<i32>(), sample_count) };
	for sample in slice {
		*sample <<= 8;
	}
}

#[expect(clippy::cast_ptr_alignment, reason = "guaranteed by WASAPI")]
pub fn shift_down(ptr: *mut u8, sample_count: usize) {
	let slice = unsafe { slice::from_raw_parts_mut(ptr.cast::<i32>(), sample_count) };
	for sample in slice {
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

impl From<Direction> for DeviceDirection {
	fn from(value: Direction) -> Self {
		match value {
			Direction::Capture => DeviceDirection::Input,
			Direction::Render  => DeviceDirection::Output,
		}
	}
}

impl From<WaveFormat> for SupportedStreamConfigRange {
	fn from(value: WaveFormat) -> Self {
		todo!()
	}
}

pub fn get_waveformat() -> WaveFormat {
	todo!()
}

pub struct SendSync<T>(pub T);

unsafe impl<T> Send for SendSync<T> {}
unsafe impl<T> Sync for SendSync<T> {}

impl<T> Deref for SendSync<T> {
	type Target = T;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivateAudioInterfaceCompletionHandler(mpsc::Sender<WinResult<IUnknown>>);

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivateAudioInterfaceCompletionHandler_Impl {
    fn ActivateCompleted(&self, activate_operation: windows_core::Ref<'_, IActivateAudioInterfaceAsyncOperation>) -> windows_core::Result<()> {
        let e_pointer1 = windows_core::Error::from(E_POINTER);
		let e_pointer2 = e_pointer1.clone();
		let e_pointer3 = e_pointer1.clone();
		
		let op = activate_operation
			.as_ref()
			.ok_or(e_pointer1)?;
		
		let mut hresult = HRESULT::default();
        let mut out = None;
		
		unsafe { op.GetActivateResult(&raw mut hresult, &raw mut out) }?;
    
		let result = hresult.ok().and(out.ok_or(e_pointer2));

		self.0
			.send(result)
			.map_err(|_| e_pointer3)?;

        Ok(())
    }
}

pub trait BufferFlagsExt {
	fn data_discontinuity(&self) -> bool;
	fn silent(&self) -> bool;
	fn timestamp_error(&self) -> bool;
}

impl BufferFlagsExt for _AUDCLNT_BUFFERFLAGS {
	fn data_discontinuity(&self) -> bool {
		self.0 & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 != 0
	}
	
	fn silent(&self) -> bool {
		self.0 & AUDCLNT_BUFFERFLAGS_SILENT.0 != 0
	}
	
	fn timestamp_error(&self) -> bool {
		self.0 & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 != 0
	}
}