use governor::{
    clock::DefaultClock,
    middleware::NoOpMiddleware,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};
use nonzero_ext::nonzero;
use std::{
    collections::HashMap,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

/// Per-host token-bucket rate limiters.
pub struct PerHostRateLimiter {
    limiters: Mutex<HashMap<String, Arc<Limiter>>>,
    rps: NonZeroU32,
    burst: NonZeroU32,
}

impl PerHostRateLimiter {
    pub fn new(requests_per_second: f64, burst: u32) -> Self {
        let rps =
            NonZeroU32::new(requests_per_second.max(1.0).round() as u32).unwrap_or(nonzero!(1u32));
        let burst = NonZeroU32::new(burst.max(1)).unwrap_or(nonzero!(1u32));
        Self {
            limiters: Mutex::new(HashMap::new()),
            rps,
            burst,
        }
    }

    /// Clone the `Arc<Limiter>` for `host` (inserting a new one if needed) and
    /// release the mutex before awaiting — so other hosts are never blocked.
    pub async fn wait(&self, host: &str) {
        let limiter: Arc<Limiter> = {
            let mut map = self.limiters.lock().unwrap();
            Arc::clone(map.entry(host.to_string()).or_insert_with(|| {
                let quota = Quota::per_second(self.rps).allow_burst(self.burst);
                Arc::new(RateLimiter::direct(quota))
            }))
        };
        limiter.until_ready().await;
    }
}
