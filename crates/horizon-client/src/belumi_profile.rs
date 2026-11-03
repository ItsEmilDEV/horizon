use horizon_client_render::spans::Span;
use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const BUDGET_MS: f32 = 50.0;
const TOP: usize = 8;

#[derive(Default)]
struct SpanStat {
    name: &'static str,
    sum: f32,
    cpu: f32,
    max: f32,
}

pub struct Profiler {
    period: Duration,
    since: Instant,
    ticks: u32,
    over: u32,
    sum: f32,
    max: f32,
    spans: Vec<SpanStat>,
}

impl Profiler {
    pub fn new(period: Duration, now: Instant) -> Self {
        Self { period, since: now, ticks: 0, over: 0, sum: 0.0, max: 0.0, spans: Vec::new() }
    }

    pub fn record(&mut self, now: Instant, wall_ms: f32, spans: &[Span]) -> Option<String> {
        self.ticks += 1;
        self.sum += wall_ms;
        self.max = self.max.max(wall_ms);
        if wall_ms > BUDGET_MS {
            self.over += 1;
        }
        for s in spans {
            let stat = match self.spans.iter().position(|x| x.name == s.name) {
                Some(i) => &mut self.spans[i],
                None => {
                    self.spans.push(SpanStat { name: s.name, ..SpanStat::default() });
                    self.spans.last_mut().unwrap()
                }
            };
            stat.sum += s.ms;
            stat.cpu += s.cpu_ms;
            stat.max = stat.max.max(s.ms);
        }
        if now.duration_since(self.since) < self.period {
            return None;
        }
        let report = self.report(now);
        *self = Self::new(self.period, now);
        Some(report)
    }

    fn report(&mut self, now: Instant) -> String {
        let n = self.ticks.max(1) as f32;
        let secs = now.duration_since(self.since).as_secs_f32();
        self.spans.sort_by(|a, b| b.sum.total_cmp(&a.sum));
        let top: Vec<String> = self
            .spans
            .iter()
            .take(TOP)
            .filter(|s| s.sum / n >= 0.01)
            .map(|s| format!("{} {:.2}/{:.1} ms", s.name, s.sum / n, s.max))
            .collect();
        format!(
            "[belumi] profile {secs:.0}s: {} ticks, mean {:.2} ms, max {:.1} ms, {} over {BUDGET_MS:.0} ms; spans (mean/max): {}",
            self.ticks,
            self.sum / n,
            self.max,
            self.over,
            if top.is_empty() { "none".to_owned() } else { top.join(", ") }
        )
    }
}

fn period() -> Option<Duration> {
    static PERIOD: OnceLock<Option<Duration>> = OnceLock::new();
    *PERIOD.get_or_init(|| {
        let secs: f32 = std::env::var("BELUMI_PROFILE").ok()?.parse().ok()?;
        (secs > 0.0).then(|| Duration::from_secs_f32(secs))
    })
}

pub fn enabled() -> bool {
    period().is_some()
}

static PROFILER: Mutex<Option<Profiler>> = Mutex::new(None);

pub fn record_tick(wall_ms: f32, spans: &[Span]) {
    let Some(period) = period() else { return };
    let now = Instant::now();
    let mut guard = PROFILER.lock().unwrap();
    let p = guard.get_or_insert_with(|| Profiler::new(period, now));
    if let Some(line) = p.record(now, wall_ms, spans) {
        log::info!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(name: &'static str, ms: f32) -> Span {
        Span { name, ms, cpu_ms: ms }
    }

    #[test]
    fn reports_after_the_period_with_means_and_worst() {
        let t0 = Instant::now();
        let mut p = Profiler::new(Duration::from_secs(1), t0);
        assert!(p.record(t0 + Duration::from_millis(100), 2.0, &[span("chunks", 1.0)]).is_none());
        assert!(p.record(t0 + Duration::from_millis(200), 60.0, &[span("chunks", 5.0), span("mobs", 3.0)]).is_none());
        let line = p.record(t0 + Duration::from_millis(1100), 4.0, &[span("mobs", 1.0)]).unwrap();
        assert!(line.contains("3 ticks"), "{line}");
        assert!(line.contains("max 60.0 ms"), "{line}");
        assert!(line.contains("1 over 50 ms"), "{line}");
        assert!(line.contains("chunks 2.00/5.0 ms, mobs 1.33/3.0 ms"), "{line}");
        assert!(p.record(t0 + Duration::from_millis(1200), 1.0, &[]).is_none());
    }
}
