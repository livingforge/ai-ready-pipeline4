use std::time::{Duration, Instant};

/// Disjoint wall-clock intervals, including waits; not CPU time. CLI response
/// formatting and stdout writing are added by the caller after Store::search.
#[derive(Default)]
pub struct SearchTimings {
    pub setup: Duration,
    pub integrity: Duration,
    pub index_update: Duration,
    pub sql: Duration,
    pub response: Duration,
}

impl SearchTimings {
    pub fn report(&self, total: Duration) -> serde_json::Value {
        let measured = self.setup + self.integrity + self.index_update + self.sql + self.response;
        let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
        serde_json::json!({
            "kind": "document-search-profile",
            "setup_ms": ms(self.setup),
            "integrity_ms": ms(self.integrity),
            "index_update_ms": ms(self.index_update),
            "sql_ms": ms(self.sql),
            "response_ms": ms(self.response),
            "other_ms": ms(total.saturating_sub(measured)),
            "total_ms": ms(total)
        })
    }
}

pub(super) struct Timer<'a> {
    target: &'a mut Duration,
    start: Instant,
}

impl<'a> Timer<'a> {
    pub(super) fn new(target: &'a mut Duration) -> Self {
        Self {
            target,
            start: Instant::now(),
        }
    }
}

impl Drop for Timer<'_> {
    fn drop(&mut self) {
        *self.target += self.start.elapsed();
    }
}
