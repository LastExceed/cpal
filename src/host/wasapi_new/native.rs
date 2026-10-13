use std::time::Duration;
use std::ptr;
use std::sync::mpsc;
use tap::Pipe;
use windows::Win32::Foundation::{CloseHandle, E_POINTER, HANDLE, PROPERTYKEY, S_FALSE, S_OK, WAIT_EVENT, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject};
use windows_core::{HRESULT, HSTRING, IUnknown, Interface, implement};

use crate::SampleFormat;

use super::utils::WinResult;

pub fn to_aqs(PROPERTYKEY { fmtid, pid }: &PROPERTYKEY) -> HSTRING {
	format!("{{{fmtid:?}}} {pid}").into()
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

pub fn create_waveformatextensible(sample_rate: u32, channel_mask: u32, cpal_format: SampleFormat) -> WAVEFORMATEXTENSIBLE {
	let bytes_per_sample = cpal_format.sample_size() as u32;
	let n_channels = channel_mask.count_ones();
	
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag     : WAVE_FORMAT_EXTENSIBLE as _,
            nChannels      : n_channels as _,
            nSamplesPerSec : sample_rate, // struct property has confusing name
            nAvgBytesPerSec: sample_rate * n_channels * bytes_per_sample,
            nBlockAlign    : (n_channels * bytes_per_sample) as _,
            wBitsPerSample : bytes_per_sample as u16 * u8::BITS as u16,
            cbSize         : (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as _,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: cpal_format.bits_per_sample() as _
        },
        dwChannelMask: channel_mask,
        SubFormat: if cpal_format.is_float() { KSDATAFORMAT_SUBTYPE_IEEE_FLOAT } else { KSDATAFORMAT_SUBTYPE_PCM },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client(pub IAudioClient3);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capturer(IAudioCaptureClient);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renderer(IAudioRenderClient);
#[derive(Debug)]
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
	#[expect(clippy::unwrap_in_result, reason = "this would be an OS bug")]
	pub fn activate(path: &HSTRING) -> WinResult<Self> {
		let (sender, receiver) = mpsc::channel();
		let completion_handler: IActivateAudioInterfaceCompletionHandler = ActivateAudioInterfaceCompletionHandler(sender).into();
		
		let iid = IAudioClient3::IID;
		unsafe { ActivateAudioInterfaceAsync(path, &raw const iid, None, &completion_handler)?; }
		
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
			)?;
		};
		
		Ok(periods)
	}
	
	pub fn get_current_shared_mode_engine_period(&self) -> WinResult<(*mut WAVEFORMATEX, u32)> {
		let mut format = ptr::null_mut();
		let mut period = 0;
		
		unsafe { self.0.GetCurrentSharedModeEnginePeriod(&raw mut format, &raw mut period)?; }
		
		Ok((format, period))
	}
	
	pub fn is_format_supported(&self, ptr: *const WAVEFORMATEX) -> WinResult<Option<WaveFormatAny>> {
		let mut closest = ptr::null_mut();
		let hresult = unsafe { self.0.IsFormatSupported(AUDCLNT_SHAREMODE_SHARED, ptr, Some(&raw mut closest)) };

		#[deny(nonstandard_style, reason = "prevent accidental wildcard patterns")]
		match hresult {
			S_OK => ptr
				.pipe(WaveFormatAny::read_from)
				.pipe(Some)
				.pipe(Ok),
			
			S_FALSE => closest
    			.cast_const()
				.pipe(WaveFormatAny::read_from)
				.pipe(Some)
				.pipe(Ok),
			
			AUDCLNT_E_UNSUPPORTED_FORMAT => Ok(None),
			
			other => other
				.pipe(windows_core::Error::from_hresult)
				.pipe(Err)
		}
		
		// todo: memory leak!
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
	
	pub fn get_capturer(&self) -> WinResult<Capturer> {
		unsafe { self.0.GetService() }
    		.map(Capturer)
	}
	
	pub fn get_renderer(&self) -> WinResult<Renderer> {
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WaveFormatAny {
	// Short(WAVEFORMAT),
	Medium(WAVEFORMATEX),
	Long(WAVEFORMATEXTENSIBLE)
}

impl WaveFormatAny {
	pub fn read_from(ptr: *const WAVEFORMATEX) -> Self {
		let medium = unsafe { ptr.as_ref() }.unwrap();
		if medium.cbSize == 0 {
			return Self::Medium(*medium);
		}

		Self::Long(unsafe { ptr.cast::<WAVEFORMATEXTENSIBLE>().read() })
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
	pub fn with_buffer(&self, func: impl FnOnce(&CapturePacket)) -> WinResult<()> {
		let mut packet = CapturePacket::default();
		
		unsafe {
			self.0.GetBuffer(
				&raw mut packet.ptr,
				&raw mut packet.n_frames,
				(&raw mut packet.flags).cast(), // repr(transparent)
				Some(&raw mut packet.dev_pos),
				Some(&raw mut packet.qpc_pos)
			)?;
		}

		func(&packet);
		
		unsafe {
			self.0.ReleaseBuffer(packet.n_frames)?;
		}
		
		Ok(())
	}
}

#[derive(Debug, Default)]
pub struct CapturePacket {
	pub ptr     : *mut u8,
	pub n_frames: u32,
	pub flags   : _AUDCLNT_BUFFERFLAGS,
	pub dev_pos : u64,
	pub qpc_pos : u64
}

impl Renderer {
	pub fn with_buffer(&self, frame_count: u32, func: impl FnOnce(*mut u8)) -> WinResult<()> {
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
	
	pub const fn as_raw(&self) -> &HANDLE {
		&self.0
	}
	
	pub fn set(&self) -> WinResult<()> {
		unsafe { SetEvent(self.0) }
	}
	
	pub fn wait_single(&self, timeout: Option<Duration>) -> WinResult<bool> {
		let millis = timeout.map_or(INFINITE, |duration| duration.as_millis() as _);

		match unsafe { WaitForSingleObject(self.0, millis) } {
			WAIT_OBJECT_0 => Ok(true),
			WAIT_TIMEOUT => Ok(false),
			WAIT_FAILED => Err(windows_core::Error::from_thread()),
			_ => unreachable!()
		}
	}
	
	pub fn wait_multiple<const N: usize>(events: [&Self; N], timeout: Option<Duration>) -> WinResult<Option<usize>> {
		let wait_event = unsafe {
			WaitForMultipleObjects(
				&events.map(Self::as_raw).map(Clone::clone),
				false,
				timeout.map_or(INFINITE, |duration| duration.as_millis() as _)
			)
		};
		
		match wait_event {
			WAIT_FAILED   => Err(windows_core::Error::from_thread()),
			WAIT_TIMEOUT  => Ok(None),
			WAIT_EVENT(i) => Ok(Some(i as _))
		}
	}
}

impl Drop for Event {
	fn drop(&mut self) {
		_ = unsafe { CloseHandle(self.0) };
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