use parking_lot::Mutex;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};

type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, PartialEq, Eq)]
enum SubmissionAdmission {
    Queued,
    Coalesced,
}

fn classify_submission<T>(
    result: Result<(), mpsc::TrySendError<T>>,
) -> Result<SubmissionAdmission, &'static str> {
    match result {
        Ok(()) => Ok(SubmissionAdmission::Queued),
        Err(mpsc::TrySendError::Full(_)) => Ok(SubmissionAdmission::Coalesced),
        Err(mpsc::TrySendError::Disconnected(_)) => Err("gpu_completion_worker_closed"),
    }
}

fn notify_wake(wake: &Mutex<Option<Wake>>) {
    if let Some(wake) = wake.lock().clone() {
        wake();
    }
}

/// therefore need a completion driver, not a repeating frame or polling timer.
/// This worker waits for one exact submitted fence, never device-wide idle on
/// the render/session owner. Full notifications coalesce: a later admitted
/// fence completes all earlier submissions as well.
pub(super) struct Progress {
    send: Option<mpsc::SyncSender<wgpu::SubmissionIndex>>,
    thread: Option<thread::JoinHandle<()>>,
    wake: Arc<Mutex<Option<Wake>>>,
    stop: Arc<AtomicBool>,
}
impl Progress {
    pub fn new(
        device: wgpu::Device,
        errors: mpsc::SyncSender<String>,
        failed: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let (send, submissions) = mpsc::sync_channel::<wgpu::SubmissionIndex>(3);
        let wake = Arc::new(Mutex::new(None::<Wake>));
        let worker_wake = wake.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let thread = thread::Builder::new()
            .name("gpu-completion".into())
            .spawn(move || {
                while let Ok(submission) = submissions.recv() {
                    while !worker_stop.load(Ordering::Acquire) {
                        match device.poll(wgpu::PollType::Wait {
                            submission_index: Some(submission.clone()),
                            timeout: Some(Duration::from_secs(10)),
                        }) {
                            Ok(_) => break,
                            Err(wgpu::PollError::Timeout) => notify_wake(&worker_wake),
                            Err(error) => {
                                failed.store(true, Ordering::Release);
                                let _ = errors
                                    .try_send(format!("gpu_submission_completion_failed: {error}"));
                                notify_wake(&worker_wake);
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(|error| format!("gpu_completion_worker_start_failed: {error}"))?;
        Ok(Self {
            send: Some(send),
            thread: Some(thread),
            wake,
            stop,
        })
    }
    pub fn set_wake(&self, wake: Wake) {
        *self.wake.lock() = Some(wake);
    }
    pub fn submitted(&self, submission: wgpu::SubmissionIndex) -> Result<(), String> {
        classify_submission(self.send.as_ref().unwrap().try_send(submission))
            .map(|_| ())
            .map_err(str::to_owned)
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.send.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_completion_notification_is_coalesced_but_closed_worker_is_fatal() {
        let (send, receive) = mpsc::sync_channel::<()>(1);
        send.try_send(()).unwrap();
        assert_eq!(
            classify_submission(send.try_send(())),
            Ok(SubmissionAdmission::Coalesced)
        );
        drop(receive);
        let (send, receive) = mpsc::sync_channel::<()>(1);
        drop(receive);
        assert_eq!(
            classify_submission(send.try_send(())),
            Err("gpu_completion_worker_closed")
        );
    }
}
