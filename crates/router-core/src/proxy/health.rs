#![allow(clippy::if_not_else)]
#![allow(clippy::too_many_lines)]

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::domain::RouteId;

pub const FAILURE_THRESHOLD: u8 = 5;
pub const ACTIVATION_WRITE_RETRY_DELAY: Duration = Duration::from_mins(1);
pub const RECOVERY_EVIDENCE_DEADLINE: Duration = Duration::from_mins(1);
const RECOVERY_BACKOFF: [Duration; 4] = [
    Duration::from_mins(1),
    Duration::from_mins(2),
    Duration::from_mins(5),
    Duration::from_mins(10),
];

pub trait MonotonicClock: Send + Sync {
    fn now(&self) -> Duration;
}

pub struct SystemMonotonicClock {
    started: Instant,
}

impl Default for SystemMonotonicClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl MonotonicClock for SystemMonotonicClock {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthFailureClass {
    Connection,
    Timeout,
    RateLimit,
    Authentication,
    Quota,
    Billing,
    ModelUnavailable,
    Service,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryOrigin {
    ProviderFailure,
    ModelBypassed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteHealthSnapshot {
    Striking {
        failure_count: u8,
    },
    Switching,
    SwitchPending {
        retry_after_seconds: Option<u16>,
    },
    Open {
        origin: RecoveryOrigin,
        recovery_successes: u8,
        retry_after_seconds: u16,
    },
    Probing {
        recovery_successes: u8,
    },
}

pub trait HealthChangeSink: Send + Sync {
    fn route_health_changed(&self, route_id: RouteId, health: Option<RouteHealthSnapshot>);
}

pub struct NoopHealthChangeSink;

impl HealthChangeSink for NoopHealthChangeSink {
    fn route_health_changed(&self, _route_id: RouteId, _health: Option<RouteHealthSnapshot>) {}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthAttemptRef {
    pub request_id: String,
    pub attempt_index: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TripLease {
    pub route_id: RouteId,
    pub lease_id: u64,
    pub version: u64,
    pub selection_generation: u64,
    pub failure: HealthFailureClass,
    pub source_attempt: Option<HealthAttemptRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeLease {
    pub route_id: RouteId,
    pub lease_id: u64,
    pub version: u64,
    pub selection_generation: u64,
    pub backoff_step: u8,
    pub recovery_successes: u8,
    pub origin: RecoveryOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingProof {
    pub route_id: RouteId,
    pub version: u64,
    pub selection_generation: u64,
    pub failure: HealthFailureClass,
    pub source_attempt: Option<HealthAttemptRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HealthActivationProof {
    Advance {
        source: TripLease,
    },
    AdvanceRecovered {
        source: PendingProof,
        target: ProbeLease,
    },
    Recover {
        target: ProbeLease,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthActivationReservation {
    reservation_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StrikeResult {
    Ignored,
    BelowThreshold { failure_count: u8 },
    TripAcquired(TripLease),
    TripBusy,
    Pending,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateHealth {
    Closed,
    Pending,
    OpenCooling,
    OpenReady,
    Probing,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeLeaseResult {
    Acquired(ProbeLease),
    Busy,
    NoneReady,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaterProbeLeaseResult {
    Acquired {
        source: PendingProof,
        probe: ProbeLease,
    },
    Busy,
    NotReady,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeCompletion {
    FirstPositive,
    SecondPositiveReady,
    Applied,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivatedSkipKind {
    HealthUnavailable,
    ModelFallbackExcluded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivatedSkipHealth {
    pub route_id: RouteId,
    pub kind: ActivatedSkipKind,
}

#[derive(Clone, Debug)]
enum Entry {
    Striking {
        failure_count: u8,
        last_failure: HealthFailureClass,
        version: u64,
        selection_generation: u64,
    },
    Tripping {
        lease_id: u64,
        failure: HealthFailureClass,
        reserved: bool,
        version: u64,
        selection_generation: u64,
        source_attempt: Option<HealthAttemptRef>,
    },
    Pending {
        failure: HealthFailureClass,
        activation_retry_at: Option<Duration>,
        version: u64,
        selection_generation: u64,
        source_attempt: Option<HealthAttemptRef>,
    },
    Open {
        retry_at: Duration,
        backoff_step: u8,
        recovery_successes: u8,
        origin: RecoveryOrigin,
        last_failure: Option<HealthFailureClass>,
        version: u64,
        selection_generation: u64,
    },
    HalfOpen {
        lease_id: u64,
        backoff_step: u8,
        recovery_successes: u8,
        origin: RecoveryOrigin,
        last_failure: Option<HealthFailureClass>,
        version: u64,
        selection_generation: u64,
    },
}

impl Entry {
    const fn generation(&self) -> u64 {
        match self {
            Self::Striking {
                selection_generation,
                ..
            }
            | Self::Tripping {
                selection_generation,
                ..
            }
            | Self::Pending {
                selection_generation,
                ..
            }
            | Self::Open {
                selection_generation,
                ..
            }
            | Self::HalfOpen {
                selection_generation,
                ..
            } => *selection_generation,
        }
    }

    const fn version(&self) -> u64 {
        match self {
            Self::Striking { version, .. }
            | Self::Tripping { version, .. }
            | Self::Pending { version, .. }
            | Self::Open { version, .. }
            | Self::HalfOpen { version, .. } => *version,
        }
    }
}

#[derive(Default)]
struct RegistryState {
    routes: HashMap<RouteId, Entry>,
    active_probe: Option<(RouteId, u64, u64, u64)>,
    activation_reservation: Option<u64>,
    health_generation: Option<u64>,
    next_lease_id: u64,
    next_version: u64,
    next_activation_reservation_id: u64,
}

impl RegistryState {
    fn lease_id(&mut self) -> u64 {
        self.next_lease_id = self.next_lease_id.wrapping_add(1).max(1);
        self.next_lease_id
    }

    fn version(&mut self) -> u64 {
        self.next_version = self.next_version.wrapping_add(1).max(1);
        self.next_version
    }

    fn activation_reservation_id(&mut self) -> u64 {
        self.next_activation_reservation_id =
            self.next_activation_reservation_id.wrapping_add(1).max(1);
        self.next_activation_reservation_id
    }
}

pub struct RouteHealthRegistry {
    state: Mutex<RegistryState>,
    clock: Arc<dyn MonotonicClock>,
    changes: Arc<dyn HealthChangeSink>,
}

impl Default for RouteHealthRegistry {
    fn default() -> Self {
        Self::new(
            Arc::new(SystemMonotonicClock::default()),
            Arc::new(NoopHealthChangeSink),
        )
    }
}

impl RouteHealthRegistry {
    #[must_use]
    pub fn new(clock: Arc<dyn MonotonicClock>, changes: Arc<dyn HealthChangeSink>) -> Self {
        Self {
            state: Mutex::new(RegistryState::default()),
            clock,
            changes,
        }
    }

    #[must_use]
    pub fn snapshot(&self, route_id: &RouteId) -> Option<RouteHealthSnapshot> {
        let now = self.clock.now();
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.routes.get(route_id).map(|entry| {
            debug_assert_ne!(entry.version(), 0);
            project(entry, now)
        })
    }

    #[must_use]
    pub fn snapshots(&self) -> HashMap<RouteId, RouteHealthSnapshot> {
        let now = self.clock.now();
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .routes
            .iter()
            .map(|(route_id, entry)| (route_id.clone(), project(entry, now)))
            .collect()
    }

    #[must_use]
    pub fn health_generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .health_generation
            .unwrap_or(0)
    }

    #[must_use]
    pub fn candidate_health(
        &self,
        route_id: &RouteId,
        selection_generation: u64,
    ) -> CandidateHealth {
        let now = self.clock.now();
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .health_generation
            .is_some_and(|generation| generation != selection_generation)
        {
            return CandidateHealth::Stale;
        }
        match state.routes.get(route_id) {
            Some(entry) if entry.generation() != selection_generation => CandidateHealth::Stale,
            None | Some(Entry::Striking { .. }) => CandidateHealth::Closed,
            Some(Entry::Tripping { .. } | Entry::Pending { .. }) => CandidateHealth::Pending,
            Some(Entry::Open { retry_at, .. }) if *retry_at <= now => CandidateHealth::OpenReady,
            Some(Entry::Open { .. }) => CandidateHealth::OpenCooling,
            Some(Entry::HalfOpen { .. }) => CandidateHealth::Probing,
        }
    }

    #[must_use]
    pub fn activation_proof_current(
        &self,
        proof: &HealthActivationProof,
        current_route_id: &RouteId,
        target_route_id: &RouteId,
        skipped: &[ActivatedSkipHealth],
    ) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        activation_proof_matches(&state, proof, current_route_id, target_route_id, skipped)
    }

    #[must_use]
    pub fn begin_activation(
        &self,
        proof: &HealthActivationProof,
        current_route_id: &RouteId,
        target_route_id: &RouteId,
        skipped: &[ActivatedSkipHealth],
    ) -> Option<HealthActivationReservation> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.activation_reservation.is_some()
            || !activation_proof_matches(&state, proof, current_route_id, target_route_id, skipped)
        {
            return None;
        }
        let reservation_id = state.activation_reservation_id();
        state.activation_reservation = Some(reservation_id);
        Some(HealthActivationReservation { reservation_id })
    }

    pub fn cancel_activation(&self, reservation: &HealthActivationReservation) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.activation_reservation != Some(reservation.reservation_id) {
            return false;
        }
        state.activation_reservation = None;
        true
    }

    pub fn record_ordinary_positive(&self, route_id: &RouteId, selection_generation: u64) -> bool {
        let changed = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some() {
                return false;
            }
            if !observe_current_generation(&mut state, selection_generation) {
                return false;
            }
            let removable = matches!(
                state.routes.get(route_id),
                Some(Entry::Striking { selection_generation: generation, .. }
                    | Entry::Pending { selection_generation: generation, .. })
                    if *generation == selection_generation
            ) || matches!(
                state.routes.get(route_id),
                Some(Entry::Tripping {
                    selection_generation: generation,
                    reserved: false,
                    ..
                }) if *generation == selection_generation
            );
            removable && state.routes.remove(route_id).is_some()
        };
        if changed {
            self.publish(route_id);
        }
        changed
    }

    #[must_use]
    pub fn record_ordinary_failure(
        &self,
        route_id: &RouteId,
        selection_generation: u64,
        failure: HealthFailureClass,
        has_configured_successor: bool,
        may_trip: bool,
    ) -> StrikeResult {
        self.record_ordinary_failure_inner(
            route_id,
            selection_generation,
            failure,
            has_configured_successor,
            may_trip,
            None,
        )
    }

    #[must_use]
    pub fn record_ordinary_failure_for_attempt(
        &self,
        route_id: &RouteId,
        selection_generation: u64,
        failure: HealthFailureClass,
        has_configured_successor: bool,
        may_trip: bool,
        source_attempt: HealthAttemptRef,
    ) -> StrikeResult {
        self.record_ordinary_failure_inner(
            route_id,
            selection_generation,
            failure,
            has_configured_successor,
            may_trip,
            Some(source_attempt),
        )
    }

    fn record_ordinary_failure_inner(
        &self,
        route_id: &RouteId,
        selection_generation: u64,
        failure: HealthFailureClass,
        has_configured_successor: bool,
        may_trip: bool,
        source_attempt: Option<HealthAttemptRef>,
    ) -> StrikeResult {
        if !has_configured_successor {
            return StrikeResult::Ignored;
        }
        let (result, changed) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some() {
                return StrikeResult::Stale;
            }
            if !observe_current_generation(&mut state, selection_generation) {
                return StrikeResult::Stale;
            }
            let current = state.routes.remove(route_id);
            match current {
                None => {
                    let version = state.version();
                    state.routes.insert(
                        route_id.clone(),
                        Entry::Striking {
                            failure_count: 1,
                            last_failure: failure,
                            version,
                            selection_generation,
                        },
                    );
                    (StrikeResult::BelowThreshold { failure_count: 1 }, true)
                }
                Some(entry) if entry.generation() != selection_generation => {
                    state.routes.insert(route_id.clone(), entry);
                    (StrikeResult::Stale, false)
                }
                Some(Entry::Striking { failure_count, .. })
                    if failure_count < FAILURE_THRESHOLD - 1 =>
                {
                    let next = failure_count + 1;
                    let version = state.version();
                    state.routes.insert(
                        route_id.clone(),
                        Entry::Striking {
                            failure_count: next,
                            last_failure: failure,
                            version,
                            selection_generation,
                        },
                    );
                    (
                        StrikeResult::BelowThreshold {
                            failure_count: next,
                        },
                        true,
                    )
                }
                Some(Entry::Striking { .. }) if may_trip => {
                    let lease_id = state.lease_id();
                    let version = state.version();
                    state.routes.insert(
                        route_id.clone(),
                        Entry::Tripping {
                            lease_id,
                            failure,
                            reserved: false,
                            version,
                            selection_generation,
                            source_attempt: source_attempt.clone(),
                        },
                    );
                    (
                        StrikeResult::TripAcquired(TripLease {
                            route_id: route_id.clone(),
                            lease_id,
                            version,
                            selection_generation,
                            failure,
                            source_attempt,
                        }),
                        true,
                    )
                }
                Some(Entry::Striking { .. }) => {
                    let version = state.version();
                    state.routes.insert(
                        route_id.clone(),
                        Entry::Pending {
                            failure,
                            activation_retry_at: None,
                            version,
                            selection_generation,
                            source_attempt,
                        },
                    );
                    (StrikeResult::Pending, true)
                }
                Some(entry @ Entry::Tripping { .. }) => {
                    state.routes.insert(route_id.clone(), entry);
                    (StrikeResult::TripBusy, false)
                }
                Some(entry @ Entry::Pending { .. }) => {
                    state.routes.insert(route_id.clone(), entry);
                    (StrikeResult::Pending, false)
                }
                Some(entry @ (Entry::Open { .. } | Entry::HalfOpen { .. })) => {
                    state.routes.insert(route_id.clone(), entry);
                    (StrikeResult::Ignored, false)
                }
            }
        };
        if changed {
            self.publish(route_id);
        }
        result
    }

    #[must_use]
    pub fn claim_pending(
        &self,
        route_id: &RouteId,
        selection_generation: u64,
    ) -> Option<TripLease> {
        let now = self.clock.now();
        let lease = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some() {
                return None;
            }
            if !observe_current_generation(&mut state, selection_generation) {
                return None;
            }
            let Some(Entry::Pending {
                failure,
                activation_retry_at,
                selection_generation: generation,
                source_attempt,
                ..
            }) = state.routes.get(route_id)
            else {
                return None;
            };
            if *generation != selection_generation
                || activation_retry_at.is_some_and(|retry_at| retry_at > now)
            {
                return None;
            }
            let failure = *failure;
            let source_attempt = source_attempt.clone();
            let lease_id = state.lease_id();
            let version = state.version();
            state.routes.insert(
                route_id.clone(),
                Entry::Tripping {
                    lease_id,
                    failure,
                    reserved: false,
                    version,
                    selection_generation,
                    source_attempt: source_attempt.clone(),
                },
            );
            Some(TripLease {
                route_id: route_id.clone(),
                lease_id,
                version,
                selection_generation,
                failure,
                source_attempt,
            })
        };
        if lease.is_some() {
            self.publish(route_id);
        }
        lease
    }

    #[must_use]
    pub fn reserve_trip(&self, lease: &TripLease) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.activation_reservation.is_some()
            || state.health_generation != Some(lease.selection_generation)
        {
            return false;
        }
        let Some(Entry::Tripping {
            lease_id,
            version,
            selection_generation,
            reserved,
            ..
        }) = state.routes.get_mut(&lease.route_id)
        else {
            return false;
        };
        if *lease_id != lease.lease_id
            || *version != lease.version
            || *selection_generation != lease.selection_generation
            || *reserved
        {
            return false;
        }
        *reserved = true;
        true
    }

    pub fn release_trip(&self, lease: &TripLease, persistence_failed: bool) -> bool {
        let now = self.clock.now();
        let changed = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some()
                || state.health_generation != Some(lease.selection_generation)
                || !trip_matches(state.routes.get(&lease.route_id), lease)
            {
                false
            } else {
                let version = state.version();
                state.routes.insert(
                    lease.route_id.clone(),
                    Entry::Pending {
                        failure: lease.failure,
                        activation_retry_at: persistence_failed
                            .then_some(now + ACTIVATION_WRITE_RETRY_DELAY),
                        version,
                        selection_generation: lease.selection_generation,
                        source_attempt: lease.source_attempt.clone(),
                    },
                );
                true
            }
        };
        if changed {
            self.publish(&lease.route_id);
        }
        changed
    }

    pub fn discard_trip(&self, lease: &TripLease) -> bool {
        let changed = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.activation_reservation.is_none()
                && state.health_generation == Some(lease.selection_generation)
                && trip_matches(state.routes.get(&lease.route_id), lease)
                && state.routes.remove(&lease.route_id).is_some()
        };
        if changed {
            self.publish(&lease.route_id);
        }
        changed
    }

    #[must_use]
    pub fn try_acquire_probe(
        &self,
        candidates: &[RouteId],
        selection_generation: u64,
    ) -> ProbeLeaseResult {
        let now = self.clock.now();
        let acquired = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some() {
                return ProbeLeaseResult::Busy;
            }
            if !observe_current_generation(&mut state, selection_generation) {
                return ProbeLeaseResult::NoneReady;
            }
            if state.active_probe.is_some() {
                return ProbeLeaseResult::Busy;
            }
            let candidate = candidates.iter().find_map(|route_id| {
                let Entry::Open {
                    retry_at,
                    backoff_step,
                    recovery_successes,
                    origin,
                    last_failure,
                    selection_generation: generation,
                    ..
                } = state.routes.get(route_id)?
                else {
                    return None;
                };
                (*generation == selection_generation && *retry_at <= now).then(|| {
                    (
                        route_id.clone(),
                        *backoff_step,
                        *recovery_successes,
                        *origin,
                        *last_failure,
                    )
                })
            });
            let Some((route_id, backoff_step, recovery_successes, origin, last_failure)) =
                candidate
            else {
                return ProbeLeaseResult::NoneReady;
            };
            let lease_id = state.lease_id();
            let version = state.version();
            state.routes.insert(
                route_id.clone(),
                Entry::HalfOpen {
                    lease_id,
                    backoff_step,
                    recovery_successes,
                    origin,
                    last_failure,
                    version,
                    selection_generation,
                },
            );
            state.active_probe = Some((route_id.clone(), lease_id, version, selection_generation));
            ProbeLease {
                route_id,
                lease_id,
                version,
                selection_generation,
                backoff_step,
                recovery_successes,
                origin,
            }
        };
        self.publish(&acquired.route_id);
        ProbeLeaseResult::Acquired(acquired)
    }

    #[must_use]
    pub fn try_acquire_later_probe(
        &self,
        source: &TripLease,
        candidate: &RouteId,
    ) -> LaterProbeLeaseResult {
        let now = self.clock.now();
        let acquired = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some() {
                return LaterProbeLeaseResult::Busy;
            }
            if state.health_generation != Some(source.selection_generation) {
                return LaterProbeLeaseResult::Stale;
            }
            if !reserved_trip_matches(state.routes.get(&source.route_id), source) {
                return LaterProbeLeaseResult::Stale;
            }
            if state.active_probe.is_some() {
                return LaterProbeLeaseResult::Busy;
            }
            let Some(Entry::Open {
                retry_at,
                backoff_step,
                recovery_successes,
                origin,
                last_failure,
                selection_generation,
                ..
            }) = state.routes.get(candidate)
            else {
                return LaterProbeLeaseResult::NotReady;
            };
            if *selection_generation != source.selection_generation || *retry_at > now {
                return LaterProbeLeaseResult::NotReady;
            }
            let backoff_step = *backoff_step;
            let recovery_successes = *recovery_successes;
            let origin = *origin;
            let last_failure = *last_failure;
            let source_version = state.version();
            state.routes.insert(
                source.route_id.clone(),
                Entry::Pending {
                    failure: source.failure,
                    activation_retry_at: None,
                    version: source_version,
                    selection_generation: source.selection_generation,
                    source_attempt: source.source_attempt.clone(),
                },
            );
            let lease_id = state.lease_id();
            let version = state.version();
            state.routes.insert(
                candidate.clone(),
                Entry::HalfOpen {
                    lease_id,
                    backoff_step,
                    recovery_successes,
                    origin,
                    last_failure,
                    version,
                    selection_generation: source.selection_generation,
                },
            );
            state.active_probe = Some((
                candidate.clone(),
                lease_id,
                version,
                source.selection_generation,
            ));
            (
                PendingProof {
                    route_id: source.route_id.clone(),
                    version: source_version,
                    selection_generation: source.selection_generation,
                    failure: source.failure,
                    source_attempt: source.source_attempt.clone(),
                },
                ProbeLease {
                    route_id: candidate.clone(),
                    lease_id,
                    version,
                    selection_generation: source.selection_generation,
                    backoff_step,
                    recovery_successes,
                    origin,
                },
            )
        };
        self.publish(&source.route_id);
        self.publish(candidate);
        LaterProbeLeaseResult::Acquired {
            source: acquired.0,
            probe: acquired.1,
        }
    }

    #[must_use]
    pub fn complete_probe_positive(&self, lease: &ProbeLease) -> ProbeCompletion {
        let now = self.clock.now();
        let (completion, changed) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some() || !probe_matches(&state, lease) {
                (ProbeCompletion::Stale, false)
            } else if lease.recovery_successes == 0 {
                let last_failure = match state.routes.get(&lease.route_id) {
                    Some(Entry::HalfOpen { last_failure, .. }) => *last_failure,
                    _ => None,
                };
                let version = state.version();
                state.routes.insert(
                    lease.route_id.clone(),
                    Entry::Open {
                        retry_at: now,
                        backoff_step: lease.backoff_step,
                        recovery_successes: 1,
                        origin: lease.origin,
                        last_failure,
                        version,
                        selection_generation: lease.selection_generation,
                    },
                );
                clear_probe_slot(&mut state, lease);
                (ProbeCompletion::FirstPositive, true)
            } else {
                (ProbeCompletion::SecondPositiveReady, false)
            }
        };
        if changed {
            self.publish(&lease.route_id);
        }
        completion
    }

    pub fn complete_probe_failure(
        &self,
        lease: &ProbeLease,
        failure: HealthFailureClass,
    ) -> ProbeCompletion {
        let now = self.clock.now();
        self.finish_probe_open(
            lease,
            RecoveryOrigin::ProviderFailure,
            Some(failure),
            0,
            lease.backoff_step.saturating_add(1).min(3),
            now,
            false,
        )
    }

    pub fn complete_probe_neutral(&self, lease: &ProbeLease) -> ProbeCompletion {
        self.defer_probe(lease, false)
    }

    pub fn complete_probe_cancelled(&self, lease: &ProbeLease) -> ProbeCompletion {
        self.defer_probe(lease, false)
    }

    pub fn complete_probe_committed(&self, lease: &ProbeLease) -> ProbeCompletion {
        self.defer_probe(lease, false)
    }

    pub fn complete_probe_activation_failure(&self, lease: &ProbeLease) -> ProbeCompletion {
        let now = self.clock.now();
        self.finish_probe_open(lease, lease.origin, None, 1, lease.backoff_step, now, true)
    }

    pub fn discard_probe(&self, lease: &ProbeLease) -> ProbeCompletion {
        let changed = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some() || !probe_matches(&state, lease) {
                false
            } else {
                state.routes.remove(&lease.route_id);
                clear_probe_slot(&mut state, lease);
                true
            }
        };
        if changed {
            self.publish(&lease.route_id);
            ProbeCompletion::Applied
        } else {
            ProbeCompletion::Stale
        }
    }

    fn defer_probe(&self, lease: &ProbeLease, base_delay: bool) -> ProbeCompletion {
        let now = self.clock.now();
        self.finish_probe_open(
            lease,
            lease.origin,
            None,
            lease.recovery_successes,
            lease.backoff_step,
            now,
            base_delay,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_probe_open(
        &self,
        lease: &ProbeLease,
        origin: RecoveryOrigin,
        failure: Option<HealthFailureClass>,
        recovery_successes: u8,
        backoff_step: u8,
        now: Duration,
        base_delay: bool,
    ) -> ProbeCompletion {
        let changed = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some() || !probe_matches(&state, lease) {
                false
            } else {
                let retained_failure =
                    failure.or_else(|| match state.routes.get(&lease.route_id) {
                        Some(Entry::HalfOpen { last_failure, .. }) => *last_failure,
                        _ => None,
                    });
                let version = state.version();
                let delay = if base_delay {
                    ACTIVATION_WRITE_RETRY_DELAY
                } else {
                    recovery_delay(backoff_step)
                };
                state.routes.insert(
                    lease.route_id.clone(),
                    Entry::Open {
                        retry_at: now + delay,
                        backoff_step,
                        recovery_successes,
                        origin,
                        last_failure: retained_failure,
                        version,
                        selection_generation: lease.selection_generation,
                    },
                );
                clear_probe_slot(&mut state, lease);
                true
            }
        };
        if changed {
            self.publish(&lease.route_id);
            ProbeCompletion::Applied
        } else {
            ProbeCompletion::Stale
        }
    }

    pub fn commit_advance(
        &self,
        source: &TripLease,
        target_route_id: &RouteId,
        new_selection_generation: u64,
        participants: &[RouteId],
        skipped: &[ActivatedSkipHealth],
    ) -> bool {
        let now = self.clock.now();
        let participant_set = participants.iter().cloned().collect::<HashSet<_>>();
        let mut changed_routes = HashSet::new();
        let applied = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.activation_reservation.is_none()
                && apply_advance(
                    &mut state,
                    source,
                    target_route_id,
                    new_selection_generation,
                    &participant_set,
                    skipped,
                    now,
                    &mut changed_routes,
                )
        };
        if applied {
            self.publish_many(changed_routes);
        }
        applied
    }

    pub fn commit_recovery(
        &self,
        target: &ProbeLease,
        new_selection_generation: u64,
        participants: &[RouteId],
    ) -> bool {
        let now = self.clock.now();
        let participant_set = participants.iter().cloned().collect::<HashSet<_>>();
        let mut changed_routes = HashSet::new();
        let applied = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.activation_reservation.is_none()
                && apply_recovery(
                    &mut state,
                    target,
                    new_selection_generation,
                    &participant_set,
                    now,
                    &mut changed_routes,
                )
        };
        if applied {
            self.publish_many(changed_routes);
        }
        applied
    }

    pub fn commit_advance_recovered(
        &self,
        source: &PendingProof,
        target: &ProbeLease,
        new_selection_generation: u64,
        participants: &[RouteId],
        skipped: &[ActivatedSkipHealth],
    ) -> bool {
        let now = self.clock.now();
        let participant_set = participants.iter().cloned().collect::<HashSet<_>>();
        let mut changed_routes = HashSet::new();
        let applied = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.activation_reservation.is_none()
                && apply_advance_recovered(
                    &mut state,
                    source,
                    target,
                    new_selection_generation,
                    &participant_set,
                    skipped,
                    now,
                    &mut changed_routes,
                )
        };
        if applied {
            self.publish_many(changed_routes);
        }
        applied
    }

    pub fn commit_activation(
        &self,
        reservation: &HealthActivationReservation,
        proof: &HealthActivationProof,
        target_route_id: &RouteId,
        new_selection_generation: u64,
        participants: &[RouteId],
        skipped: &[ActivatedSkipHealth],
    ) -> bool {
        let now = self.clock.now();
        let participant_set = participants.iter().cloned().collect::<HashSet<_>>();
        let mut changed_routes = HashSet::new();
        let applied = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation != Some(reservation.reservation_id) {
                return false;
            }
            let applied = match proof {
                HealthActivationProof::Advance { source } => apply_advance(
                    &mut state,
                    source,
                    target_route_id,
                    new_selection_generation,
                    &participant_set,
                    skipped,
                    now,
                    &mut changed_routes,
                ),
                HealthActivationProof::AdvanceRecovered { source, target }
                    if &target.route_id == target_route_id =>
                {
                    apply_advance_recovered(
                        &mut state,
                        source,
                        target,
                        new_selection_generation,
                        &participant_set,
                        skipped,
                        now,
                        &mut changed_routes,
                    )
                }
                HealthActivationProof::Recover { target }
                    if &target.route_id == target_route_id =>
                {
                    apply_recovery(
                        &mut state,
                        target,
                        new_selection_generation,
                        &participant_set,
                        now,
                        &mut changed_routes,
                    )
                }
                HealthActivationProof::AdvanceRecovered { .. }
                | HealthActivationProof::Recover { .. } => false,
            };
            state.activation_reservation = None;
            applied
        };
        if applied {
            self.publish_many(changed_routes);
        }
        applied
    }

    pub fn defer_pending_activation(&self, proof: &PendingProof) -> bool {
        let now = self.clock.now();
        let changed = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.activation_reservation.is_some()
                || state.health_generation != Some(proof.selection_generation)
                || !pending_matches(state.routes.get(&proof.route_id), proof)
            {
                false
            } else {
                let version = state.version();
                state.routes.insert(
                    proof.route_id.clone(),
                    Entry::Pending {
                        failure: proof.failure,
                        activation_retry_at: Some(now + ACTIVATION_WRITE_RETRY_DELAY),
                        version,
                        selection_generation: proof.selection_generation,
                        source_attempt: proof.source_attempt.clone(),
                    },
                );
                true
            }
        };
        if changed {
            self.publish(&proof.route_id);
        }
        changed
    }

    pub fn invalidate_route_and_rebase(&self, route_id: &RouteId, participants: &[RouteId]) -> u64 {
        let now = self.clock.now();
        let participant_set = participants.iter().cloned().collect::<HashSet<_>>();
        let mut changed_routes = HashSet::new();
        let new_selection_generation;
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.activation_reservation = None;
            new_selection_generation = next_health_generation(&state);
            if state.routes.remove(route_id).is_some() {
                changed_routes.insert(route_id.clone());
            }
            if state
                .active_probe
                .as_ref()
                .is_some_and(|(active, ..)| active == route_id)
            {
                state.active_probe = None;
            }
            rebase_entries(
                &mut state,
                &participant_set,
                new_selection_generation,
                now,
                &mut changed_routes,
            );
            state.health_generation = Some(new_selection_generation);
        }
        self.publish_many(changed_routes);
        new_selection_generation
    }

    pub fn advance_generation_and_clear(&self) -> u64 {
        let (route_ids, generation) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let route_ids = state.routes.keys().cloned().collect::<Vec<_>>();
            state.routes.clear();
            state.active_probe = None;
            state.activation_reservation = None;
            let generation = next_health_generation(&state);
            state.health_generation = Some(generation);
            (route_ids, generation)
        };
        self.publish_many(route_ids);
        generation
    }

    pub fn clear_to_generation(&self, generation: u64) {
        let route_ids = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let route_ids = state.routes.keys().cloned().collect::<Vec<_>>();
            state.routes.clear();
            state.active_probe = None;
            state.activation_reservation = None;
            state.health_generation = Some(generation);
            route_ids
        };
        self.publish_many(route_ids);
    }

    fn publish(&self, route_id: &RouteId) {
        self.changes
            .route_health_changed(route_id.clone(), self.snapshot(route_id));
    }

    fn publish_many(&self, route_ids: impl IntoIterator<Item = RouteId>) {
        for route_id in route_ids {
            self.publish(&route_id);
        }
    }
}

fn activation_proof_matches(
    state: &RegistryState,
    proof: &HealthActivationProof,
    current_route_id: &RouteId,
    target_route_id: &RouteId,
    skipped: &[ActivatedSkipHealth],
) -> bool {
    let selection_generation = match proof {
        HealthActivationProof::Advance { source } => {
            if &source.route_id != current_route_id
                || !reserved_trip_matches(state.routes.get(current_route_id), source)
                || !closed_target_matches(
                    state.routes.get(target_route_id),
                    source.selection_generation,
                )
            {
                return false;
            }
            source.selection_generation
        }
        HealthActivationProof::AdvanceRecovered { source, target } => {
            if &source.route_id != current_route_id
                || &target.route_id != target_route_id
                || !pending_matches(state.routes.get(current_route_id), source)
                || !probe_matches(state, target)
                || target.recovery_successes != 1
            {
                return false;
            }
            source.selection_generation
        }
        HealthActivationProof::Recover { target } => {
            if &target.route_id != target_route_id
                || !probe_matches(state, target)
                || target.recovery_successes != 1
            {
                return false;
            }
            target.selection_generation
        }
    };
    state.health_generation == Some(selection_generation)
        && skipped.iter().all(|skip| match skip.kind {
            ActivatedSkipKind::ModelFallbackExcluded => true,
            ActivatedSkipKind::HealthUnavailable => {
                state.routes.get(&skip.route_id).is_some_and(|entry| {
                    entry.generation() == selection_generation
                        && matches!(
                            entry,
                            Entry::Tripping { .. }
                                | Entry::Pending { .. }
                                | Entry::Open { .. }
                                | Entry::HalfOpen { .. }
                        )
                })
            }
        })
}

#[allow(clippy::too_many_arguments)]
fn apply_advance(
    state: &mut RegistryState,
    source: &TripLease,
    target_route_id: &RouteId,
    new_selection_generation: u64,
    participants: &HashSet<RouteId>,
    skipped: &[ActivatedSkipHealth],
    now: Duration,
    changed_routes: &mut HashSet<RouteId>,
) -> bool {
    if !reserved_trip_matches(state.routes.get(&source.route_id), source)
        || state.health_generation != Some(source.selection_generation)
        || new_selection_generation != source.selection_generation.saturating_add(1)
    {
        return false;
    }
    rebase_entries(
        state,
        participants,
        new_selection_generation,
        now,
        changed_routes,
    );
    let version = state.version();
    state.routes.insert(
        source.route_id.clone(),
        Entry::Open {
            retry_at: now + recovery_delay(0),
            backoff_step: 0,
            recovery_successes: 0,
            origin: RecoveryOrigin::ProviderFailure,
            last_failure: Some(source.failure),
            version,
            selection_generation: new_selection_generation,
        },
    );
    changed_routes.insert(source.route_id.clone());
    for skipped_route in skipped {
        apply_activated_skip(
            state,
            skipped_route,
            new_selection_generation,
            now,
            changed_routes,
        );
    }
    if let Some(Entry::HalfOpen { .. }) = state.routes.get(target_route_id) {
        state.routes.remove(target_route_id);
        changed_routes.insert(target_route_id.clone());
    }
    state.health_generation = Some(new_selection_generation);
    true
}

fn apply_recovery(
    state: &mut RegistryState,
    target: &ProbeLease,
    new_selection_generation: u64,
    participants: &HashSet<RouteId>,
    now: Duration,
    changed_routes: &mut HashSet<RouteId>,
) -> bool {
    if !probe_matches(state, target)
        || target.recovery_successes != 1
        || new_selection_generation != target.selection_generation.saturating_add(1)
    {
        return false;
    }
    clear_probe_slot(state, target);
    state.routes.remove(&target.route_id);
    changed_routes.insert(target.route_id.clone());
    rebase_entries(
        state,
        participants,
        new_selection_generation,
        now,
        changed_routes,
    );
    state.health_generation = Some(new_selection_generation);
    true
}

#[allow(clippy::too_many_arguments)]
fn apply_advance_recovered(
    state: &mut RegistryState,
    source: &PendingProof,
    target: &ProbeLease,
    new_selection_generation: u64,
    participants: &HashSet<RouteId>,
    skipped: &[ActivatedSkipHealth],
    now: Duration,
    changed_routes: &mut HashSet<RouteId>,
) -> bool {
    if !pending_matches(state.routes.get(&source.route_id), source)
        || !probe_matches(state, target)
        || target.recovery_successes != 1
        || source.selection_generation != target.selection_generation
        || new_selection_generation != source.selection_generation.saturating_add(1)
    {
        return false;
    }
    clear_probe_slot(state, target);
    state.routes.remove(&target.route_id);
    rebase_entries(
        state,
        participants,
        new_selection_generation,
        now,
        changed_routes,
    );
    let version = state.version();
    state.routes.insert(
        source.route_id.clone(),
        Entry::Open {
            retry_at: now + recovery_delay(0),
            backoff_step: 0,
            recovery_successes: 0,
            origin: RecoveryOrigin::ProviderFailure,
            last_failure: Some(source.failure),
            version,
            selection_generation: new_selection_generation,
        },
    );
    changed_routes.insert(source.route_id.clone());
    changed_routes.insert(target.route_id.clone());
    for skipped_route in skipped {
        apply_activated_skip(
            state,
            skipped_route,
            new_selection_generation,
            now,
            changed_routes,
        );
    }
    state.health_generation = Some(new_selection_generation);
    true
}

fn trip_matches(entry: Option<&Entry>, lease: &TripLease) -> bool {
    matches!(
        entry,
        Some(Entry::Tripping {
            lease_id,
            version,
            selection_generation,
            failure,
            source_attempt,
            ..
        }) if *lease_id == lease.lease_id
            && *version == lease.version
            && *selection_generation == lease.selection_generation
            && *failure == lease.failure
            && source_attempt == &lease.source_attempt
    )
}

fn closed_target_matches(entry: Option<&Entry>, generation: u64) -> bool {
    entry.is_none()
        || matches!(
            entry,
            Some(Entry::Striking {
                selection_generation,
                ..
            }) if *selection_generation == generation
        )
}

fn observe_current_generation(state: &mut RegistryState, generation: u64) -> bool {
    if let Some(current) = state.health_generation {
        current == generation
    } else {
        state.health_generation = Some(generation);
        true
    }
}

fn next_health_generation(state: &RegistryState) -> u64 {
    state.health_generation.unwrap_or(0).wrapping_add(1)
}

fn reserved_trip_matches(entry: Option<&Entry>, lease: &TripLease) -> bool {
    trip_matches(entry, lease) && matches!(entry, Some(Entry::Tripping { reserved: true, .. }))
}

fn pending_matches(entry: Option<&Entry>, proof: &PendingProof) -> bool {
    matches!(
        entry,
        Some(Entry::Pending {
            failure,
            version,
            selection_generation,
            source_attempt,
            ..
        }) if *failure == proof.failure
            && *version == proof.version
            && *selection_generation == proof.selection_generation
            && source_attempt == &proof.source_attempt
    )
}

fn probe_matches(state: &RegistryState, lease: &ProbeLease) -> bool {
    state.health_generation == Some(lease.selection_generation)
        && matches!(
            state.routes.get(&lease.route_id),
            Some(Entry::HalfOpen {
                lease_id,
                version,
                selection_generation,
                backoff_step,
                recovery_successes,
                origin,
                ..
            }) if *lease_id == lease.lease_id
                && *version == lease.version
                && *selection_generation == lease.selection_generation
                && *backoff_step == lease.backoff_step
                && *recovery_successes == lease.recovery_successes
                && *origin == lease.origin
        )
        && state
            .active_probe
            .as_ref()
            .is_some_and(|(route_id, lease_id, version, generation)| {
                route_id == &lease.route_id
                    && *lease_id == lease.lease_id
                    && *version == lease.version
                    && *generation == lease.selection_generation
            })
}

fn clear_probe_slot(state: &mut RegistryState, lease: &ProbeLease) {
    if state
        .active_probe
        .as_ref()
        .is_some_and(|(route_id, lease_id, version, generation)| {
            route_id == &lease.route_id
                && *lease_id == lease.lease_id
                && *version == lease.version
                && *generation == lease.selection_generation
        })
    {
        state.active_probe = None;
    }
}

fn recovery_delay(step: u8) -> Duration {
    RECOVERY_BACKOFF[usize::from(step.min(3))]
}

fn remaining_seconds(deadline: Duration, now: Duration) -> u16 {
    let remaining = deadline.saturating_sub(now);
    let rounded = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    u16::try_from(rounded.min(600)).unwrap_or(600)
}

fn project(entry: &Entry, now: Duration) -> RouteHealthSnapshot {
    match entry {
        Entry::Striking { failure_count, .. } => RouteHealthSnapshot::Striking {
            failure_count: (*failure_count).clamp(1, FAILURE_THRESHOLD - 1),
        },
        Entry::Tripping { .. } => RouteHealthSnapshot::Switching,
        Entry::Pending {
            activation_retry_at,
            ..
        } => RouteHealthSnapshot::SwitchPending {
            retry_after_seconds: activation_retry_at
                .map(|retry_at| remaining_seconds(retry_at, now)),
        },
        Entry::Open {
            retry_at,
            recovery_successes,
            origin,
            ..
        } => RouteHealthSnapshot::Open {
            origin: *origin,
            recovery_successes: (*recovery_successes).min(1),
            retry_after_seconds: remaining_seconds(*retry_at, now),
        },
        Entry::HalfOpen {
            recovery_successes, ..
        } => RouteHealthSnapshot::Probing {
            recovery_successes: (*recovery_successes).min(1),
        },
    }
}

fn rebase_entries(
    state: &mut RegistryState,
    participants: &HashSet<RouteId>,
    new_selection_generation: u64,
    now: Duration,
    changed_routes: &mut HashSet<RouteId>,
) {
    let route_ids = state.routes.keys().cloned().collect::<Vec<_>>();
    for route_id in route_ids {
        if !participants.contains(&route_id) {
            state.routes.remove(&route_id);
            changed_routes.insert(route_id.clone());
            if state
                .active_probe
                .as_ref()
                .is_some_and(|(active, ..)| active == &route_id)
            {
                state.active_probe = None;
            }
            continue;
        }
        let Some(entry) = state.routes.remove(&route_id) else {
            continue;
        };
        let rebased = match entry {
            Entry::Striking {
                failure_count,
                last_failure,
                ..
            } => Entry::Striking {
                failure_count,
                last_failure,
                version: state.version(),
                selection_generation: new_selection_generation,
            },
            Entry::Pending {
                failure,
                activation_retry_at,
                source_attempt,
                ..
            } => Entry::Pending {
                failure,
                activation_retry_at,
                version: state.version(),
                selection_generation: new_selection_generation,
                source_attempt,
            },
            Entry::Tripping {
                failure,
                source_attempt,
                ..
            } => Entry::Pending {
                failure,
                activation_retry_at: None,
                version: state.version(),
                selection_generation: new_selection_generation,
                source_attempt,
            },
            Entry::Open {
                retry_at,
                backoff_step,
                recovery_successes,
                origin,
                last_failure,
                ..
            } => Entry::Open {
                retry_at,
                backoff_step,
                recovery_successes,
                origin,
                last_failure,
                version: state.version(),
                selection_generation: new_selection_generation,
            },
            Entry::HalfOpen {
                backoff_step,
                recovery_successes,
                origin,
                last_failure,
                ..
            } => Entry::Open {
                retry_at: now + recovery_delay(backoff_step),
                backoff_step,
                recovery_successes,
                origin,
                last_failure,
                version: state.version(),
                selection_generation: new_selection_generation,
            },
        };
        state.routes.insert(route_id.clone(), rebased);
        changed_routes.insert(route_id);
    }
    state.active_probe = None;
}

fn apply_activated_skip(
    state: &mut RegistryState,
    skipped: &ActivatedSkipHealth,
    new_selection_generation: u64,
    now: Duration,
    changed_routes: &mut HashSet<RouteId>,
) {
    let current = state.routes.remove(&skipped.route_id);
    let replacement = match (skipped.kind, current) {
        (ActivatedSkipKind::ModelFallbackExcluded, None | Some(Entry::Striking { .. })) => {
            Some(Entry::Open {
                retry_at: now + recovery_delay(0),
                backoff_step: 0,
                recovery_successes: 0,
                origin: RecoveryOrigin::ModelBypassed,
                last_failure: None,
                version: state.version(),
                selection_generation: new_selection_generation,
            })
        }
        (
            ActivatedSkipKind::HealthUnavailable,
            Some(Entry::Pending { failure, .. } | Entry::Tripping { failure, .. }),
        ) => Some(Entry::Open {
            retry_at: now + recovery_delay(0),
            backoff_step: 0,
            recovery_successes: 0,
            origin: RecoveryOrigin::ProviderFailure,
            last_failure: Some(failure),
            version: state.version(),
            selection_generation: new_selection_generation,
        }),
        (_, current) => current,
    };
    if let Some(replacement) = replacement {
        state.routes.insert(skipped.route_id.clone(), replacement);
    }
    changed_routes.insert(skipped.route_id.clone());
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[derive(Default)]
    struct TestClock(AtomicU64);

    impl TestClock {
        fn advance(&self, duration: Duration) {
            self.0.fetch_add(
                u64::try_from(duration.as_millis()).unwrap(),
                Ordering::AcqRel,
            );
        }
    }

    impl MonotonicClock for TestClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::Acquire))
        }
    }

    fn route(value: &str) -> RouteId {
        RouteId::from_string(value.to_owned())
    }

    fn registry(clock: Arc<TestClock>) -> RouteHealthRegistry {
        RouteHealthRegistry::new(clock, Arc::new(NoopHealthChangeSink))
    }

    #[test]
    fn five_shared_failures_trip_without_a_time_window_and_success_resets() {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock.clone());
        let route_id = route("route-a");
        for expected in 1..=4 {
            assert_eq!(
                registry.record_ordinary_failure(
                    &route_id,
                    7,
                    HealthFailureClass::Service,
                    true,
                    true,
                ),
                StrikeResult::BelowThreshold {
                    failure_count: expected,
                }
            );
            clock.advance(Duration::from_hours(24));
        }
        let StrikeResult::TripAcquired(lease) =
            registry.record_ordinary_failure(&route_id, 7, HealthFailureClass::Timeout, true, true)
        else {
            panic!("fifth failure must own the trip proof");
        };
        assert_eq!(
            registry.snapshot(&route_id),
            Some(RouteHealthSnapshot::Switching)
        );
        assert!(registry.reserve_trip(&lease));
        let target = route("route-b");
        assert!(registry.activation_proof_current(
            &HealthActivationProof::Advance {
                source: lease.clone(),
            },
            &route_id,
            &target,
            &[],
        ));
        assert!(registry.release_trip(&lease, false));
        assert!(!registry.activation_proof_current(
            &HealthActivationProof::Advance {
                source: lease.clone(),
            },
            &route_id,
            &target,
            &[],
        ));
        assert!(registry.record_ordinary_positive(&route_id, 7));
        assert_eq!(registry.snapshot(&route_id), None);
    }

    #[test]
    fn activation_reservation_freezes_health_until_commit_or_cancel() {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock);
        let source = route("route-a");
        let target = route("route-b");
        let unrelated = route("route-c");
        for _ in 0..4 {
            let _ = registry.record_ordinary_failure(
                &source,
                7,
                HealthFailureClass::Service,
                true,
                true,
            );
        }
        let StrikeResult::TripAcquired(lease) =
            registry.record_ordinary_failure(&source, 7, HealthFailureClass::Service, true, true)
        else {
            panic!("fifth failure must own the trip proof");
        };
        assert!(registry.reserve_trip(&lease));
        let proof = HealthActivationProof::Advance {
            source: lease.clone(),
        };
        let reservation = registry
            .begin_activation(&proof, &source, &target, &[])
            .expect("current proof reserves activation");

        assert_eq!(
            registry.record_ordinary_failure(
                &unrelated,
                7,
                HealthFailureClass::Service,
                true,
                true,
            ),
            StrikeResult::Stale
        );
        assert!(!registry.release_trip(&lease, false));
        assert!(
            registry
                .begin_activation(&proof, &source, &target, &[])
                .is_none()
        );
        assert!(registry.commit_activation(
            &reservation,
            &proof,
            &target,
            8,
            &[source.clone(), target.clone(), unrelated],
            &[],
        ));
        assert_eq!(registry.health_generation(), 8);
        assert!(matches!(
            registry.snapshot(&source),
            Some(RouteHealthSnapshot::Open { .. })
        ));

        let cancelled_registry = RouteHealthRegistry::new(
            Arc::new(TestClock::default()),
            Arc::new(NoopHealthChangeSink),
        );
        for _ in 0..4 {
            let _ = cancelled_registry.record_ordinary_failure(
                &source,
                3,
                HealthFailureClass::Service,
                true,
                true,
            );
        }
        let StrikeResult::TripAcquired(lease) = cancelled_registry.record_ordinary_failure(
            &source,
            3,
            HealthFailureClass::Service,
            true,
            true,
        ) else {
            panic!("fifth failure must own the trip proof");
        };
        assert!(cancelled_registry.reserve_trip(&lease));
        let proof = HealthActivationProof::Advance {
            source: lease.clone(),
        };
        let reservation = cancelled_registry
            .begin_activation(&proof, &source, &target, &[])
            .expect("current proof reserves activation");
        assert!(cancelled_registry.cancel_activation(&reservation));
        assert!(cancelled_registry.release_trip(&lease, false));
    }

    #[test]
    fn no_successor_creates_no_health_and_spent_budget_keeps_pending_proof() {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock);
        let route = route("route-a");
        assert_eq!(
            registry.record_ordinary_failure(&route, 1, HealthFailureClass::Service, false, true,),
            StrikeResult::Ignored
        );
        assert_eq!(registry.snapshot(&route), None);
        for _ in 0..4 {
            let _ = registry.record_ordinary_failure(
                &route,
                1,
                HealthFailureClass::Service,
                true,
                false,
            );
        }
        assert_eq!(
            registry.record_ordinary_failure(&route, 1, HealthFailureClass::Service, true, false,),
            StrikeResult::Pending
        );
        assert_eq!(
            registry.snapshot(&route),
            Some(RouteHealthSnapshot::SwitchPending {
                retry_after_seconds: None,
            })
        );

        let attempt = HealthAttemptRef {
            request_id: "threshold-owner".to_owned(),
            attempt_index: 4,
        };
        let tracked = RouteId::from_string("route-tracked".to_owned());
        for _ in 0..4 {
            let _ = registry.record_ordinary_failure(
                &tracked,
                1,
                HealthFailureClass::Service,
                true,
                false,
            );
        }
        assert_eq!(
            registry.record_ordinary_failure_for_attempt(
                &tracked,
                1,
                HealthFailureClass::Service,
                true,
                false,
                attempt.clone(),
            ),
            StrikeResult::Pending
        );
        assert_eq!(
            registry
                .claim_pending(&tracked, 1)
                .expect("pending proof")
                .source_attempt,
            Some(attempt)
        );
    }

    #[test]
    fn cleared_registry_rejects_old_results_even_without_a_route_entry() {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock);
        let route_id = route("route-a");

        assert_eq!(
            registry
                .record_ordinary_failure(&route_id, 7, HealthFailureClass::Service, true, true,),
            StrikeResult::BelowThreshold { failure_count: 1 }
        );
        assert_eq!(registry.advance_generation_and_clear(), 8);
        assert_eq!(
            registry
                .record_ordinary_failure(&route_id, 7, HealthFailureClass::Timeout, true, true,),
            StrikeResult::Stale
        );
        assert!(!registry.record_ordinary_positive(&route_id, 7));
        assert_eq!(
            registry.candidate_health(&route_id, 7),
            CandidateHealth::Stale
        );
        assert_eq!(registry.snapshot(&route_id), None);
        assert_eq!(
            registry
                .record_ordinary_failure(&route_id, 8, HealthFailureClass::Timeout, true, true,),
            StrikeResult::BelowThreshold { failure_count: 1 }
        );
    }

    #[test]
    fn route_edit_invalidates_only_that_route_and_rebases_other_health() {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock);
        let edited = route("route-edited");
        let retained = route("route-retained");
        for route_id in [&edited, &retained] {
            assert_eq!(
                registry.record_ordinary_failure(
                    route_id,
                    3,
                    HealthFailureClass::Service,
                    true,
                    true,
                ),
                StrikeResult::BelowThreshold { failure_count: 1 }
            );
        }

        assert_eq!(
            registry.invalidate_route_and_rebase(&edited, &[edited.clone(), retained.clone()],),
            4
        );
        assert_eq!(registry.snapshot(&edited), None);
        assert_eq!(
            registry.snapshot(&retained),
            Some(RouteHealthSnapshot::Striking { failure_count: 1 })
        );
        assert_eq!(
            registry.record_ordinary_failure(&edited, 3, HealthFailureClass::Timeout, true, true,),
            StrikeResult::Stale
        );
        assert_eq!(
            registry
                .record_ordinary_failure(&retained, 4, HealthFailureClass::Timeout, true, true,),
            StrikeResult::BelowThreshold { failure_count: 2 }
        );
    }

    #[test]
    fn singleton_probe_slot_two_positives_and_capped_backoff_are_deterministic() {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock.clone());
        let a = route("route-a");
        let b = route("route-b");
        for route in [&a, &b] {
            for _ in 0..5 {
                let result = registry.record_ordinary_failure(
                    route,
                    1,
                    HealthFailureClass::Service,
                    true,
                    true,
                );
                if let StrikeResult::TripAcquired(lease) = result {
                    assert!(registry.reserve_trip(&lease));
                    assert!(registry.commit_advance(&lease, &b, 2, &[a.clone(), b.clone()], &[],));
                    break;
                }
            }
        }
        clock.advance(Duration::from_mins(1));
        let ProbeLeaseResult::Acquired(first) =
            registry.try_acquire_probe(std::slice::from_ref(&a), 2)
        else {
            panic!("first probe should be due");
        };
        assert_eq!(
            registry.try_acquire_probe(std::slice::from_ref(&b), 2),
            ProbeLeaseResult::Busy
        );
        assert_eq!(
            registry.complete_probe_positive(&first),
            ProbeCompletion::FirstPositive
        );
        let ProbeLeaseResult::Acquired(second) =
            registry.try_acquire_probe(std::slice::from_ref(&a), 2)
        else {
            panic!("second probe has no minimum spacing");
        };
        assert_eq!(second.recovery_successes, 1);
        assert_eq!(
            registry.complete_probe_positive(&second),
            ProbeCompletion::SecondPositiveReady
        );
        assert!(registry.commit_recovery(&second, 3, &[a.clone(), b.clone()],));
        assert_eq!(registry.snapshot(&a), None);
    }

    #[test]
    fn neutral_preserves_progress_and_failure_advances_backoff() {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock.clone());
        let a = route("route-a");
        let b = route("route-b");
        let mut trip = None;
        for _ in 0..5 {
            if let StrikeResult::TripAcquired(lease) =
                registry.record_ordinary_failure(&a, 1, HealthFailureClass::Service, true, true)
            {
                trip = Some(lease);
            }
        }
        let trip = trip.unwrap();
        assert!(registry.reserve_trip(&trip));
        assert!(registry.commit_advance(&trip, &b, 2, &[a.clone(), b.clone()], &[],));
        clock.advance(Duration::from_mins(1));
        let ProbeLeaseResult::Acquired(probe) =
            registry.try_acquire_probe(std::slice::from_ref(&a), 2)
        else {
            panic!("probe should be due");
        };
        assert_eq!(
            registry.complete_probe_neutral(&probe),
            ProbeCompletion::Applied
        );
        assert_eq!(
            registry.snapshot(&a),
            Some(RouteHealthSnapshot::Open {
                origin: RecoveryOrigin::ProviderFailure,
                recovery_successes: 0,
                retry_after_seconds: 60,
            })
        );
        clock.advance(Duration::from_mins(1));
        let ProbeLeaseResult::Acquired(probe) =
            registry.try_acquire_probe(std::slice::from_ref(&a), 2)
        else {
            panic!("probe should be due again");
        };
        assert_eq!(
            registry.complete_probe_failure(&probe, HealthFailureClass::Timeout),
            ProbeCompletion::Applied
        );
        assert_eq!(
            registry.snapshot(&a),
            Some(RouteHealthSnapshot::Open {
                origin: RecoveryOrigin::ProviderFailure,
                recovery_successes: 0,
                retry_after_seconds: 120,
            })
        );
    }

    #[test]
    fn later_probe_atomically_retains_source_and_second_positive_advances() {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock.clone());
        let a = route("route-a");
        let b = route("route-b");
        let c = route("route-c");

        let mut b_trip = None;
        for _ in 0..5 {
            if let StrikeResult::TripAcquired(lease) =
                registry.record_ordinary_failure(&b, 1, HealthFailureClass::Service, true, true)
            {
                b_trip = Some(lease);
            }
        }
        let b_trip = b_trip.expect("B reaches its forward threshold");
        assert!(registry.reserve_trip(&b_trip));
        assert!(registry.commit_advance(&b_trip, &c, 2, &[a.clone(), b.clone(), c.clone()], &[],));

        let mut a_trip = None;
        for _ in 0..5 {
            if let StrikeResult::TripAcquired(lease) =
                registry.record_ordinary_failure(&a, 2, HealthFailureClass::Timeout, true, true)
            {
                a_trip = Some(lease);
            }
        }
        let a_trip = a_trip.expect("A reaches its forward threshold");
        assert!(registry.reserve_trip(&a_trip));
        clock.advance(Duration::from_mins(1));

        let LaterProbeLeaseResult::Acquired {
            source: first_source,
            probe: first_probe,
        } = registry.try_acquire_later_probe(&a_trip, &b)
        else {
            panic!("expired later B should acquire the singleton probe");
        };
        assert_eq!(
            registry.snapshot(&a),
            Some(RouteHealthSnapshot::SwitchPending {
                retry_after_seconds: None,
            })
        );
        assert_eq!(
            registry.complete_probe_positive(&first_probe),
            ProbeCompletion::FirstPositive
        );
        assert!(!registry.defer_pending_activation(&PendingProof {
            version: first_source.version.saturating_add(1),
            ..first_source.clone()
        }));

        let second_source = registry
            .claim_pending(&a, 2)
            .expect("next request reclaims the source proof");
        assert!(registry.reserve_trip(&second_source));
        let LaterProbeLeaseResult::Acquired {
            source: second_source,
            probe: second_probe,
        } = registry.try_acquire_later_probe(&second_source, &b)
        else {
            panic!("second distinct request can immediately probe B");
        };
        assert_eq!(second_probe.recovery_successes, 1);
        assert_eq!(
            registry.complete_probe_positive(&second_probe),
            ProbeCompletion::SecondPositiveReady
        );
        assert!(registry.activation_proof_current(
            &HealthActivationProof::AdvanceRecovered {
                source: second_source.clone(),
                target: second_probe.clone(),
            },
            &a,
            &b,
            &[],
        ));
        assert!(registry.commit_advance_recovered(
            &second_source,
            &second_probe,
            3,
            &[a.clone(), b.clone(), c],
            &[],
        ));
        assert_eq!(registry.snapshot(&b), None);
        assert_eq!(
            registry.snapshot(&a),
            Some(RouteHealthSnapshot::Open {
                origin: RecoveryOrigin::ProviderFailure,
                recovery_successes: 0,
                retry_after_seconds: 60,
            })
        );
    }

    #[test]
    fn later_second_positive_persistence_failure_preserves_both_proofs() {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock.clone());
        let source = route("route-source");
        let target = route("route-target");
        let successor = route("route-successor");

        let mut target_trip = None;
        for _ in 0..5 {
            if let StrikeResult::TripAcquired(lease) = registry.record_ordinary_failure(
                &target,
                1,
                HealthFailureClass::Service,
                true,
                true,
            ) {
                target_trip = Some(lease);
            }
        }
        let target_trip = target_trip.unwrap();
        assert!(registry.reserve_trip(&target_trip));
        assert!(registry.commit_advance(
            &target_trip,
            &successor,
            2,
            &[source.clone(), target.clone(), successor.clone()],
            &[],
        ));
        let mut source_trip = None;
        for _ in 0..5 {
            if let StrikeResult::TripAcquired(lease) = registry.record_ordinary_failure(
                &source,
                2,
                HealthFailureClass::Timeout,
                true,
                true,
            ) {
                source_trip = Some(lease);
            }
        }
        let mut source_trip = source_trip.unwrap();
        assert!(registry.reserve_trip(&source_trip));
        clock.advance(Duration::from_mins(1));
        let LaterProbeLeaseResult::Acquired { probe, .. } =
            registry.try_acquire_later_probe(&source_trip, &target)
        else {
            panic!("first later probe");
        };
        assert_eq!(
            registry.complete_probe_positive(&probe),
            ProbeCompletion::FirstPositive
        );
        source_trip = registry.claim_pending(&source, 2).unwrap();
        assert!(registry.reserve_trip(&source_trip));
        let LaterProbeLeaseResult::Acquired {
            source: source_proof,
            probe,
        } = registry.try_acquire_later_probe(&source_trip, &target)
        else {
            panic!("second later probe");
        };
        assert_eq!(
            registry.complete_probe_positive(&probe),
            ProbeCompletion::SecondPositiveReady
        );
        assert_eq!(
            registry.complete_probe_activation_failure(&probe),
            ProbeCompletion::Applied
        );
        assert!(registry.defer_pending_activation(&source_proof));
        assert_eq!(
            registry.snapshot(&source),
            Some(RouteHealthSnapshot::SwitchPending {
                retry_after_seconds: Some(60),
            })
        );
        assert_eq!(
            registry.snapshot(&target),
            Some(RouteHealthSnapshot::Open {
                origin: RecoveryOrigin::ProviderFailure,
                recovery_successes: 1,
                retry_after_seconds: 60,
            })
        );
    }
}
