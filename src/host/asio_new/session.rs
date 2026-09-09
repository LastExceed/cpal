use crate::ErrorKind::*;
use crate::host::com;
use crate::*;
use azo::dto::ChannelCounts;
use azo::{Driver, WinResult};
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::vec;
use tap::prelude::*;
use windows_core::{GUID, Interface};

use super::callbacks::Callbacks;
use super::utils::{CpalResult, DoubleBuffer, create_report, err};
use super::{SupportedConfigs, capabilities, simplex};

#[derive(Debug)]
pub struct Factory {
    com_worker: com::worker::Handle,
    cache: Mutex<HashMap<GUID, Weak<Session>>>,
}

impl Factory {
    pub fn new() -> Self {
        Self {
            com_worker: com::worker::Handle::new(),
            cache: Mutex::default(),
        }
    }

    #[expect(clippy::unwrap_in_result, reason = "should be infallible")]
    pub fn get_session(&self, clsid: &GUID) -> WinResult<Arc<Session>> {
        let mut guard = self.cache.lock().expect("Mutex poisoned");

        if let Some(existing) = guard.get(clsid).and_then(Weak::upgrade) {
            return Ok(existing);
        }

        let new = Session::new(*clsid, &self.com_worker)?.pipe(Arc::new);
        guard.insert(*clsid, Arc::downgrade(&new));

        Ok(new)
    }
}

#[derive(Debug)]
pub struct Session {
    pub driver: Driver,
    init_success: bool,
    clsid_string: String,
    pub stream_exists: Mutex<bool>,
    _com_worker: com::worker::Handle,
}

impl Session {
    pub fn new(clsid: GUID, com_worker: &com::worker::Handle) -> WinResult<Self> {
        let driver = com_worker.create_driver(clsid)?;

        Self {
            init_success: driver.init(None),
            driver,
            clsid_string: format!("{clsid:?}"),
            stream_exists: Mutex::default(),
            _com_worker: com_worker.clone(), // hold on to this to keep the thread alive that initialized the COM apartment in which the driver was created
        }
        .pipe(Ok)
    }

    pub fn id(&self) -> CpalResult<DeviceId> {
        DeviceId::new(HostId::AsioNew, self.clsid_string.clone()).pipe(Ok)
    }

    pub fn description(&self) -> CpalResult<DeviceDescription> {
        let name_c = self.driver.name();
        let name = name_c.to_string_lossy();

        let direction = match capabilities::channel_counts(&self.driver)? {
            ChannelCounts { in_: 1.., out: 1.. } => DeviceDirection::Duplex,
            ChannelCounts { in_: 1.., out: 0 } => DeviceDirection::Input,
            ChannelCounts { in_: 0, out: 1.. } => DeviceDirection::Output,
            _ => DeviceDirection::Unknown,
        };

        let mut extended = vec![format!("driver version: {}", self.driver.version())];

        if !self.init_success {
            extended.push("ASIO driver failed to initialize".to_owned()); // ASIO drivers can often still do *something* when they fail to initialize
            extended.push(format!(
                "last error: {}",
                self.driver.last_error().to_string_lossy()
            ));
        }

        DeviceDescriptionBuilder::new(&name)
            .driver(name)
            .direction(direction)
            .extended(extended)
            .build()
            .pipe(Ok)
    }

    #[must_use]
    pub fn supports_direction<const IN: bool, const OUT: bool>(&self) -> bool {
        if !self.init_success {
            return false;
        }

        let Ok(counts) = self.driver.channel_counts() else {
            return false;
        }; // can't do anything if it can't even count the channels

        if IN && counts.in_ == 0 {
            return false;
        }

        if OUT && counts.out == 0 {
            return false;
        }

        true
    }

    pub fn supported_configs<const INPUT: bool>(&self) -> CpalResult<SupportedConfigs> {
        let ch_count = capabilities::channel_count::<INPUT>(&self.driver)?;
        if ch_count == 0 {
            return err(
                UnsupportedOperation,
                "the device has no channels in this direction",
            );
        }

        let (min_rate, max_rate) = capabilities::sample_rates(&self.driver)?;
        let buf_size = capabilities::buffer_size_supported(&self.driver);
        let sample_formats = capabilities::sample_formats::<INPUT>(&self.driver, ch_count)?;

        sample_formats
            .map(move |format| {
                SupportedStreamConfigRange::new(ch_count as _, min_rate, max_rate, buf_size, format)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .pipe(Ok)
    }

    pub fn default_config<const INPUT: bool>(&self) -> CpalResult<SupportedStreamConfig> {
        self.supported_configs::<INPUT>()?
            .next()
            .expect("infallible")
            .pipe(|range| {
                SupportedStreamConfig::new(
                    range.channels(),
                    range.min_sample_rate(),
                    *range.buffer_size(),
                    range.sample_format(),
                )
            })
            .pipe(Ok)
    }

    pub fn build_stream(
        self: &Arc<Self>,
        cfg_in: simplex::Config,
        cfg_out: simplex::Config,
        sample_rate: SampleRate,
        buffer_size: BufferSize,
        data_cb: data_cb_type!(),
        error_cb: error_cb_type!(),
    ) -> CpalResult<super::Stream> {
        let mut guard = self.stream_exists.lock().or(err(
            DeviceNotAvailable,
            "Mutex poisoned. This device is most likely dead",
        ))?;
        if *guard {
            return err(
                UnsupportedOperation,
                "ASIO only supports 1 stream per device",
            );
        }

        self.set_sample_rate(sample_rate)?;
        let frame_count = self.get_frame_count(buffer_size)?;
        let callbacks = self.prepare(cfg_in, cfg_out, frame_count, data_cb, error_cb)?;

        *guard = true;

        super::Stream {
            session: Arc::clone(self),
            frame_count,
            _callbacks: callbacks, // keep this alive until the stream is dropped
        }
        .pipe(Ok)
    }

    fn set_sample_rate(&self, sample_rate: SampleRate) -> CpalResult<()> {
        self.driver
            .can_sample_rate(sample_rate as _)
            .map_err(|_| Error::with_message(InvalidInput, "sample rate not supported"))?;

        self.driver
            .set_sample_rate(sample_rate as _)
            .map_err(|asio_error| create_report(&self.driver, asio_error, "set_sample_rate"))?;

        Ok(())
    }

    fn get_frame_count(&self, requested: BufferSize) -> CpalResult<FrameCount> {
        match requested {
            BufferSize::Fixed(n) => n,
            BufferSize::Default => capabilities::buffer_size_preferred(&self.driver)? as FrameCount,
        }
        .pipe(Ok)
    }

    /// ASIO lifecycle stage 2 ("initialized") -> stage 3 ("prepared")
    /// - See ASIO specification section II.2
    fn prepare(
        self: &Arc<Self>,
        cfg_in: simplex::Config,
        cfg_out: simplex::Config,
        frame_count: FrameCount,
        data_cb: data_cb_type!(),
        error_cb: error_cb_type!(),
    ) -> CpalResult<Pin<Box<Callbacks>>> {
        let channel_ids: Vec<_> = [cfg_in, cfg_out]
            .into_iter()
            .flat_map(|cfg| cfg.validate(&self.driver))
            .collect::<CpalResult<_>>()?;

        // FIXME: consider using `Pin::defaul()` once MSRV has risen to 1.91+
        let mut callbacks = Callbacks::default().pipe(Box::pin);

        // SAFETY:
        // `Callbacks` is pinned, and kept alive until after the buffers are disposed (see `Drop` implementation of `Stream`)
        let mut double_buffers = unsafe {
            self.driver
                .create_buffers(channel_ids, frame_count as _, callbacks.pointers())
        }
        .map_err(|error| create_report(&self.driver, error, "create_buffers"))?
        .map(DoubleBuffer);

        let buffers_in = double_buffers.by_ref().take(cfg_in.channels as _).collect();
        let buffers_out = double_buffers.collect();
        let simplex_in = simplex::WithScratch::new(cfg_in.format, frame_count, buffers_in);
        let simplex_out = simplex::WithScratch::new(cfg_out.format, frame_count, buffers_out);

        callbacks
            .as_mut()
            .prime(Arc::clone(self), data_cb, error_cb, simplex_in, simplex_out);

        Ok(callbacks)
    }
}

impl Hash for Session {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.driver.as_raw().as_raw().hash(state);
        self.init_success.hash(state);
        self.clsid_string.hash(state);
    }
}
