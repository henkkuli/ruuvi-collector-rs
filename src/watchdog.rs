use tokio::{
    sync::Mutex,
    time::{delay_for, Instant},
};
use std::{
    sync::Arc,
    time::Duration,
};

struct WatchdogImpl {
    duration: Duration,
    end: Instant,
}

#[derive(Clone)]
pub struct Watchdog(Arc<Mutex<WatchdogImpl>>);

impl Watchdog {
    pub fn new(duration: Duration) -> Self {
        Watchdog(Arc::new(Mutex::new(WatchdogImpl {
            duration,
            end: Instant::now() + duration,
        })))
    }

    pub async fn pet(&self) {
        let lock = &mut self.0.lock().await;
        lock.end = Instant::now() + lock.duration;
    }

    pub async fn wait(&self) {
        loop {
            if let Ok(lock) = self.0.try_lock() {
                let now = Instant::now();
                if now >= lock.end {
                    return;
                }
                delay_for(lock.end - now).await;
            } else {
                delay_for(Duration::from_millis(100)).await;
            }
        }
    }
}
