use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use axum::body::{Body, Bytes};
use http_body::{Frame, SizeHint};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalRequestActivityTransition {
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

struct ActivityState {
    count: usize,
    revision: u64,
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
                    pending_transitions: VecDeque::new(),
                    dispatching: false,
                }),
                sink,
            }),
        }
    }

    #[must_use]
    pub fn acquire(&self) -> Option<LogicalRequestActivityGuard> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = state.count;
        state.count = state.count.checked_add(1)?;
        state.revision = state.revision.saturating_add(1);
        if previous == 0 {
            let transition = LogicalRequestActivityTransition {
                active: true,
                count: state.count,
                revision: state.revision,
            };
            state.pending_transitions.push_back(transition);
        }
        let should_dispatch = !state.dispatching && !state.pending_transitions.is_empty();
        state.dispatching |= should_dispatch;
        drop(state);
        if should_dispatch {
            self.dispatch_transitions();
        }
        Some(LogicalRequestActivityGuard {
            tracker: Some(self.clone()),
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

    fn release(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(count) = state.count.checked_sub(1) else {
            return;
        };
        state.count = count;
        state.revision = state.revision.saturating_add(1);
        if count == 0 {
            let transition = LogicalRequestActivityTransition {
                active: false,
                count,
                revision: state.revision,
            };
            state.pending_transitions.push_back(transition);
        }
        let should_dispatch = !state.dispatching && !state.pending_transitions.is_empty();
        state.dispatching |= should_dispatch;
        drop(state);
        if should_dispatch {
            self.dispatch_transitions();
        }
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
}

impl LogicalRequestActivityGuard {
    fn release(&mut self) {
        if let Some(tracker) = self.tracker.take() {
            tracker.release();
        }
    }
}

impl Drop for LogicalRequestActivityGuard {
    fn drop(&mut self) {
        self.release();
    }
}

pub struct LogicalRequestActivityBody {
    inner: Pin<Box<Body>>,
    guard: Option<LogicalRequestActivityGuard>,
}

impl LogicalRequestActivityBody {
    pub fn new(inner: Body, guard: LogicalRequestActivityGuard) -> Self {
        Self {
            inner: Box::pin(inner),
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
        let result = self.inner.as_mut().poll_frame(cx);
        if matches!(result, Poll::Ready(None | Some(Err(_)))) {
            self.release();
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for LogicalRequestActivityBody {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use axum::body::to_bytes;

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
    fn tracker_notifies_only_zero_nonzero_transitions() {
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
            *sink.0.lock().expect("activity sink mutex"),
            [
                LogicalRequestActivityTransition {
                    active: true,
                    count: 1,
                    revision: 1,
                },
                LogicalRequestActivityTransition {
                    active: false,
                    count: 0,
                    revision: 4,
                },
            ]
        );
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
            *sink.transitions.lock().expect("transition sink mutex"),
            [
                LogicalRequestActivityTransition {
                    active: true,
                    count: 1,
                    revision: 1,
                },
                LogicalRequestActivityTransition {
                    active: false,
                    count: 0,
                    revision: 4,
                },
            ]
        );
    }
}
