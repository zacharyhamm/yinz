//! Server-local bounded login history and token buckets. All time comes from Clock.
use super::RateLimited;
use std::collections::{HashMap, VecDeque};
fn denial(ms: i64) -> RateLimited {
    RateLimited {
        retry_after_s: (ms.max(1) as u64).div_ceil(1000),
    }
}
#[derive(Default)]
struct Client {
    failures: VecDeque<i64>,
    until: i64,
    level: u32,
    last_failure: i64,
}
impl Client {
    fn prune(&mut self, now: i64) {
        self.failures.retain(|t| now.saturating_sub(*t) < 900000);
        if now.saturating_sub(self.last_failure) >= 900000 && now >= self.until {
            self.level = 0;
        }
    }
}
pub(crate) struct Bucket {
    tokens: f64,
    last: i64,
    rate: f64,
    burst: f64,
}
impl Bucket {
    pub fn new(now: i64, rate: u32, burst: u32) -> Self {
        Self {
            tokens: f64::from(burst),
            last: now,
            rate: f64::from(rate),
            burst: f64::from(burst),
        }
    }
    pub fn spend(&mut self, now: i64) -> Result<(), RateLimited> {
        let elapsed = now.saturating_sub(self.last).max(0);
        self.last = self.last.max(now);
        self.tokens = (self.tokens + elapsed as f64 * self.rate / 1000.0).min(self.burst);
        if self.tokens < 1.0 {
            return Err(denial(
                ((1.0 - self.tokens) * 1000.0 / self.rate).ceil() as i64
            ));
        }
        self.tokens -= 1.0;
        Ok(())
    }
}
#[derive(Default)]
pub(crate) struct Limiter {
    clients: HashMap<String, Client>,
    overflow: Client,
    global: VecDeque<i64>,
    global_until: i64,
    api: HashMap<String, (Bucket, i64)>,
    public: Option<Bucket>,
}
impl Limiter {
    fn client(&mut self, key: &str, now: i64) -> &mut Client {
        self.clients.retain(|_, c| {
            c.prune(now);
            !(c.failures.is_empty() && c.level == 0 && now >= c.until)
        });
        if key.len() > 256 || (!self.clients.contains_key(key) && self.clients.len() >= 1024) {
            self.overflow.prune(now);
            return &mut self.overflow;
        }
        self.clients.entry(key.into()).or_default()
    }
    pub fn admit(&mut self, key: &str, now: i64) -> Result<(), RateLimited> {
        if now < self.global_until {
            return Err(denial(self.global_until - now));
        }
        let c = self.client(key, now);
        if now < c.until {
            return Err(denial(c.until - now));
        }
        Ok(())
    }
    pub fn failure(&mut self, key: &str, now: i64) {
        self.global.retain(|t| now.saturating_sub(*t) < 3600000);
        if now >= self.global_until {
            self.global.push_back(now);
        }
        if self.global.len() >= 30 {
            self.global_until = self.global_until.max(now.saturating_add(900000));
            self.global.clear();
        }
        let c = self.client(key, now);
        c.last_failure = c.last_failure.max(now);
        if now < c.until {
            return;
        }
        c.failures.push_back(now);
        if c.failures.len() >= 5 {
            c.until =
                now.saturating_add((60000i64.saturating_mul(1 << c.level.min(4))).min(900000));
            c.level = (c.level + 1).min(4);
            c.failures.clear();
        }
    }
    pub fn success(&mut self, key: &str, now: i64) {
        *self.client(key, now) = Client::default();
    }
    pub fn api(&mut self, id: &str, expires: i64, now: i64) -> Result<(), RateLimited> {
        self.api.retain(|_, (_, expiry)| now < *expiry);
        let (b, e) = self
            .api
            .entry(id.into())
            .or_insert_with(|| (Bucket::new(now, 20, 40), expires));
        *e = expires;
        b.spend(now)
    }
    pub fn retain_sessions<'a>(&mut self, ids: impl Iterator<Item = &'a str>) {
        let ids: std::collections::HashSet<_> = ids.collect();
        self.api.retain(|id, _| ids.contains(id.as_str()));
    }
    pub fn remove(&mut self, id: &str) {
        self.api.remove(id);
    }
    pub fn public(&mut self, now: i64) -> Result<(), RateLimited> {
        self.public
            .get_or_insert_with(|| Bucket::new(now, 50, 100))
            .spend(now)
    }
}
#[cfg(test)]
#[path = "../unit/limiter.rs"]
mod tests;
