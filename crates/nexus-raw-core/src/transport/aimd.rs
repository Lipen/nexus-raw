//! The AIMD worker limit: workers shrink on `Retry-After` and recover additively (§7).
//!
//! A flat worker pool keeps full pressure on a server that answers 429 with
//! `Retry-After`: every worker re-runs into the limit.
//! The controller here halves the in-flight capacity on every rate-limited
//! failure and, on a streak of successes, raises it one step at a time back to
//! the configured maximum.
//! Decrease is immediate: from that point the run acquires fewer permits.
//! Increase is gradual: one step per success streak, never overshooting the max.

use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Recovery slope: one extra worker per this many consecutive successes.
/// The 429 that armed the slope must be answered by a full streak of successes
/// before the first step: one success after a 429 never snaps the limit back.
const SUCCESS_PER_STEP: u32 = 4;

/// The pure, deterministic AIMD controller.
///
/// Events in: [`Aimd::on_retryable`] (a retryable failure carrying a
/// `Retry-After`), [`Aimd::on_success`].
/// State out: [`Aimd::limit`], the current worker capacity.
/// The unit tests pin the whole trajectory: halving, the floor, the linear
/// recovery, the cap at max and the hysteresis that keeps a single success
/// after a 429 from snapping the limit back up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Aimd {
    max: usize,
    limit: usize,
    /// The lowest limit the trajectory ever reached.
    min: usize,
    /// Consecutive successes since the last 429 (or since the start).
    streak: u32,
    /// Successes owed before one recovery step may fire.
    /// Armed to the full slope width on every 429.
    debt: u32,
}

impl Aimd {
    /// Start at the configured maximum: a run that never sees a 429 keeps it.
    #[must_use]
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max,
            limit: max,
            min: max,
            streak: 0,
            debt: 0,
        }
    }

    /// A retryable failure carrying a `Retry-After`: halve the capacity, forget the streak.
    /// The floor is 1: some transfer always goes on.
    pub(crate) fn on_retryable(&mut self) {
        self.limit = (self.limit / 2).max(1);
        self.min = self.min.min(self.limit);
        self.streak = 0;
        self.debt = SUCCESS_PER_STEP;
    }

    /// A success: count the streak, and take one recovery step per
    /// [`SUCCESS_PER_STEP`] consecutive successes, capped at the maximum.
    /// Never overshoots: the limit converges to `max` and stays there.
    pub(crate) fn on_success(&mut self) {
        self.streak = self.streak.saturating_add(1);
        if self.debt > 0 {
            self.debt -= 1;
        }
        if self.limit < self.max && self.debt == 0 {
            self.limit += 1;
            self.streak = 0;
        }
    }

    /// The current worker capacity.
    #[must_use]
    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    /// The lowest capacity the trajectory ever reached.
    #[must_use]
    pub(crate) fn min_limit(&self) -> usize {
        self.min
    }

}

/// One run's shared worker budget: the AIMD controller over a real semaphore.
///
/// Transfer loops acquire through this type instead of a raw `Semaphore`.
/// A decrease closes free permits immediately and records the still-held ones
/// as owed: each is surrendered when its task finishes.
/// An increase first cancels an owed permit, then grows the semaphore one
/// permit at a time, never above `max`.
#[derive(Debug)]
pub struct WorkerLimit {
    inner: Arc<Semaphore>,
    /// The AIMD state machine: halving on 429, linear recovery on successes.
    aimd: Mutex<Aimd>,
    /// Permits a decrease could not reclaim because tasks still held them.
    owed: Mutex<usize>,
}

impl WorkerLimit {
    /// A budget of `max` workers.
    #[must_use]
    pub fn new(max: usize) -> Self {
        Self {
            inner: Arc::new(Semaphore::new(max)),
            aimd: Mutex::new(Aimd::new(max)),
            owed: Mutex::new(0),
        }
    }

    /// Acquire one permit under the current limit.
    ///
    /// # Errors
    ///
    /// Returns the closed-semaphore message: the semaphore lives as long as the
    /// run, so closure is an internal invariant break.
    pub(crate) async fn acquire(&self) -> Result<OwnedSemaphorePermit, String> {
        self.inner
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("worker limit closed: {e}"))
    }

    /// A retryable failure carrying a `Retry-After`: halve the capacity now.
    ///
    /// Free permits are closed at once, so later acquires run under the reduced
    /// limit without waiting.
    /// Permits still held become owed and are surrendered as their tasks finish.
    pub(crate) fn on_retryable(&self) {
        let decrease = {
            let mut aimd = self.aimd.lock().expect("aimd lock");
            let old = aimd.limit();
            aimd.on_retryable();
            old - aimd.limit()
        };
        if decrease == 0 {
            return;
        }
        // Close every free permit the controller gave up.
        let mut taken = 0usize;
        for _ in 0..decrease {
            if let Ok(permit) = self.inner.try_acquire() {
                permit.forget();
                taken += 1;
            }
        }
        let mut owed = self.owed.lock().expect("owed lock");
        *owed += decrease - taken;
    }

    /// A success: count it toward recovery.
    /// Cancels an owed permit first, then grows the semaphore one permit at a
    /// time, never above `max`.
    pub(crate) fn on_success(&self) {
        let grow = {
            let mut aimd = self.aimd.lock().expect("aimd lock");
            let old = aimd.limit();
            aimd.on_success();
            aimd.limit().saturating_sub(old)
        };
        if grow == 0 {
            return;
        }
        let mut owed = self.owed.lock().expect("owed lock");
        if *owed > 0 {
            // A decrease still waits on held permits: the recovered capacity
            // cancels one before the semaphore grows.
            *owed -= 1;
            return;
        }
        self.inner.add_permits(grow);
    }

    /// A task finished successfully and released its permit.
    /// Settles the task's owed debt, then counts the success.
    pub(crate) fn release_success(&self) {
        self.settle(true);
    }

    /// A task finished unsuccessfully and released its permit.
    /// Settles the task's owed debt without counting a success.
    pub(crate) fn release_failure(&self) {
        self.settle(false);
    }

    /// One task released its permit: surrender it if it was held through a
    /// decrease, then count the outcome.
    ///
    /// The caller drops the permit before calling, so a held-through-decrease
    /// permit is free here and reclaims as one `try_acquire` + `forget`.
    fn settle(&self, success: bool) {
        {
            let mut owed = self.owed.lock().expect("owed lock");
            if *owed > 0 {
                if let Ok(permit) = self.inner.try_acquire() {
                    permit.forget();
                    *owed -= 1;
                }
                // `try_acquire` lost to a racing acquirer: leave the debt, the
                // next release settles it.
            }
        }
        if success {
            self.on_success();
        }
    }

    /// The current limit, for logs and tests.
    #[must_use]
    pub fn limit(&self) -> usize {
        self.aimd.lock().expect("aimd lock").limit()
    }

    /// The lowest limit the trajectory ever reached, for tests and the
    /// conformance assertion on a reduced-after-429s run.
    #[must_use]
    pub fn min_limit(&self) -> usize {
        self.aimd.lock().expect("aimd lock").min_limit()
    }

    /// Permits still held through a decrease, awaiting surrender.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn owed(&self) -> usize {
        *self.owed.lock().expect("owed lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the pure controller ----------------------------------------------

    #[test]
    fn halving_on_429() {
        let mut a = Aimd::new(8);
        a.on_retryable();
        assert_eq!(a.limit(), 4);
        a.on_retryable();
        assert_eq!(a.limit(), 2);
        a.on_retryable();
        assert_eq!(a.limit(), 1);
    }

    #[test]
    fn floor_is_one() {
        let mut a = Aimd::new(8);
        for _ in 0..10 {
            a.on_retryable();
        }
        assert_eq!(a.limit(), 1);
    }

    #[test]
    fn recovery_is_linear_every_four_successes() {
        let mut a = Aimd::new(4);
        a.on_retryable();
        assert_eq!(a.limit(), 2);
        // The full slope is owed before the first step: hysteresis.
        for _ in 0..3 {
            a.on_success();
        }
        assert_eq!(a.limit(), 2, "three successes alone must not recover");
        a.on_success();
        assert_eq!(a.limit(), 3);
        for _ in 0..4 {
            a.on_success();
        }
        assert_eq!(a.limit(), 4);
    }

    #[test]
    fn cap_at_max_no_overshoot() {
        let mut a = Aimd::new(4);
        for _ in 0..100 {
            a.on_success();
        }
        assert_eq!(a.limit(), 4);
        // A reduced limit also converges exactly to the max.
        let mut b = Aimd::new(4);
        b.on_retryable();
        for _ in 0..100 {
            b.on_success();
        }
        assert_eq!(b.limit(), 4);
    }

    #[test]
    fn one_success_after_429_does_not_snap_back() {
        let mut a = Aimd::new(8);
        a.on_retryable();
        assert_eq!(a.limit(), 4);
        a.on_success();
        assert_eq!(a.limit(), 4, "hysteresis: a lone success changes nothing");
        a.on_retryable();
        assert_eq!(a.limit(), 2, "a fresh 429 halves again");
    }

    #[test]
    fn a_429_resets_recovery_progress() {
        let mut a = Aimd::new(8);
        a.on_retryable();
        for _ in 0..3 {
            a.on_success();
        }
        a.on_retryable();
        assert_eq!(a.limit(), 2);
        for _ in 0..3 {
            a.on_success();
        }
        assert_eq!(a.limit(), 2);
        a.on_success();
        assert_eq!(a.limit(), 3);
    }

    #[test]
    fn single_worker_stays_at_the_floor() {
        let mut a = Aimd::new(1);
        a.on_retryable();
        assert_eq!(a.limit(), 1);
        a.on_success();
        assert_eq!(a.limit(), 1);
    }

    // ---- the semaphore wrapper --------------------------------------------

    #[tokio::test]
    async fn acquire_runs_under_the_max_without_429s() {
        let wl = Arc::new(WorkerLimit::new(3));
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..9 {
            let (wl, live, peak) = (wl.clone(), live.clone(), peak.clone());
            set.spawn(async move {
                let permit = wl.acquire().await.unwrap();
                let now = live.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                drop(permit);
                wl.release_success();
            });
        }
        while set.join_next().await.is_some() {}
        assert_eq!(peak.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert_eq!(wl.limit(), 3);
    }

    #[tokio::test]
    async fn free_permits_shrink_immediately_and_held_ones_settle() {
        let wl = WorkerLimit::new(4);
        let a = wl.acquire().await.unwrap();
        let b = wl.acquire().await.unwrap();
        let c = wl.acquire().await.unwrap();
        // Three held, one free: the decrease closes the free one at once and
        // records one held permit as owed (3 in flight over the new limit of 2).
        wl.on_retryable();
        assert_eq!(wl.limit(), 2);
        assert_eq!(wl.owed(), 1);
        // The semaphore is fully drained: acquires wait until a held permit settles.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), wl.acquire())
                .await
                .is_err(),
            "no free permit may exist right after the decrease"
        );
        // `a` was held through the decrease: dropping it frees a permit, and
        // the settle call surrenders it as owed.
        drop(a);
        wl.release_success();
        assert_eq!(wl.owed(), 0);
        // Both remaining permits (b, c) are still in flight at limit 2: the
        // semaphore stays drained.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), wl.acquire())
                .await
                .is_err(),
            "the reduced limit is fully in flight after the settle"
        );
        drop((b, c));
    }

    #[tokio::test]
    async fn increase_cancels_owed_before_growing() {
        let wl = WorkerLimit::new(4);
        let a = wl.acquire().await.unwrap();
        let b = wl.acquire().await.unwrap();
        let c = wl.acquire().await.unwrap();
        // Three in flight: the decrease to 2 closes only the one free permit,
        // so one held permit stays owed.
        wl.on_retryable();
        assert_eq!(wl.limit(), 2);
        assert_eq!(wl.owed(), 1);
        drop(c);
        // The full slope of successes must pass before the first recovery step.
        for _ in 0..4 {
            wl.release_success();
        }
        // The recovered step cancelled the owed permit instead of growing:
        // with a and b still held, the semaphore has nothing free to add to.
        assert_eq!(wl.limit(), 3);
        assert_eq!(wl.owed(), 0);
        drop((a, b));
        // Everything released: the semaphore holds exactly the current limit.
        assert_eq!(wl.inner.available_permits(), 3);
    }
}
