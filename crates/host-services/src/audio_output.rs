//! Each mixer owns its own WinMM handle and buffers. No process-global sound,
//! device-volume operation, callback pointer or borrowed buffer crosses owners.
use crate::audio::Mixer;
use std::sync::{Arc, Mutex, atomic::AtomicBool};

#[cfg(windows)]
pub(crate) use native::AudioOutput;
#[cfg(not(windows))]
pub(crate) struct AudioOutput;
#[cfg(not(windows))]
impl AudioOutput {
    pub(crate) fn start(_: Arc<Mutex<Mixer>>, _: Arc<AtomicBool>) -> Result<Self, String> {
        Err("Native WAV playback is not implemented on this platform".into())
    }
    pub(crate) fn stop(self) {}
    pub(crate) fn wake(&self) {}
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::audio::OUTPUT_RATE;
    use std::{
        ffi::c_void,
        mem, ptr,
        sync::atomic::Ordering,
        thread::{self, JoinHandle},
        time::Duration,
    };
    const DONE: u32 = 1;
    #[repr(C, packed(1))]
    struct WaveFormat {
        tag: u16,
        channels: u16,
        rate: u32,
        bytes_per_second: u32,
        alignment: u16,
        bits: u16,
        extra: u16,
    }
    #[repr(C)]
    struct WaveHeader {
        data: *mut c_void,
        length: u32,
        recorded: u32,
        user: usize,
        flags: u32,
        loops: u32,
        next: *mut WaveHeader,
        reserved: usize,
    }
    struct Buffer {
        header: WaveHeader,
        samples: Vec<i16>,
        prepared: bool,
        queued: bool,
    }
    impl Buffer {
        fn new() -> Box<Self> {
            let samples = vec![0_i16; OUTPUT_RATE as usize / 100 * 2];
            let mut buffer = Box::new(Self {
                header: WaveHeader {
                    data: ptr::null_mut(),
                    length: (samples.len() * 2) as u32,
                    recorded: 0,
                    user: 0,
                    flags: 0,
                    loops: 0,
                    next: ptr::null_mut(),
                    reserved: 0,
                },
                samples,
                prepared: false,
                queued: false,
            });
            buffer.header.data = buffer.samples.as_mut_ptr().cast();
            buffer
        }
    }
    struct Device {
        handle: usize,
    }
    impl Drop for Device {
        fn drop(&mut self) {
            if self.handle != 0 {
                unsafe {
                    waveOutClose(self.handle);
                }
            }
        }
    }
    impl Device {
        fn open() -> Result<Self, String> {
            let format = WaveFormat {
                tag: 1,
                channels: 2,
                rate: OUTPUT_RATE,
                bytes_per_second: OUTPUT_RATE * 4,
                alignment: 4,
                bits: 16,
                extra: 0,
            };
            let mut handle = 0_usize;
            check(
                unsafe { waveOutOpen(&mut handle, u32::MAX, &format, 0, 0, 0) },
                "open",
            )?;
            Ok(Self { handle })
        }
        fn stream(
            mut self,
            shared: &Arc<Mutex<Mixer>>,
            retired: &AtomicBool,
            stopping: &AtomicBool,
        ) -> Result<(), String> {
            // Keep 40 ms queued: the native clock regression catches gaps that
            // a 30 ms queue can leave between WinMM completion/refill batches.
            let mut buffers = (0..4).map(|_| Buffer::new()).collect::<Vec<_>>();
            let result = (|| {
                for buffer in &mut buffers {
                    check(
                        unsafe {
                            waveOutPrepareHeader(
                                self.handle,
                                &mut buffer.header,
                                mem::size_of::<WaveHeader>() as u32,
                            )
                        },
                        "prepare buffer",
                    )?;
                    buffer.prepared = true;
                }
                while !stopping.load(Ordering::Acquire) && !retired.load(Ordering::Acquire) {
                    for buffer in &mut buffers {
                        if buffer.queued {
                            // WinMM owns flags while a header is submitted.
                            if unsafe { ptr::read_volatile(ptr::addr_of!(buffer.header.flags)) }
                                & DONE
                                == 0
                            {
                                continue;
                            }
                            buffer.queued = false;
                            let mut mixer = shared
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            mixer.completed = mixer.completed.saturating_add(1);
                        }
                        let active = shared
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .mix(&mut buffer.samples);
                        if active
                            && !stopping.load(Ordering::Acquire)
                            && !retired.load(Ordering::Acquire)
                        {
                            check(
                                unsafe {
                                    waveOutWrite(
                                        self.handle,
                                        &mut buffer.header,
                                        mem::size_of::<WaveHeader>() as u32,
                                    )
                                },
                                "submit buffer",
                            )?;
                            buffer.queued = true;
                            let mut mixer = shared
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            mixer.submitted = mixer.submitted.saturating_add(1);
                        }
                    }
                    if buffers.iter().any(|buffer| buffer.queued) {
                        thread::sleep(Duration::from_millis(2));
                    } else {
                        thread::park_timeout(Duration::from_millis(50));
                    }
                }
                Ok(())
            })();
            // Reset returns every queued buffer before its allocation can die.
            // On an unconfirmed driver failure retain the native owner and pins.
            if let Err(error) = check(unsafe { waveOutReset(self.handle) }, "reset") {
                mem::forget(buffers);
                self.handle = 0;
                return Err(format!("{error}; native audio resources retained"));
            }
            let mut cleanup_error = None;
            for mut buffer in buffers {
                if buffer.prepared
                    && let Err(error) = check(
                        unsafe {
                            waveOutUnprepareHeader(
                                self.handle,
                                &mut buffer.header,
                                mem::size_of::<WaveHeader>() as u32,
                            )
                        },
                        "release buffer",
                    )
                {
                    cleanup_error = Some(error);
                    mem::forget(buffer);
                }
            }
            if let Some(error) = cleanup_error {
                self.handle = 0;
                return Err(format!("{error}; native audio resources retained"));
            }
            check(unsafe { waveOutClose(self.handle) }, "close")?;
            self.handle = 0;
            result
        }
    }
    pub(crate) struct AudioOutput {
        stopping: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
        shared: Arc<Mutex<Mixer>>,
    }
    impl AudioOutput {
        pub(crate) fn start(
            shared: Arc<Mutex<Mixer>>,
            retired: Arc<AtomicBool>,
        ) -> Result<Self, String> {
            let device = Device::open()?;
            let stopping = Arc::new(AtomicBool::new(false));
            let flag = stopping.clone();
            let worker_shared = shared.clone();
            let thread = thread::Builder::new()
                .name("ModuleEngine WAV mixer".into())
                .spawn(move || {
                    if let Err(error) = device.stream(&worker_shared, &retired, &flag) {
                        worker_shared
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .fault = Some(error);
                    }
                })
                .map_err(|error| format!("Cannot start audio mixer: {error}"))?;
            Ok(Self {
                stopping,
                thread: Some(thread),
                shared,
            })
        }
        fn finish(&mut self) {
            self.stopping.store(true, Ordering::Release);
            self.wake();
            if self
                .thread
                .take()
                .is_some_and(|thread| thread.join().is_err())
            {
                self.shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .fault = Some("Native audio mixer thread panicked".into());
            }
        }
        pub(crate) fn stop(mut self) {
            self.finish();
        }
        pub(crate) fn wake(&self) {
            if let Some(thread) = &self.thread {
                thread.thread().unpark();
            }
        }
    }
    impl Drop for AudioOutput {
        fn drop(&mut self) {
            self.finish();
        }
    }
    fn check(code: u32, operation: &str) -> Result<(), String> {
        if code == 0 {
            Ok(())
        } else {
            Err(format!(
                "WinMM could not {operation} for the owned WAV mixer (code {code})"
            ))
        }
    }
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn waveOutOpen(
            handle: *mut usize,
            device: u32,
            format: *const WaveFormat,
            callback: usize,
            instance: usize,
            flags: u32,
        ) -> u32;
        fn waveOutPrepareHeader(handle: usize, header: *mut WaveHeader, size: u32) -> u32;
        fn waveOutWrite(handle: usize, header: *mut WaveHeader, size: u32) -> u32;
        fn waveOutReset(handle: usize) -> u32;
        fn waveOutUnprepareHeader(handle: usize, header: *mut WaveHeader, size: u32) -> u32;
        fn waveOutClose(handle: usize) -> u32;
    }
}
