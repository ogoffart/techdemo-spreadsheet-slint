use std::time::Duration;

pub(crate) struct Debouncer {
    timeout: Option<slint::Timer>,
}

impl Debouncer {
    pub(crate) fn new() -> Self {
        Self { timeout: None }
    }

    pub(crate) fn debounce<F>(&mut self, delay: Duration, callback: F)
    where
        F: 'static + FnMut(),
    {
        if let Some(timeout) = self.timeout.take() {
            timeout.stop();
        }

        let timeout = slint::Timer::default();
        timeout.start(slint::TimerMode::SingleShot, delay, callback);
        self.timeout = Some(timeout);
    }
}
