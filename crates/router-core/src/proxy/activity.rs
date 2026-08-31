use std::{
    collections::{HashMap, HashSet, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};

use axum::body::{Body, Bytes};
use http_body::{Frame, SizeHint};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

const TOOL_HANDOFF_TIMEOUT: Duration = Duration::from_mins(15);
const MAX_WAITING_TURNS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogicalRequestActivityPhase {
    Idle,
    Live,
    Waiting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalRequestActivityTransition {
    pub phase: LogicalRequestActivityPhase,
    pub active: bool,
    pub count: usize,
    pub revision: u64,
}

pub trait LogicalRequestActivitySink: Send + Sync {
    fn activity_changed(&self, transition: LogicalRequestActivityTransition);
}

pub struct NoopLogicalRequestActivitySink;

impl LogicalRequestActivitySink for NoopLogicalRequestActivitySink {
    fn activity_changed(&self, _transition: LogicalRequestActivityTransition) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestActivityDisposition {
    Final,
    ClientToolHandoff,
    Failure,
}

#[derive(Default)]
struct RequestActivityReport {
    terminal: Option<RequestActivityDisposition>,
}

#[derive(Clone, Default)]
pub struct LogicalRequestActivityReporter {
    report: Arc<Mutex<RequestActivityReport>>,
}

impl LogicalRequestActivityReporter {
    pub fn mark_terminal(&self, disposition: RequestActivityDisposition) {
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if report.terminal.is_none() {
            report.terminal = Some(disposition);
        }
    }

    fn disposition(&self) -> RequestActivityDisposition {
        self.report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .terminal
            .unwrap_or(RequestActivityDisposition::Failure)
    }

    #[cfg(test)]
    pub(crate) fn reported_disposition_for_test(&self) -> Option<RequestActivityDisposition> {
        self.report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .terminal
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TurnKey([u8; 32]);

impl TurnKey {
    fn from_turn_id(turn_id: &str) -> Option<Self> {
        (!turn_id.trim().is_empty()).then(|| Self(Sha256::digest(turn_id.as_bytes()).into()))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EpochTurnKey {
    epoch: u64,
    turn: TurnKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TurnPhase {
    Live,
    Waiting,
}

struct TurnActivity {
    latest_generation: u64,
    latest_disposition: Option<RequestActivityDisposition>,
    live_generations: HashSet<u64>,
    phase: TurnPhase,
    wait_cancel: Option<oneshot::Sender<()>>,
}

struct ActivityState {
    count: usize,
    revision: u64,
    epoch: u64,
    next_generation: u64,
    waiting_turns: usize,
    turns: HashMap<EpochTurnKey, TurnActivity>,
    pending_transitions: VecDeque<LogicalRequestActivityTransition>,
    dispatching: bool,
}

struct LogicalRequestActivityInner {
    state: Mutex<ActivityState>,
    sink: Arc<dyn LogicalRequestActivitySink>,
}

#[derive(Clone)]
pub struct LogicalRequestActivityTracker {
    inner: Arc<LogicalRequestActivityInner>,
}

impl Default for LogicalRequestActivityTracker {
    fn default() -> Self {
        Self::new(Arc::new(NoopLogicalRequestActivitySink))
    }
}

impl LogicalRequestActivityTracker {
    #[must_use]
    pub fn new(sink: Arc<dyn LogicalRequestActivitySink>) -> Self {
        Self {
            inner: Arc::new(LogicalRequestActivityInner {
                state: Mutex::new(ActivityState {
                    count: 0,
                    revision: 0,
                    epoch: 0,
                    next_generation: 0,
                    waiting_turns: 0,
                    turns: HashMap::new(),
                    pending_transitions: VecDeque::new(),
                    dispatching: false,
                }),
                sink,
            }),
        }
    }

    #[must_use]
    pub fn acquire(&self) -> Option<LogicalRequestActivityGuard> {
        self.acquire_turn(None)
    }

    #[must_use]
    pub fn acquire_turn(&self, turn_id: Option<&str>) -> Option<LogicalRequestActivityGuard> {
        let requested_turn = turn_id.and_then(TurnKey::from_turn_id);
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = requested_turn
            .map(|turn| EpochTurnKey {
                epoch: state.epoch,
                turn,
            })
            .filter(|key| state.turns.contains_key(key) || state.waiting_turns < MAX_WAITING_TURNS);
        let previous_phase = Self::derived_phase(&state);
        let generation = if let Some(key) = key {
            state.next_generation = state.next_generation.checked_add(1)?;
            let generation = state.next_generation;
            let mut resumed_wait = false;
            if let Some(turn) = state.turns.get_mut(&key) {
                if turn.phase == TurnPhase::Waiting {
                    turn.phase = TurnPhase::Live;
                    if let Some(cancel) = turn.wait_cancel.take() {
                        let _ = cancel.send(());
                    }
                    resumed_wait = true;
                }
                turn.latest_generation = generation;
                turn.latest_disposition = None;
                turn.live_generations.insert(generation);
            } else {
                state.count = state.count.checked_add(1)?;
                state.turns.insert(
                    key,
                    TurnActivity {
                        latest_generation: generation,
                        latest_disposition: None,
                        live_generations: HashSet::from([generation]),
                        phase: TurnPhase::Live,
                        wait_cancel: None,
                    },
                );
            }
            if resumed_wait {
                state.waiting_turns = state.waiting_turns.saturating_sub(1);
            }
            Some(generation)
        } else {
            state.count = state.count.checked_add(1)?;
            None
        };
        Self::record_change(&mut state, previous_phase);
        let should_dispatch = Self::start_dispatch(&mut state);
        drop(state);
        if should_dispatch {
            self.dispatch_transitions();
        }
        Some(LogicalRequestActivityGuard {
            tracker: Some(self.clone()),
            key,
            generation,
            reporter: LogicalRequestActivityReporter::default(),
            timeout_runtime: tokio::runtime::Handle::try_current().ok(),
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> (usize, u64) {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.count, state.revision)
    }

    #[must_use]
    pub fn phase(&self) -> LogicalRequestActivityPhase {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::derived_phase(&state)
    }

    pub fn begin_new_epoch(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_phase = Self::derived_phase(&state);
        state.epoch = state.epoch.saturating_add(1);
        let waiting_keys = state
            .turns
            .iter()
            .filter_map(|(key, turn)| (turn.phase == TurnPhase::Waiting).then_some(*key))
            .collect::<Vec<_>>();
        for key in waiting_keys {
            if let Some(mut turn) = state.turns.remove(&key) {
                if let Some(cancel) = turn.wait_cancel.take() {
                    let _ = cancel.send(());
                }
                state.count = state.count.saturating_sub(1);
                state.waiting_turns = state.waiting_turns.saturating_sub(1);
            }
        }
        Self::record_change(&mut state, previous_phase);
        let should_dispatch = Self::start_dispatch(&mut state);
        drop(state);
        if should_dispatch {
            self.dispatch_transitions();
        }
    }

    fn release(
        &self,
        key: Option<EpochTurnKey>,
        generation: Option<u64>,
        disposition: RequestActivityDisposition,
        timeout_runtime: Option<tokio::runtime::Handle>,
    ) {
        let mut timeout = None;
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_phase = Self::derived_phase(&state);
        match (key, generation) {
            (Some(key), Some(generation)) => {
                let mut remove_turn = false;
                let mut entered_wait = None;
                let can_wait = key.epoch == state.epoch
                    && state.waiting_turns < MAX_WAITING_TURNS
                    && timeout_runtime.is_some();
                if let Some(turn) = state.turns.get_mut(&key) {
                    turn.live_generations.remove(&generation);
                    if generation == turn.latest_generation {
                        turn.latest_disposition = Some(disposition);
                    }
                    if turn.live_generations.is_empty() {
                        if turn.latest_disposition
                            == Some(RequestActivityDisposition::ClientToolHandoff)
                            && can_wait
                        {
                            let (cancel, cancelled) = oneshot::channel();
                            turn.phase = TurnPhase::Waiting;
                            turn.wait_cancel = Some(cancel);
                            entered_wait = Some(turn.latest_generation);
                            timeout = Some((key, turn.latest_generation, cancelled));
                        } else {
                            remove_turn = true;
                        }
                    }
                }
                if let Some(wait_generation) = entered_wait {
                    state.waiting_turns = state.waiting_turns.saturating_add(1);
                    debug_assert_eq!(timeout.as_ref().map(|entry| entry.1), Some(wait_generation));
                }
                if remove_turn && state.turns.remove(&key).is_some() {
                    state.count = state.count.saturating_sub(1);
                }
            }
            _ => state.count = state.count.saturating_sub(1),
        }
        Self::record_change(&mut state, previous_phase);
        let should_dispatch = Self::start_dispatch(&mut state);
        drop(state);
        if should_dispatch {
            self.dispatch_transitions();
        }
        if let (Some((key, generation, cancelled)), Some(timeout_runtime)) =
            (timeout, timeout_runtime)
        {
            self.spawn_wait_timeout(key, generation, cancelled, &timeout_runtime);
        }
    }

    fn spawn_wait_timeout(
        &self,
        key: EpochTurnKey,
        generation: u64,
        mut cancelled: oneshot::Receiver<()>,
        timeout_runtime: &tokio::runtime::Handle,
    ) {
        let tracker = self.clone();
        timeout_runtime.spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(TOOL_HANDOFF_TIMEOUT) => {
                    tracker.expire_wait(key, generation);
                }
                _ = &mut cancelled => {}
            }
        });
    }

    fn expire_wait(&self, key: EpochTurnKey, generation: u64) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_phase = Self::derived_phase(&state);
        let matches = state.turns.get(&key).is_some_and(|turn| {
            turn.phase == TurnPhase::Waiting && turn.latest_generation == generation
        });
        if matches {
            state.turns.remove(&key);
            state.waiting_turns = state.waiting_turns.saturating_sub(1);
            state.count = state.count.saturating_sub(1);
        }
        if !matches {
            return;
        }
        Self::record_change(&mut state, previous_phase);
        let should_dispatch = Self::start_dispatch(&mut state);
        drop(state);
        if should_dispatch {
            self.dispatch_transitions();
        }
    }

    fn derived_phase(state: &ActivityState) -> LogicalRequestActivityPhase {
        if state.count == 0 {
            LogicalRequestActivityPhase::Idle
        } else if state.count > state.waiting_turns {
            LogicalRequestActivityPhase::Live
        } else {
            LogicalRequestActivityPhase::Waiting
        }
    }

    fn record_change(state: &mut ActivityState, previous_phase: LogicalRequestActivityPhase) {
        state.revision = state.revision.saturating_add(1);
        let phase = Self::derived_phase(state);
        if previous_phase != phase {
            state
                .pending_transitions
                .push_back(LogicalRequestActivityTransition {
                    phase,
                    active: phase != LogicalRequestActivityPhase::Idle,
                    count: state.count,
                    revision: state.revision,
                });
        }
    }

    fn start_dispatch(state: &mut ActivityState) -> bool {
        let should_dispatch = !state.dispatching && !state.pending_transitions.is_empty();
        state.dispatching |= should_dispatch;
        should_dispatch
    }

    fn dispatch_transitions(&self) {
        loop {
            let transition = {
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let transition = state.pending_transitions.pop_front();
                if transition.is_none() {
                    state.dispatching = false;
                }
                transition
            };
            let Some(transition) = transition else {
                return;
            };
            let _ = catch_unwind(AssertUnwindSafe(|| {
                self.inner.sink.activity_changed(transition);
            }));
        }
    }
}

pub struct LogicalRequestActivityGuard {
    tracker: Option<LogicalRequestActivityTracker>,
    key: Option<EpochTurnKey>,
    generation: Option<u64>,
    reporter: LogicalRequestActivityReporter,
    timeout_runtime: Option<tokio::runtime::Handle>,
}

impl LogicalRequestActivityGuard {
    #[must_use]
    pub fn reporter(&self) -> LogicalRequestActivityReporter {
        self.reporter.clone()
    }

    fn release(&mut self) {
        if let Some(tracker) = self.tracker.take() {
            tracker.release(
                self.key,
                self.generation,
                self.reporter.disposition(),
                self.timeout_runtime.take(),
            );
        }
    }
}

impl Drop for LogicalRequestActivityGuard {
    fn drop(&mut self) {
        self.release();
    }
}

pub struct LogicalRequestActivityBody {
    inner: Option<Pin<Box<Body>>>,
    guard: Option<LogicalRequestActivityGuard>,
}

impl LogicalRequestActivityBody {
    pub fn new(inner: Body, guard: LogicalRequestActivityGuard) -> Self {
        Self {
            inner: Some(Box::pin(inner)),
            guard: Some(guard),
        }
    }

    fn release(&mut self) {
        self.guard.take();
    }
}

impl http_body::Body for LogicalRequestActivityBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let Some(inner) = self.inner.as_mut() else {
            return Poll::Ready(None);
        };
        let result = inner.as_mut().poll_frame(cx);
        if matches!(result, Poll::Ready(None | Some(Err(_)))) {
            self.release();
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        self.inner
            .as_ref()
            .is_none_or(http_body::Body::is_end_stream)
    }

    fn size_hint(&self) -> SizeHint {
        self.inner
            .as_ref()
            .map_or_else(SizeHint::new, http_body::Body::size_hint)
    }
}

impl Drop for LogicalRequestActivityBody {
    fn drop(&mut self) {
        // The observed response stream must publish its terminal disposition
        // before the outer logical activity lease consumes it.
        self.inner.take();
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use axum::body::to_bytes;
    use futures_util::{StreamExt, stream};

    use super::*;

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<LogicalRequestActivityTransition>>);

    impl LogicalRequestActivitySink for RecordingSink {
        fn activity_changed(&self, transition: LogicalRequestActivityTransition) {
            self.0.lock().expect("activity sink mutex").push(transition);
        }
    }

    #[derive(Default)]
    struct SnapshottingSink {
        tracker: OnceLock<LogicalRequestActivityTracker>,
        snapshots: Mutex<Vec<(usize, u64)>>,
    }

    #[derive(Default)]
    struct ReentrantSink {
        tracker: OnceLock<LogicalRequestActivityTracker>,
        transitions: Mutex<Vec<LogicalRequestActivityTransition>>,
        nested_guard: Mutex<Option<LogicalRequestActivityGuard>>,
    }

    impl LogicalRequestActivitySink for ReentrantSink {
        fn activity_changed(&self, transition: LogicalRequestActivityTransition) {
            self.transitions
                .lock()
                .expect("transition sink mutex")
                .push(transition);
            if transition.active && transition.revision == 1 {
                let nested = self
                    .tracker
                    .get()
                    .expect("tracker installed")
                    .acquire()
                    .expect("nested activity guard");
                *self.nested_guard.lock().expect("nested guard mutex") = Some(nested);
            }
        }
    }

    impl LogicalRequestActivitySink for SnapshottingSink {
        fn activity_changed(&self, _transition: LogicalRequestActivityTransition) {
            let snapshot = self.tracker.get().expect("tracker installed").snapshot();
            self.snapshots
                .lock()
                .expect("snapshot sink mutex")
                .push(snapshot);
        }
    }

    #[test]
    fn tracker_notifies_only_global_phase_transitions() {
        let sink = Arc::new(RecordingSink::default());
        let tracker = LogicalRequestActivityTracker::new(sink.clone());

        let first = tracker.acquire().expect("first activity guard");
        let second = tracker.acquire().expect("second activity guard");
        assert_eq!(tracker.snapshot(), (2, 2));
        drop(first);
        assert_eq!(tracker.snapshot(), (1, 3));
        drop(second);
        assert_eq!(tracker.snapshot(), (0, 4));
        assert_eq!(
            sink.0
                .lock()
                .expect("activity sink mutex")
                .iter()
                .map(|transition| transition.phase)
                .collect::<Vec<_>>(),
            [
                LogicalRequestActivityPhase::Live,
                LogicalRequestActivityPhase::Idle
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn same_turn_tool_handoff_continues_without_idle_transition() {
        let sink = Arc::new(RecordingSink::default());
        let tracker = LogicalRequestActivityTracker::new(sink.clone());
        let first = tracker.acquire_turn(Some("turn-a")).expect("first lease");
        first
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(first);
        assert_eq!(tracker.snapshot().0, 1);

        let continuation = tracker
            .acquire_turn(Some("turn-a"))
            .expect("continuation lease");
        drop(continuation);
        assert_eq!(tracker.snapshot().0, 0);
        assert_eq!(
            sink.0
                .lock()
                .expect("sink")
                .iter()
                .map(|transition| transition.phase)
                .collect::<Vec<_>>(),
            [
                LogicalRequestActivityPhase::Live,
                LogicalRequestActivityPhase::Waiting,
                LogicalRequestActivityPhase::Live,
                LogicalRequestActivityPhase::Idle,
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn abandoned_tool_handoff_expires_and_stale_timeout_cannot_clear_continuation() {
        let tracker = LogicalRequestActivityTracker::default();
        let first = tracker.acquire_turn(Some("turn-a")).expect("first lease");
        first
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(first);
        tokio::task::yield_now().await;
        tokio::time::advance(
            TOOL_HANDOFF_TIMEOUT
                .checked_sub(Duration::from_secs(1))
                .expect("handoff timeout exceeds one second"),
        )
        .await;
        tokio::task::yield_now().await;
        let continuation = tracker
            .acquire_turn(Some("turn-a"))
            .expect("continuation lease");
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::task::yield_now().await;
        assert_eq!(tracker.snapshot().0, 1);
        continuation
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(continuation);
        tokio::task::yield_now().await;
        tokio::time::advance(TOOL_HANDOFF_TIMEOUT).await;
        tokio::task::yield_now().await;
        assert_eq!(tracker.snapshot().0, 0);
    }

    #[tokio::test]
    async fn newest_same_turn_disposition_controls_post_live_state() {
        let tracker = LogicalRequestActivityTracker::default();
        let older = tracker.acquire_turn(Some("turn-a")).expect("older");
        let newer = tracker.acquire_turn(Some("turn-a")).expect("newer");
        older
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(older);
        drop(newer);
        assert_eq!(tracker.snapshot().0, 0);

        let older = tracker.acquire_turn(Some("turn-b")).expect("older");
        let newer = tracker.acquire_turn(Some("turn-b")).expect("newer");
        newer
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(newer);
        drop(older);
        assert_eq!(tracker.snapshot().0, 1);
    }

    #[test]
    fn tool_handoff_without_a_timeout_runtime_does_not_wait_forever() {
        let tracker = LogicalRequestActivityTracker::default();
        let lease = tracker
            .acquire_turn(Some("no-runtime"))
            .expect("request lease");
        lease
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(lease);

        assert_eq!(tracker.snapshot().0, 0);
    }

    #[test]
    fn missing_and_empty_turn_ids_remain_independent_request_leases() {
        let tracker = LogicalRequestActivityTracker::default();
        let missing = tracker.acquire_turn(None).expect("missing turn lease");
        let empty = tracker.acquire_turn(Some("  ")).expect("empty turn lease");
        assert_eq!(tracker.snapshot().0, 2);
        missing
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        empty
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(missing);
        drop(empty);
        assert_eq!(tracker.snapshot().0, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_capacity_falls_back_to_request_scoped_activity() {
        let tracker = LogicalRequestActivityTracker::default();
        for index in 0..MAX_WAITING_TURNS {
            let turn_id = format!("turn-{index}");
            let lease = tracker
                .acquire_turn(Some(&turn_id))
                .expect("bounded waiting lease");
            lease
                .reporter()
                .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
            drop(lease);
        }
        assert_eq!(tracker.snapshot().0, MAX_WAITING_TURNS);

        let overflow = tracker
            .acquire_turn(Some("capacity-overflow"))
            .expect("request-scoped overflow lease");
        overflow
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(overflow);
        assert_eq!(tracker.snapshot().0, MAX_WAITING_TURNS);

        let resumed = tracker
            .acquire_turn(Some("turn-0"))
            .expect("existing turn resumes at capacity");
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Live);
        resumed
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(resumed);
        assert_eq!(tracker.snapshot().0, MAX_WAITING_TURNS);
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Waiting);
    }

    #[tokio::test(start_paused = true)]
    async fn live_activity_wins_over_waiting_and_epoch_reset_clears_waits() {
        let sink = Arc::new(RecordingSink::default());
        let tracker = LogicalRequestActivityTracker::new(sink.clone());
        let waiting = tracker
            .acquire_turn(Some("waiting-turn"))
            .expect("waiting lease");
        waiting
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(waiting);
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Waiting);

        let live = tracker.acquire().expect("anonymous live lease");
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Live);
        drop(live);
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Waiting);

        tracker.begin_new_epoch();
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Idle);
        assert_eq!(tracker.snapshot().0, 0);
        assert_eq!(
            sink.0
                .lock()
                .expect("sink")
                .iter()
                .map(|transition| transition.phase)
                .collect::<Vec<_>>(),
            [
                LogicalRequestActivityPhase::Live,
                LogicalRequestActivityPhase::Waiting,
                LogicalRequestActivityPhase::Live,
                LogicalRequestActivityPhase::Waiting,
                LogicalRequestActivityPhase::Idle,
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn old_epoch_live_guard_cannot_wait_or_join_a_new_same_turn() {
        let tracker = LogicalRequestActivityTracker::default();
        let old = tracker.acquire_turn(Some("same-turn")).expect("old lease");
        tracker.begin_new_epoch();
        let current = tracker
            .acquire_turn(Some("same-turn"))
            .expect("current lease");
        assert_eq!(tracker.snapshot().0, 2);

        old.reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(old);
        assert_eq!(tracker.snapshot().0, 1);
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Live);

        current
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(current);
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Waiting);
        tracker.begin_new_epoch();
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Idle);
    }

    #[tokio::test]
    async fn body_read_error_releases_activity_once() {
        let sink = Arc::new(RecordingSink::default());
        let tracker = LogicalRequestActivityTracker::new(sink.clone());
        let guard = tracker.acquire().expect("activity guard");
        let stream = futures_util::stream::once(async {
            Err::<Bytes, std::io::Error>(std::io::Error::other("synthetic read failure"))
        });
        let body = Body::new(LogicalRequestActivityBody::new(
            Body::from_stream(stream),
            guard,
        ));

        assert!(to_bytes(body, 1024).await.is_err());
        assert_eq!(tracker.snapshot(), (0, 2));
        assert_eq!(sink.0.lock().expect("activity sink mutex").len(), 2);
    }

    #[tokio::test]
    async fn terminal_observer_drop_reports_before_outer_activity_release() {
        let tracker = LogicalRequestActivityTracker::default();
        let guard = tracker
            .acquire_turn(Some("terminal-drop-turn"))
            .expect("activity guard");
        let reporter = guard.reporter();
        let source = stream::once(async {
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            ))
        })
        .chain(stream::pending());
        let observed = crate::proxy::sse::observe_sse_stream(source, None, move |_| {
            reporter.mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        });
        let body = Body::new(LogicalRequestActivityBody::new(
            Body::from_stream(observed),
            guard,
        ));
        let mut downstream = body.into_data_stream();
        assert!(downstream.next().await.is_some());
        drop(downstream);

        assert_eq!(tracker.snapshot().0, 1);
    }

    #[test]
    fn transition_sink_can_read_the_tracker_without_reentering_the_state_lock() {
        let sink = Arc::new(SnapshottingSink::default());
        let tracker = LogicalRequestActivityTracker::new(sink.clone());
        sink.tracker.set(tracker.clone()).ok().expect("set tracker");
        let guard = tracker.acquire().expect("activity guard");
        drop(guard);
        assert_eq!(
            *sink.snapshots.lock().expect("snapshot sink mutex"),
            [(1, 1), (0, 2)]
        );
    }

    #[test]
    fn transition_sink_can_reenter_the_tracker_without_deadlocking_or_reordering() {
        let sink = Arc::new(ReentrantSink::default());
        let tracker = LogicalRequestActivityTracker::new(sink.clone());
        sink.tracker.set(tracker.clone()).ok().expect("set tracker");
        let outer = tracker.acquire().expect("outer activity guard");
        assert_eq!(tracker.snapshot(), (2, 2));
        drop(outer);
        assert_eq!(tracker.snapshot(), (1, 3));
        drop(sink.nested_guard.lock().expect("nested guard mutex").take());
        assert_eq!(tracker.snapshot(), (0, 4));
        assert_eq!(
            sink.transitions
                .lock()
                .expect("transition sink mutex")
                .len(),
            2
        );
    }
}
