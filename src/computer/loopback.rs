//! Windows 10 2004+ WASAPI process loopback: the audio rendered by one process
//! tree (the shared window's process and its children), never the system mix, so
//! Solaris's own playback and voice chat are not captured. Not executed on a
//! Windows host yet: it is compile-checked only (`x86_64-pc-windows-gnu`).
use super::audio::{AudioSource, FloatBytes, Pcm, RATE};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        Media::Audio::{
            AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
            AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS,
            AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
            IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            WAVEFORMATEX,
        },
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
            Threading::{CreateEventW, SetEvent, WaitForSingleObject},
            Variant::VT_BLOB,
        },
    },
    core::{HRESULT, Interface, PROPVARIANT, Result as WinResult, implement},
};

const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Completed(isize);
impl IActivateAudioInterfaceCompletionHandler_Impl for Completed_Impl {
    fn ActivateCompleted(&self, _: Option<&IActivateAudioInterfaceAsyncOperation>) -> WinResult<()> {
        unsafe { SetEvent(HANDLE(self.0 as *mut _)) }
    }
}

/// Audio of `pid` and its child processes as 48 kHz mono `f32`.
pub(crate) fn open(pid: u32) -> Result<Box<dyn AudioSource>, String> {
    if pid == 0 || pid == std::process::id() {
        return Err("Refusing to capture Solaris's own audio".into());
    }
    let pcm = Pcm::default();
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready) = mpsc::channel();
    let (output, halt) = (pcm.clone(), stop.clone());
    let thread = std::thread::Builder::new()
        .name("process-loopback".into())
        .spawn(move || {
            // SAFETY: COM is initialised and released on this thread only.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            if let Err(error) = capture(pid, &output, &halt, &ready_tx) {
                let _ = ready_tx.send(Err(error.clone()));
                output.fail(error);
            }
            unsafe { CoUninitialize() };
        })
        .map_err(|e| e.to_string())?;
    match ready.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(())) => Ok(Box::new(Loopback { pcm, stop, thread: Some(thread) })),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            stop.store(true, Ordering::Release);
            Err("Process loopback activation timed out".into())
        }
    }
}

fn win(context: &'static str) -> impl Fn(windows::core::Error) -> String {
    move |error| format!("{context}: {error}")
}

fn capture(
    pid: u32,
    pcm: &Pcm,
    stop: &AtomicBool,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    // SAFETY: plain WASAPI calls; every pointer outlives the call that uses it.
    unsafe {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let mut raw = *PROPVARIANT::new().as_raw();
        {
            let inner = &mut raw.Anonymous.Anonymous;
            inner.vt = VT_BLOB.0;
            inner.Anonymous.blob.cbSize =
                std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
            inner.Anonymous.blob.pBlobData =
                (&mut params as *mut AUDIOCLIENT_ACTIVATION_PARAMS).cast();
        }
        // The blob borrows `params` through activation completion. Do not let
        // PropVariantClear free this stack allocation as an owned COM blob.
        let variant = std::mem::ManuallyDrop::new(PROPVARIANT::from_raw(raw));
        let done = CreateEventW(None, false, false, None).map_err(win("CreateEvent"))?;
        let handler: IActivateAudioInterfaceCompletionHandler = Completed(done.0 as isize).into();
        let operation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&variant),
            &handler,
        )
        .map_err(win("ActivateAudioInterfaceAsync"));
        let activated = operation.and_then(|operation| {
            if WaitForSingleObject(done, 5000) != WAIT_OBJECT_0 {
                return Err("Process loopback activation timed out".to_owned());
            }
            let mut result = HRESULT(0);
            let mut interface = None;
            operation
                .GetActivateResult(&mut result, &mut interface)
                .map_err(win("GetActivateResult"))?;
            result.ok().map_err(win("Process loopback activation"))?;
            interface
                .ok_or_else(|| "Process loopback returned no audio client".to_owned())?
                .cast::<IAudioClient>()
                .map_err(win("IAudioClient"))
        });
        let _ = CloseHandle(done);
        let client = activated?;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 1,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 4,
            nBlockAlign: 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                2_000_000,
                0,
                &format,
                None,
            )
            .map_err(win("IAudioClient::Initialize"))?;
        let event = CreateEventW(None, false, false, None).map_err(win("CreateEvent"))?;
        let result = (|| {
            client.SetEventHandle(event).map_err(win("SetEventHandle"))?;
            let reader: IAudioCaptureClient = client.GetService().map_err(win("IAudioCaptureClient"))?;
            client.Start().map_err(win("IAudioClient::Start"))?;
            let _ = ready.send(Ok(()));
            let mut bytes = FloatBytes::default();
            while !stop.load(Ordering::Acquire) {
                // The event is signalled per period; the timeout only bounds stop latency.
                WaitForSingleObject(event, 100);
                loop {
                    let packet = reader.GetNextPacketSize().map_err(win("GetNextPacketSize"))?;
                    if packet == 0 {
                        break;
                    }
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                    reader
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .map_err(win("GetBuffer"))?;
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                        pcm.push(std::iter::repeat_n(0., frames as usize));
                    } else {
                        bytes.feed(std::slice::from_raw_parts(data, frames as usize * 4), pcm);
                    }
                    reader.ReleaseBuffer(frames).map_err(win("ReleaseBuffer"))?;
                }
            }
            let _ = client.Stop();
            Ok(())
        })();
        let _ = CloseHandle(event);
        result
    }
}

struct Loopback {
    pcm: Pcm,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl AudioSource for Loopback {
    fn drain(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        self.pcm.drain(out)
    }
    /// The process keeps playing on the owner's own output.
    fn replaces_app_output(&self) -> bool {
        false
    }
    fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Loopback {
    fn drop(&mut self) {
        self.close();
    }
}
