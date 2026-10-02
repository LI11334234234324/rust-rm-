use rm_server_sync::Clock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 手动时钟：测试把"现在"拨到想要的位置，不必真的睡过去。
pub struct ManualClock {
    now: Mutex<Instant>,
}

impl ManualClock {
    pub fn new(at: Instant) -> Self {
        Self {
            now: Mutex::new(at),
        }
    }

    pub fn advance(&self, by: Duration) {
        *self.now.lock().unwrap() += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }
}
