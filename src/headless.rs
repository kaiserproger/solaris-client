//! Protocol/state-only owner without a window, GPU, texture/font or audio resources.
use super::{App, Options};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) fn run(options: Options) -> Result<(), String> {
    let owner = std::thread::current();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || owner.unpark());
    let mut app = App::new(options, wake.clone())?;
    // Automation focus is local authorization, not a fabricated native window.
    app.focused = true;
    app.publish_session();
    app.start_automation()?;
    let result = (|| {
        while !app.shutdown {
            app.advance();
            let now = Instant::now();
            app.next_frame = now + Duration::from_millis(50);
            // unpark's retained token closes the event-before-park race. Deadline
            // wakes are actual client/frame clocks and bounded task timeouts.
            std::thread::park_timeout(app.deadline().saturating_duration_since(Instant::now()));
        }
        Ok(())
    })();
    app.release("headless_exit");
    app.session.disconnect();
    app.automation.take();
    result
}
