//! The gateway's reconnect logic as a pure state machine.
//!
//! [`Machine::step`] takes an [`Input`] and returns the [`Action`]s the driver
//! must perform; it does no I/O and takes jitter as an argument, so it is
//! unit-testable without a network or a clock.

use std::time::Duration;

use crate::gateway::{GatewayConfig, GatewayUpdate};
use crate::types::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    /// Created, nothing done yet.
    Idle,
    /// Opening a stream.
    Connecting,
    /// Stream open; `healthy` once the first event has arrived.
    Connected { healthy: bool },
    /// Waiting before the next attempt.
    Backoff,
    /// Terminal.
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The token was rejected; retrying cannot help.
    Unauthenticated,
    /// Connect failure, stream error, stream end or watchdog timeout.
    Transient(String),
}

#[derive(Debug)]
pub(crate) enum Input {
    Start,
    /// The stream was opened.
    Opened,
    /// An event arrived on the stream.
    Event(Event),
    /// Connecting or the open stream failed.
    Failed(Failure),
    /// The backoff delay elapsed.
    BackoffElapsed,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    /// Tell the app.
    Emit(GatewayUpdate),
    /// Open a stream, then feed back `Opened` or `Failed`.
    Connect,
    /// Sleep, then feed back `BackoffElapsed`.
    Wait(Duration),
    /// Stop the task.
    Halt,
}

pub(crate) struct Machine {
    state: State,
    /// Consecutive failed attempts since the last healthy connection.
    failures: u32,
    config: GatewayConfig,
}

impl Machine {
    pub(crate) fn new(config: GatewayConfig) -> Self {
        Self {
            state: State::Idle,
            failures: 0,
            config,
        }
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> State {
        self.state
    }

    /// Delay before retry number `failures` (1-based): `base * 2^(n-1)` capped
    /// at `max`, then "equal jitter" — uniformly between half and all of it —
    /// with `jitter` in `[0, 1]`.
    pub(crate) fn delay(&self, failures: u32, jitter: f64) -> Duration {
        let exp = failures.saturating_sub(1).min(31);
        let raw = self.config.initial_backoff.saturating_mul(1u32 << exp);
        let capped = raw.min(self.config.max_backoff);
        capped.mul_f64(0.5 + 0.5 * jitter.clamp(0.0, 1.0))
    }

    pub(crate) fn step(&mut self, input: Input, jitter: f64) -> Vec<Action> {
        match (self.state, input) {
            (State::Stopped, _) => vec![Action::Halt],
            (State::Idle | State::Backoff, Input::Start | Input::BackoffElapsed) => {
                self.state = State::Connecting;
                vec![Action::Emit(GatewayUpdate::Connecting), Action::Connect]
            }
            (State::Connecting, Input::Opened) => {
                self.state = State::Connected { healthy: false };
                vec![Action::Emit(GatewayUpdate::Connected)]
            }
            (State::Connected { healthy }, Input::Event(event)) => {
                if !healthy {
                    // Traffic proves the connection works: restart the backoff.
                    self.failures = 0;
                    self.state = State::Connected { healthy: true };
                }
                vec![Action::Emit(GatewayUpdate::Event(event))]
            }
            (
                State::Connecting | State::Connected { .. },
                Input::Failed(Failure::Unauthenticated),
            ) => {
                self.state = State::Stopped;
                vec![Action::Emit(GatewayUpdate::AuthExpired), Action::Halt]
            }
            (
                State::Connecting | State::Connected { .. },
                Input::Failed(Failure::Transient(reason)),
            ) => {
                self.failures += 1;
                let retry_in = self.delay(self.failures, jitter);
                self.state = State::Backoff;
                vec![
                    Action::Emit(GatewayUpdate::Disconnected { retry_in, reason }),
                    Action::Wait(retry_in),
                ]
            }
            // An input that makes no sense in the current state is ignored.
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> GatewayConfig {
        GatewayConfig {
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            ..Default::default()
        }
    }

    fn transient(reason: &str) -> Input {
        Input::Failed(Failure::Transient(reason.into()))
    }

    fn retry_in(actions: &[Action]) -> Duration {
        match actions {
            [Action::Emit(GatewayUpdate::Disconnected { retry_in, .. }), Action::Wait(wait)] => {
                assert_eq!(retry_in, wait);
                *retry_in
            }
            other => panic!("unexpected actions {other:?}"),
        }
    }

    #[test]
    fn delay_doubles_caps_and_applies_jitter() {
        let m = Machine::new(config());
        assert_eq!(m.delay(1, 1.0), Duration::from_millis(500));
        assert_eq!(m.delay(2, 1.0), Duration::from_secs(1));
        assert_eq!(m.delay(3, 1.0), Duration::from_secs(2));
        assert_eq!(m.delay(7, 1.0), Duration::from_secs(30));
        assert_eq!(m.delay(1000, 1.0), Duration::from_secs(30));
        assert_eq!(m.delay(1, 0.0), Duration::from_millis(250));
        assert_eq!(m.delay(1000, 0.0), Duration::from_secs(15));
    }

    #[test]
    fn happy_path_then_drop_reconnects() {
        let mut m = Machine::new(config());
        assert_eq!(
            m.step(Input::Start, 1.0),
            vec![Action::Emit(GatewayUpdate::Connecting), Action::Connect]
        );
        assert_eq!(
            m.step(Input::Opened, 1.0),
            vec![Action::Emit(GatewayUpdate::Connected)]
        );
        assert_eq!(
            m.step(Input::Event(Event::Heartbeat), 1.0),
            vec![Action::Emit(GatewayUpdate::Event(Event::Heartbeat))]
        );
        let wait = retry_in(&m.step(transient("stream ended"), 1.0));
        assert_eq!(wait, Duration::from_millis(500));
        assert_eq!(m.state(), State::Backoff);
        assert_eq!(
            m.step(Input::BackoffElapsed, 1.0),
            vec![Action::Emit(GatewayUpdate::Connecting), Action::Connect]
        );
    }

    #[test]
    fn consecutive_failures_back_off_until_traffic_resets() {
        let mut m = Machine::new(config());
        m.step(Input::Start, 1.0);
        let mut waits = vec![];
        for _ in 0..3 {
            waits.push(retry_in(&m.step(transient("refused"), 1.0)));
            m.step(Input::BackoffElapsed, 1.0);
        }
        assert_eq!(waits, [500, 1000, 2000].map(Duration::from_millis));

        // Opening a stream alone does not reset: a server that accepts then
        // drops immediately must still be backed off from.
        m.step(Input::Opened, 1.0);
        assert_eq!(
            retry_in(&m.step(transient("dropped"), 1.0)),
            Duration::from_secs(4)
        );
        m.step(Input::BackoffElapsed, 1.0);

        m.step(Input::Opened, 1.0);
        m.step(Input::Event(Event::Heartbeat), 1.0);
        assert_eq!(
            retry_in(&m.step(transient("dropped"), 1.0)),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn unauthenticated_stops_permanently() {
        let mut m = Machine::new(config());
        m.step(Input::Start, 1.0);
        m.step(Input::Opened, 1.0);
        assert_eq!(
            m.step(Input::Failed(Failure::Unauthenticated), 1.0),
            vec![Action::Emit(GatewayUpdate::AuthExpired), Action::Halt]
        );
        assert_eq!(m.state(), State::Stopped);
        assert_eq!(m.step(Input::BackoffElapsed, 1.0), vec![Action::Halt]);
    }

    #[test]
    fn stray_inputs_are_ignored() {
        let mut m = Machine::new(config());
        assert!(m.step(Input::Opened, 1.0).is_empty());
        assert!(m.step(Input::Event(Event::Heartbeat), 1.0).is_empty());
        assert_eq!(m.state(), State::Idle);
    }
}
