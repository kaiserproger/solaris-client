use parking_lot::Mutex;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

type Wake = Arc<dyn Fn() + Send + Sync>;

/// Wgpu invokes map/fence callbacks when polled. Event-only/headless consumers
/// therefore need a completion driver, not a repeating frame or polling timer.
/// This worker waits for one exact submitted fence, never device-wide idle on
/// the render/session owner, and its queue shares the three-frame bound.
pub(super) struct Progress {
    send: Option<mpsc::SyncSender<wgpu::SubmissionIndex>>,
    thread: Option<thread::JoinHandle<()>>,
    wake: Arc<Mutex<Option<Wake>>>,
}
impl Progress {
    pub fn new(
        device: wgpu::Device,
        errors: mpsc::SyncSender<String>,
        failed: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let (send, submissions) = mpsc::sync_channel(3);
        let wake = Arc::new(Mutex::new(None::<Wake>));
        let worker_wake = wake.clone();
        let thread = thread::Builder::new()
            .name("gpu-completion".into())
            .spawn(move || {
                while let Ok(submission) = submissions.recv() {
                    if let Err(error) = device.poll(wgpu::PollType::Wait {
                        submission_index: Some(submission),
                        timeout: Some(Duration::from_secs(10)),
                    }) {
                        failed.store(true, Ordering::Release);
                        let _ =
                            errors.try_send(format!("gpu_submission_completion_failed: {error}"));
                        let wake = worker_wake.lock().clone();
                        if let Some(wake) = wake {
                            wake();
                        }
                    }
                }
            })
            .map_err(|error| format!("gpu_completion_worker_start_failed: {error}"))?;
        Ok(Self {
            send: Some(send),
            thread: Some(thread),
            wake,
        })
    }
    pub fn set_wake(&self, wake: Wake) {
        *self.wake.lock() = Some(wake);
    }
    pub fn submitted(&self, submission: wgpu::SubmissionIndex) -> Result<(), String> {
        self.send
            .as_ref()
            .unwrap()
            .try_send(submission)
            .map_err(|_| "gpu_completion_queue_bound_or_worker_closed".into())
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        self.send.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
