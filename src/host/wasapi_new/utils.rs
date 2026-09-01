use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use std::{mem, ptr, slice};
use std::sync::mpsc;
use tap::Pipe;
use windows::Win32::Foundation::{E_POINTER, HANDLE, PROPERTYKEY, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
use windows_core::{HRESULT, HSTRING, IUnknown, Interface, implement};

pub type WinResult<T> = windows::core::Result<T>;

pub fn shift_up(ptr: *mut u8, sample_count: u32) {
	let slice = unsafe { slice::from_raw_parts_mut(ptr.cast::<i32>(), sample_count as _) };
	for sample in slice {
		*sample <<= 8;
	}
}

fn shift_down() {
	
}

pub fn to_aqs(PROPERTYKEY { fmtid, pid }: &PROPERTYKEY) -> HSTRING {
	format!("{{{fmtid:?}}} {pid}").into()
}

pub enum Command {
	Play,
	Pause,
	Stop,
}

pub fn activate_audio_interface_sync<T: Interface>(path: &HSTRING) -> WinResult<T> {
	let (sender, receiver) = mpsc::channel();
	let completion_handler: IActivateAudioInterfaceCompletionHandler = ActivateAudioInterfaceCompletionHandler(sender).into();
	
	let iid = T::IID;
	unsafe { ActivateAudioInterfaceAsync(path, &raw const iid, None, &completion_handler)? };
	
	receiver
		.recv()
		.expect("system should not close the channel")?
		.cast::<T>()
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

#[derive(Debug, Clone)]
pub struct Client(IAudioClient3);
#[derive(Debug, Clone)]
pub struct Capturer(IAudioCaptureClient);
#[derive(Debug, Clone)]
pub struct Renderer(IAudioRenderClient);
#[derive(Debug, Clone)]
pub struct Event(HANDLE);

// SAFETY:
// WASAPI interfaces implement `IAgileObject`, which marks them as
// agile across COM apartment boundaries. This implies thread safety.
unsafe impl Send for Client {}
unsafe impl Sync for Client {}
unsafe impl Send for Capturer {}
unsafe impl Sync for Capturer {}
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Client {
	pub fn activate(path: &HSTRING) -> WinResult<Self> {
		let (sender, receiver) = mpsc::channel();
		let completion_handler: IActivateAudioInterfaceCompletionHandler = ActivateAudioInterfaceCompletionHandler(sender).into();
		
		let iid = IAudioClient3::IID;
		unsafe { ActivateAudioInterfaceAsync(path, &raw const iid, None, &completion_handler)? };
		
		receiver
			.recv()
			.expect("[BUG!]system should not close the channel")?
			.cast()
			.map(Self)
	}
	
	pub fn get_shared_mode_engine_period(&self, format: &WAVEFORMATEX) -> WinResult<SharedModeEnginePeriods> {
		let mut periods = SharedModeEnginePeriods::default();
		
		unsafe {
			self.0.GetSharedModeEnginePeriod(
				format,
				&raw mut periods.default,
				&raw mut periods.fundamental,
				&raw mut periods.min,
				&raw mut periods.max
			)?
		};
		
		Ok(periods)
	}
	
	pub fn get_current_shared_mode_engine_period(&self) -> WinResult<(*mut WAVEFORMATEX, u32)> {
		let mut format = ptr::null_mut();
		let mut period = 0;
		
		unsafe { self.0.GetCurrentSharedModeEnginePeriod(&raw mut format, &raw mut period)?; }
		
		Ok((format, period))
	}
	
	pub fn set_event_handle(&self) -> WinResult<Event> {
		unsafe {
			let event = Event::new()?;
			self.0.SetEventHandle(event.0)?;
			Ok(event)
		}
	}
	
	pub fn initialize_shared_audio_stream(&self, period: u32, format: *const WAVEFORMATEX) -> WinResult<()> {
		unsafe { self.0.InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, period, format, None) }
	}
	
	pub fn get_buffer_size(&self) -> WinResult<u32> {
		unsafe { self.0.GetBufferSize() }
	}
	
	pub fn get_current_padding(&self) -> WinResult<u32> {
		unsafe { self.0.GetCurrentPadding() }
	}
	
	pub fn get_capture_client(&self) -> WinResult<Capturer> {
		unsafe { self.0.GetService() }
    		.map(Capturer)
	}
	
	pub fn get_render_client(&self) -> WinResult<Renderer> {
		unsafe { self.0.GetService() }
    		.map(Renderer)
	}
	
	pub fn start(&self) -> WinResult<()> {
		unsafe { self.0.Start() }
	}
	
	pub fn stop(&self) -> WinResult<()> {
		unsafe { self.0.Stop() }
	}
	
	pub fn reset(&self) -> WinResult<()> {
		unsafe { self.0.Reset() }
	}
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct SharedModeEnginePeriods {
	pub default: u32,
	pub fundamental: u32,
	pub min: u32,
	pub max: u32
}

impl Capturer {
	pub fn with_buffer(&self, func: impl FnOnce(*mut u8, u32, _AUDCLNT_BUFFERFLAGS, u64, u64)) -> WinResult<()> {
		let mut ptr = ptr::null_mut();
		let mut n_frames = 0;
		let mut flags = 0;
		let mut dev_pos = 0;
		let mut qpc_pos = 0;
		
		unsafe {
			self.0.GetBuffer(
				&raw mut ptr,
				&raw mut n_frames,
				&raw mut flags,
				Some(&raw mut dev_pos),
				Some(&raw mut qpc_pos)
			)?;
		}

		func(ptr, n_frames, _AUDCLNT_BUFFERFLAGS(flags as _), dev_pos, qpc_pos);
		
		unsafe {
			self.0.ReleaseBuffer(n_frames)?;
		}
		
		Ok(())
	}
}

struct CapturePacket {
	ptr: *mut u8,
	n_frames: u32,
	flags: u32,
	device_position: u64,
	qpc_position: u64
}

impl Renderer {
	pub fn with_buffer(&self, frame_count: u32, func: impl FnOnce(*mut u8)) -> WinResult<()> {
		println!("{frame_count}");
		if frame_count == 0 {
			return Ok(())
		}
		
		let ptr = unsafe { self.0.GetBuffer(frame_count)? };
		
		func(ptr);
		
		unsafe { self.0.ReleaseBuffer(frame_count, 0) }
	}
}

impl Event {
	pub fn new() -> WinResult<Self> {
		unsafe { CreateEventW(None, false, true, None) }
			.map(Self)
	}
	
	pub fn wait(&self, timeout: Option<Duration>) -> WinResult<bool> {
		let ms = timeout.map_or(INFINITE, |duration| duration.as_millis() as _);
		match unsafe { WaitForSingleObject(self.0, ms) } {
			WAIT_OBJECT_0 => Ok(true),
			WAIT_TIMEOUT => Ok(false),
			WAIT_FAILED => Err(todo!()),
			_ => unreachable!()
		}
	}
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamState {
	Running,
	Draining,
	Halted
}

pub struct SignaledStreamState {
	inner: AtomicU8,
	event: Event,
}

impl SignaledStreamState {
	pub fn new() -> WinResult<Self> {
		Self {
			inner: AtomicU8::new(StreamState::Halted as _),
			event: Event::new()?
		}.pipe(Ok)
	}
	
	pub fn get(&self) -> StreamState {
		unsafe { mem::transmute(self.inner.load(Ordering::Relaxed)) }
	}
	
	pub fn set(&self, value: StreamState) -> WinResult<()> {
		self.inner.store(value as _, Ordering::Relaxed);
		unsafe { SetEvent(self.event.0) }
	}
	
	pub fn wait_value(&self, value: StreamState, timeout: Option<Duration>) -> WinResult<bool> {
		while self.get() != value {
			if !self.event.wait(timeout)? {
				return Ok(false)
			}
		}

		Ok(true)
	}
}