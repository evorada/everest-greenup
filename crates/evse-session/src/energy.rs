use std::time::{Duration, Instant};

/// Static limits of the connector, as advertised to the energy manager.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnergyConfig {
    /// Rated current per phase, amps.
    pub max_current_a: f64,
    /// Below this the board does not charge; the session pauses instead.
    pub min_current_a: f64,
    /// Phases wired to the socket.
    pub phases: u8,
    /// Phase-to-neutral voltage used to convert power limits to current.
    pub nominal_voltage_v: f64,
}

/// The root-side limits the energy manager enforces on this node.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LimitRes {
    /// Per-phase current limit, amps.
    pub ac_max_current_a: Option<f64>,
    /// Total power limit, watts.
    pub total_power_w: Option<f64>,
    /// Phase count limit.
    pub ac_max_phase_count: Option<i64>,
}

/// What to do after a limit changed or a timer fired.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decision {
    /// The limit in force, amps per phase (0 when none or expired).
    pub effective_a: f64,
    /// True when the limit allows charging at all.
    pub available: bool,
    /// `CC` value to send now, if one is due.
    pub set_current: Option<u8>,
}

/// Rate limit for `CC` orders: at least this much change ...
const MIN_STEP_A: f64 = 1.0;
/// ... and at most one order per this interval.
const MIN_INTERVAL: Duration = Duration::from_secs(5);

/// The energy-tree leaf of one connector.
///
/// Keeps the enforced limit and its validity, turns it into a current the
/// board accepts, and rate-limits `CC` orders. Until the first limit
/// arrives nothing is available: EnergyManager holds every node at 0 A
/// until it has run once, so it is required in every configuration.
#[derive(Debug, Clone)]
pub struct EnergyNode {
    config: EnergyConfig,
    limit: Option<Limit>,
    wanted: Option<u8>,
    last_sent: Option<(u8, Instant)>,
    last_available: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
struct Limit {
    amps: f64,
    received: Instant,
    valid_for: Duration,
}

impl EnergyNode {
    /// A node with no limit yet.
    pub fn new(config: EnergyConfig) -> Self {
        EnergyNode {
            config,
            limit: None,
            wanted: None,
            last_sent: None,
            last_available: None,
        }
    }

    /// The static limits.
    pub fn config(&self) -> &EnergyConfig {
        &self.config
    }

    /// A new enforced limit arrived.
    pub fn enforce(&mut self, limits: &LimitRes, valid_for: Duration, now: Instant) -> Decision {
        let phases = limits.ac_max_phase_count.map_or(self.config.phases, |p| {
            u8::try_from(p.clamp(1, i64::from(self.config.phases))).unwrap_or(1)
        });
        let mut amps = f64::INFINITY;
        if let Some(a) = limits.ac_max_current_a {
            amps = amps.min(a);
        }
        if let Some(w) = limits.total_power_w {
            amps = amps.min(w / (self.config.nominal_voltage_v * f64::from(phases)));
        }
        if !amps.is_finite() {
            amps = self.config.max_current_a;
        }
        let amps = amps.clamp(0.0, self.config.max_current_a);
        self.limit = Some(Limit {
            amps,
            received: now,
            valid_for,
        });
        let decision = self.decide(now);
        self.last_available = Some(decision.available);
        decision
    }

    /// Timer check. Returns a decision only when availability changed
    /// (a limit expired) or a rate-limited `CC` order became due.
    pub fn tick(&mut self, now: Instant) -> Option<Decision> {
        let decision = self.decide(now);
        let availability_changed = self.last_available != Some(decision.available);
        self.last_available = Some(decision.available);
        if availability_changed || decision.set_current.is_some() {
            Some(decision)
        } else {
            None
        }
    }

    /// The limit in force, amps per phase.
    pub fn effective(&self, now: Instant) -> f64 {
        match self.limit {
            Some(limit) if now.duration_since(limit.received) <= limit.valid_for => limit.amps,
            _ => 0.0,
        }
    }

    fn decide(&mut self, now: Instant) -> Decision {
        let effective = self.effective(now);
        let available = effective + 1e-9 >= self.config.min_current_a;
        let mut set_current = None;
        if available {
            let floor = effective.floor();
            let min = self.config.min_current_a.ceil();
            let max = self.config.max_current_a.floor();
            let cc = floor.clamp(min, max);
            // Safe: clamped to a configured range that fits in u8 for any sane board.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let cc = cc.min(255.0) as u8;
            self.wanted = Some(cc);
            set_current = self.try_send(now);
        }
        Decision {
            effective_a: effective,
            available,
            set_current,
        }
    }

    fn try_send(&mut self, now: Instant) -> Option<u8> {
        let wanted = self.wanted?;
        let due = match self.last_sent {
            None => true,
            Some((last, at)) => {
                last != wanted
                    && f64::from(last.abs_diff(wanted)) >= MIN_STEP_A
                    && now.duration_since(at) >= MIN_INTERVAL
            }
        };
        if due {
            self.last_sent = Some((wanted, now));
            Some(wanted)
        } else {
            None
        }
    }
}
