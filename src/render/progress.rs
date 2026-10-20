use parking_lot::{Condvar, Mutex};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};

type Wake = Arc<dyn Fn() + Send + Sync>;

/// Latest-wins hand-off between the render thread and the completion worker.
/// A newer submission replaces a still-pending older one instead of being
/// dropped: waiting for the newest fence implies every earlier one finished
/// (queue order), so nothing is lost and the final fence is always awaited.
struct LatestSlot<T> {
    state: Mutex<SlotState<T>>,
    ready: Condvar,
}

struct SlotState<T> {
    pending: Option<T>,
    closed: bool,
}

impl<T> LatestSlot<T> {
    fn new() -> Self {
        Self {
            state: Mutex::new(SlotState {
                pending: None,
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    /// Publish `value`, superseding any pending one. Errors once closed.
    fn put(&self, value: T) -> Result<(), &'static str> {
        let mut state = self.state.lock();
        if state.closed {
            return Err("gpu_completion_worker_closed");
        }
        state.pending = Some(value);
        self.ready.notify_one();
        Ok(())
    }

    /// Block for the pending value; `None` once closed.
    fn take(&self) -> Option<T> {
        let mut state = self.state.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(value) = state.pending.take() {
                return Some(value);
            }
            self.ready.wait(&mut state);
        }
    }

    fn close(&self) {
        let mut state = self.state.lock();
        state.closed = true;
        state.pending = None;
        self.ready.notify_all();
    }
}

/// Closes the slot when the worker ends for any reason (including a panic),
/// so a dead worker is reported by `put` instead of silently dropping fences.
struct CloseOnDrop<'a, T>(&'a LatestSlot<T>);
impl<T> Drop for CloseOnDrop<'_, T> {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn notify_wake(wake: &Mutex<Option<Wake>>) {
    if let Some(wake) = wake.lock().clone() {
        wake();
    }
}

/// therefore need a completion driver, not a repeating frame or polling timer.
/// This worker waits for one exact submitted fence, never device-wide idle on
/// the render/session owner. Pending fences coalesce latest-wins: the newest
/// fence is always retained and completes all earlier submissions as well.
pub(super) struct Progress {
    slot: Arc<LatestSlot<wgpu::SubmissionIndex>>,
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
        let slot = Arc::new(LatestSlot::<wgpu::SubmissionIndex>::new());
        let worker_slot = slot.clone();
        let wake = Arc::new(Mutex::new(None::<Wake>));
        let worker_wake = wake.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let thread = thread::Builder::new()
            .name("gpu-completion".into())
            .spawn(move || {
                let _closer = CloseOnDrop(&*worker_slot);
                while let Some(submission) = worker_slot.take() {
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
            slot,
            thread: Some(thread),
            wake,
            stop,
        })
    }
    pub fn set_wake(&self, wake: Wake) {
        *self.wake.lock() = Some(wake);
    }
    pub fn submitted(&self, submission: wgpu::SubmissionIndex) -> Result<(), String> {
        self.slot.put(submission).map_err(str::to_owned)
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.slot.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_pending_value_replaces_older_and_is_never_lost() {
        let slot = LatestSlot::new();
        for value in 0..100 {
            slot.put(value).unwrap();
        }
        assert_eq!(slot.take(), Some(99));
    }

    #[test]
    fn closed_slot_is_fatal_for_submitters_and_ends_the_worker() {
        let slot = LatestSlot::new();
        slot.put(1).unwrap();
        slot.close();
        assert_eq!(slot.put(2), Err("gpu_completion_worker_closed"));
        assert_eq!(slot.take(), None);
    }

    /// Saturate while the worker is stuck on the first fence, then stop
    /// submitting: the worker must still observe the final fence.
    #[test]
    fn final_fence_is_observed_after_saturation_without_further_submissions() {
        let slot = Arc::new(LatestSlot::new());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (seen_tx, seen_rx) = mpsc::channel();
        let worker_slot = slot.clone();
        let worker = thread::spawn(move || {
            let _closer = CloseOnDrop(&*worker_slot);
            while let Some(fence) = worker_slot.take() {
                if fence == 0 {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
                seen_tx.send(fence).unwrap();
            }
        });
        slot.put(0).unwrap();
        started_rx.recv().unwrap();
        for fence in 1..=50 {
            slot.put(fence).unwrap();
        }
        release_tx.send(()).unwrap();
        let mut last = seen_rx.recv().unwrap();
        while last != 50 {
            last = seen_rx.recv().unwrap();
        }
        assert_eq!(last, 50);
        slot.close();
        worker.join().unwrap();
    }
}
