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