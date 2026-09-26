//! Monotonic deadline arithmetic, independent of scheduling and object storage.
#[derive(Clone, Copy)]
pub struct Timer {
    pub next: Option<u64>,
    period: u64,
    pending: u64,
}
impl Timer {
    pub fn new(deadline: u64, period: u64) -> Option<Self> {
        (deadline < u64::MAX && period < u64::MAX).then_some(Self {
            next: Some(deadline),
            period,
            pending: 0,
        })
    }
    pub fn advance(&mut self, now: u64) {
        let Some(next) = self.next.filter(|&d| now >= d) else {
            return;
        };
        let count = if self.period == 0 {
            1
        } else {
            now.saturating_sub(next)
                .checked_div(self.period)
                .unwrap_or(0)
                .saturating_add(1)
        };
        self.pending = self.pending.saturating_add(count);
        self.next = if self.period == 0 {
            None
        } else {
            self.period
                .checked_mul(count)
                .and_then(|step| next.checked_add(step))
                .filter(|&d| d < u64::MAX)
        };
    }
    pub fn take(&mut self) -> u64 {
        core::mem::take(&mut self.pending)
    }
    pub fn exhausted(&self) -> bool {
        self.next.is_none() && self.pending == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_shot_and_periodic_coalescing() {
        let mut once = Timer::new(10, 0).unwrap();
        once.advance(9);
        assert_eq!(once.take(), 0);
        once.advance(10);
        assert_eq!(once.take(), 1);
        once.advance(100);
        assert_eq!(once.take(), 0);
        assert!(once.exhausted());
        let mut periodic = Timer::new(10, 3).unwrap();
        periodic.advance(20);
        assert_eq!(periodic.next, Some(22));
        periodic.advance(25);
        assert_eq!(periodic.take(), 6);
        assert_eq!(periodic.next, Some(28));
    }
    #[test]
    fn overflow_retires_deadline_without_wrapping() {
        assert!(Timer::new(u64::MAX, 1).is_none());
        let mut timer = Timer::new(u64::MAX - 2, 3).unwrap();
        timer.advance(u64::MAX - 1);
        assert_eq!(timer.next, None);
        assert_eq!(timer.take(), 1);
        let mut timer = Timer::new(0, 1).unwrap();
        timer.advance(u64::MAX);
        assert_eq!(timer.take(), u64::MAX);
        assert!(timer.exhausted());
    }
}
