//! WovenWiFi Stage 13.9L — reliability and lifecycle hardening.
//!
//! Adds a small policy/state layer for bounded retries, timeout recovery,
//! disconnect cleanup, reconnect backoff, and generation-tagged association
//! epochs so stale work from an old connection cannot be mistaken for the new
//! one. Cryptographic material remains owned by the Stage G/H/I modules; this
//! layer drives the lifecycle in which those secrets must be discarded.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryState {
    Idle,
    Connecting,
    Connected,
    Backoff,
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    WrongState,
    RetryExhausted,
    StaleEpoch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryStats {
    pub epoch: u32,
    pub attempts: u8,
    pub disconnects: u32,
    pub timeouts: u32,
    pub failures: u32,
}

pub struct RecoveryController {
    state: RecoveryState,
    epoch: u32,
    attempts: u8,
    max_attempts: u8,
    backoff_ticks: u32,
    deadline_tick: u64,
    disconnects: u32,
    timeouts: u32,
    failures: u32,
}

impl RecoveryController {
    pub const fn new(max_attempts: u8) -> Self {
        Self {
            state: RecoveryState::Idle,
            epoch: 1,
            attempts: 0,
            max_attempts,
            backoff_ticks: 0,
            deadline_tick: 0,
            disconnects: 0,
            timeouts: 0,
            failures: 0,
        }
    }

    pub const fn state(&self) -> RecoveryState {
        self.state
    }
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }
    pub const fn attempts(&self) -> u8 {
        self.attempts
    }

    pub const fn stats(&self) -> RecoveryStats {
        RecoveryStats {
            epoch: self.epoch,
            attempts: self.attempts,
            disconnects: self.disconnects,
            timeouts: self.timeouts,
            failures: self.failures,
        }
    }

    pub fn begin_connect(
        &mut self,
        now_tick: u64,
        timeout_ticks: u64,
    ) -> Result<u32, RecoveryError> {
        if !matches!(self.state, RecoveryState::Idle | RecoveryState::Backoff) {
            return Err(RecoveryError::WrongState);
        }
        if self.attempts >= self.max_attempts {
            self.state = RecoveryState::Exhausted;
            return Err(RecoveryError::RetryExhausted);
        }
        self.attempts = self.attempts.saturating_add(1);
        self.state = RecoveryState::Connecting;
        self.deadline_tick = now_tick.saturating_add(timeout_ticks);
        Ok(self.epoch)
    }

    pub fn connected(&mut self, epoch: u32) -> Result<(), RecoveryError> {
        self.require_epoch(epoch)?;
        if self.state != RecoveryState::Connecting {
            return Err(RecoveryError::WrongState);
        }
        self.state = RecoveryState::Connected;
        self.attempts = 0;
        self.backoff_ticks = 0;
        self.deadline_tick = 0;
        Ok(())
    }

    pub fn poll_timeout(&mut self, now_tick: u64) -> bool {
        if self.state == RecoveryState::Connecting && now_tick >= self.deadline_tick {
            self.timeouts = self.timeouts.saturating_add(1);
            self.failures = self.failures.saturating_add(1);
            self.enter_backoff();
            true
        } else {
            false
        }
    }

    pub fn connection_failed(&mut self, epoch: u32) -> Result<(), RecoveryError> {
        self.require_epoch(epoch)?;
        if self.state != RecoveryState::Connecting {
            return Err(RecoveryError::WrongState);
        }
        self.failures = self.failures.saturating_add(1);
        self.enter_backoff();
        Ok(())
    }

    pub fn disconnect(&mut self) -> u32 {
        self.disconnects = self.disconnects.saturating_add(1);
        self.bump_epoch();
        self.state = RecoveryState::Idle;
        self.attempts = 0;
        self.backoff_ticks = 0;
        self.deadline_tick = 0;
        self.epoch
    }

    pub fn tick_backoff(&mut self) -> bool {
        if self.state != RecoveryState::Backoff {
            return false;
        }
        if self.backoff_ticks > 0 {
            self.backoff_ticks -= 1;
        }
        if self.backoff_ticks == 0 {
            self.state = if self.attempts >= self.max_attempts {
                RecoveryState::Exhausted
            } else {
                RecoveryState::Idle
            };
            true
        } else {
            false
        }
    }

    pub const fn backoff_ticks(&self) -> u32 {
        self.backoff_ticks
    }

    fn enter_backoff(&mut self) {
        // Bounded exponential backoff: 1,2,4,...32 ticks.
        let shift = core::cmp::min(self.attempts.saturating_sub(1), 5);
        self.backoff_ticks = 1u32 << shift;
        self.bump_epoch();
        self.state = RecoveryState::Backoff;
        self.deadline_tick = 0;
    }

    fn bump_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.epoch = 1;
        }
    }

    fn require_epoch(&self, epoch: u32) -> Result<(), RecoveryError> {
        if epoch != self.epoch {
            return Err(RecoveryError::StaleEpoch);
        }
        Ok(())
    }
}

pub fn self_test() -> bool {
    let mut recovery = RecoveryController::new(3);

    let Ok(epoch1) = recovery.begin_connect(100, 10) else {
        return false;
    };
    if recovery.state() != RecoveryState::Connecting || recovery.poll_timeout(109) {
        return false;
    }
    if !recovery.poll_timeout(110)
        || recovery.state() != RecoveryState::Backoff
        || recovery.backoff_ticks() != 1
    {
        return false;
    }
    // Epoch changed on failure: late completion from the old attempt is stale.
    if recovery.connected(epoch1) != Err(RecoveryError::StaleEpoch) {
        return false;
    }

    if !recovery.tick_backoff() || recovery.state() != RecoveryState::Idle {
        return false;
    }
    let Ok(epoch2) = recovery.begin_connect(200, 10) else {
        return false;
    };
    if recovery.connection_failed(epoch2).is_err()
        || recovery.state() != RecoveryState::Backoff
        || recovery.backoff_ticks() != 2
    {
        return false;
    }
    if recovery.tick_backoff()
        || !recovery.tick_backoff()
        || recovery.state() != RecoveryState::Idle
    {
        return false;
    }

    let Ok(epoch3) = recovery.begin_connect(300, 10) else {
        return false;
    };
    if recovery.connected(epoch3).is_err()
        || recovery.state() != RecoveryState::Connected
        || recovery.attempts() != 0
    {
        return false;
    }

    // Disconnect invalidates all work from the old association and resets retry policy.
    let new_epoch = recovery.disconnect();
    if new_epoch == epoch3 || recovery.state() != RecoveryState::Idle || recovery.attempts() != 0 {
        return false;
    }
    if recovery.connected(epoch3) != Err(RecoveryError::StaleEpoch) {
        return false;
    }

    // Repeated failures are bounded; no infinite retry loop.
    for expected in 1..=3u8 {
        let Ok(e) = recovery.begin_connect(400 + u64::from(expected), 1) else {
            return false;
        };
        if recovery.attempts() != expected || recovery.connection_failed(e).is_err() {
            return false;
        }
        while recovery.state() == RecoveryState::Backoff {
            let _ = recovery.tick_backoff();
        }
    }
    if recovery.state() != RecoveryState::Exhausted
        || recovery.begin_connect(500, 1) != Err(RecoveryError::WrongState)
    {
        return false;
    }

    let stats = recovery.stats();
    stats.timeouts == 1 && stats.failures == 5 && stats.disconnects == 1
}
