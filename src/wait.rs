//! Wait for state conditions or check that they already hold.
//!
//! A *wait* completes when its condition becomes true. Entity and global waits
//! evaluate the current cached state first, so an already-satisfied condition
//! completes immediately unless a hold duration or
//! [`StateWait::require_transition`] says otherwise. An *expectation* checks an
//! entity's current cached state immediately and reports a mismatch instead of
//! waiting for the condition to become true.
//!
//! [`StateWait::for_at_least`], [`GlobalStateWait::for_at_least`], and
//! [`StateExpectation::for_at_least`] require the condition to remain true
//! continuously. A change that makes the condition false interrupts the hold:
//! waits continue looking for a new continuously matching interval, while an
//! expectation returns [`HoldResult::Interrupted`]. Matching updates do not
//! restart the interval. [`StateWait::within`] and
//! [`GlobalStateWait::within`] bound the entire operation, including any hold
//! interval, and report expiration as [`WaitResult::TimedOut`].
//!
//! Entity waits decode cached and event states using the entity handle's typed
//! decoder. A missing initial state can still be followed by a wait, but an
//! expectation on a missing entity returns [`crate::Error::EntityNotFound`].
//! Deletion during a wait or hold also returns that error. A malformed state
//! returns [`crate::Error::InvalidState`]; values such as `unknown` and
//! `unavailable` are either represented or rejected according to the selected
//! entity decoder.
//!
//! All operations belong to one [`crate::Context`] connection generation.
//! Cancellation or connection loss ends them with an error (normally
//! [`crate::Error::Cancelled`]); a hold is not resumed after reconnection.
//! [`crate::App`] starts the automation again with a new context and a fresh
//! wait.
//!
//! Builder types in this module are normally returned by entity or context
//! methods rather than constructed directly:
//!
//! ```no_run
//! # use std::time::Duration;
//! # use hauto::{BinarySensor, Context, Result, WaitResult};
//! # async fn example(sensor: &BinarySensor, ctx: &Context) -> Result<()> {
//! match sensor
//!     .wait_until_on(ctx)
//!     .require_transition()
//!     .for_at_least(Duration::from_secs(2))
//!     .within(Duration::from_secs(30))
//!     .await?
//! {
//!     WaitResult::Satisfied => {}
//!     WaitResult::TimedOut => {}
//! }
//! # Ok(())
//! # }
//! ```

use std::{
    fmt,
    future::{Future, IntoFuture},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use crate::{
    Error, Result,
    client::StateChangeStream,
    context::Context,
    entity::{BinaryState, EntityId},
    state::{EntityState, StateCache, StateChangedEvent},
};

type StateDecoder<T> = fn(&EntityId, &EntityState) -> Result<T>;
type StateCondition<T> = Arc<dyn Fn(&T) -> bool + Send + Sync + 'static>;

#[derive(Clone)]
/// A builder that waits for one entity's decoded state to match a condition.
///
/// Entity methods such as `BinarySensor::wait_until_on` and
/// `Sensor::wait_until_matching` normally return this type. Awaiting it yields
/// [`Result<()>`](crate::Result): `Ok(())` means the condition was satisfied
/// (and held for any requested duration). The wait has no timeout unless
/// [`within`](Self::within) is applied.
pub struct StateWait<'a, T = BinaryState> {
    ctx: &'a Context,
    entity_id: EntityId,
    decode: StateDecoder<T>,
    condition: StateCondition<T>,
    require_transition: bool,
    hold_for: Option<Duration>,
}

impl<T> fmt::Debug for StateWait<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateWait")
            .field("ctx", &self.ctx)
            .field("entity_id", &self.entity_id)
            .field("require_transition", &self.require_transition)
            .field("hold_for", &self.hold_for)
            .finish_non_exhaustive()
    }
}

impl<'a, T> StateWait<'a, T>
where
    T: Clone + Send + Sync + 'static,
{
    pub(crate) fn matching<F>(
        ctx: &'a Context,
        entity_id: EntityId,
        decode: StateDecoder<T>,
        predicate: F,
    ) -> Self
    where
        F: Fn(&T) -> bool + Send + Sync + 'static,
    {
        Self {
            ctx,
            entity_id,
            decode,
            condition: Arc::new(predicate),
            require_transition: false,
            hold_for: None,
        }
    }

    /// Requires a false-to-true observation before this wait can complete.
    ///
    /// Without this option, an already-matching cached state is accepted.
    /// With it, an initially matching condition is rejected until a later
    /// decoded state does not match and a subsequent state matches. An
    /// initially missing or non-matching state already satisfies the "false"
    /// side of this requirement.
    pub fn require_transition(mut self) -> Self {
        self.require_transition = true;
        self
    }

    /// Requires the condition to remain true continuously for `duration`.
    ///
    /// A non-matching update interrupts the interval and the wait continues
    /// until a new matching interval lasts long enough. Matching updates do
    /// not restart the timer. A zero duration behaves like no hold.
    ///
    /// Deletion or a state that cannot be decoded ends the wait with an error
    /// rather than interrupting and restarting the interval.
    pub fn for_at_least(mut self, duration: Duration) -> Self {
        self.hold_for = Some(duration);
        self
    }

    /// Limits the total time available for this wait.
    ///
    /// The timeout includes time spent waiting for the first match and time
    /// spent satisfying [`for_at_least`](Self::for_at_least). Awaiting the
    /// returned builder yields `Ok(WaitResult::TimedOut)` on ordinary
    /// expiration; cancellation and state errors remain errors.
    pub fn within(self, duration: Duration) -> TimedStateWait<'a, T> {
        TimedStateWait {
            wait: self,
            timeout: duration,
        }
    }
}

impl<'a, T> StateWait<'a, T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    pub(crate) fn new(
        ctx: &'a Context,
        entity_id: EntityId,
        decode: StateDecoder<T>,
        target: T,
    ) -> Self {
        Self::matching(ctx, entity_id, decode, move |actual| actual == &target)
    }
}

impl<'a, T> IntoFuture for StateWait<'a, T>
where
    T: Clone + Send + Sync + 'static,
{
    type Output = Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { run_state_wait(self).await.map(|_| ()) })
    }
}

#[derive(Clone)]
/// A time-bounded [`StateWait`].
///
/// This type is normally returned by [`StateWait::within`]. Awaiting it yields
/// [`Result<WaitResult>`](crate::Result): [`WaitResult::Satisfied`] indicates
/// completion and [`WaitResult::TimedOut`] indicates ordinary timeout expiry.
/// Entity deletion, malformed state, cancellation, and connection failures
/// are returned as errors instead.
pub struct TimedStateWait<'a, T = BinaryState> {
    wait: StateWait<'a, T>,
    timeout: Duration,
}

impl<T> fmt::Debug for TimedStateWait<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimedStateWait")
            .field("wait", &self.wait)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// A builder that waits for a predicate over the complete cached state.
///
/// [`Context::wait_until_state`](crate::Context::wait_until_state) normally
/// returns this type. Its predicate is evaluated immediately and again after
/// every state-change event. Awaiting the builder yields
/// [`Result<()>`](crate::Result); predicate errors and connection-generation
/// cancellation are propagated.
pub struct GlobalStateWait<'a, F> {
    ctx: &'a Context,
    predicate: F,
    hold_for: Option<Duration>,
}

impl<F> fmt::Debug for GlobalStateWait<'_, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlobalStateWait")
            .field("ctx", &self.ctx)
            .field("hold_for", &self.hold_for)
            .finish_non_exhaustive()
    }
}

impl<'a, F> GlobalStateWait<'a, F>
where
    F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
{
    pub(crate) fn new(ctx: &'a Context, predicate: F) -> Self {
        Self {
            ctx,
            predicate,
            hold_for: None,
        }
    }

    /// Requires the predicate to remain true continuously for `duration`.
    ///
    /// While holding, every state-change event causes the predicate to be
    /// evaluated against the latest cache. A false result interrupts the
    /// interval, after which the wait looks for a new true interval. A zero
    /// duration behaves like no hold. Predicate errors end the wait.
    pub fn for_at_least(mut self, duration: Duration) -> Self {
        self.hold_for = Some(duration);
        self
    }

    /// Limits the total time available for this wait.
    ///
    /// The timeout includes any interval requested by
    /// [`for_at_least`](Self::for_at_least). Awaiting the returned builder
    /// reports ordinary expiry as [`WaitResult::TimedOut`], while predicate,
    /// cancellation, and connection errors remain errors.
    pub fn within(self, duration: Duration) -> TimedGlobalStateWait<'a, F> {
        TimedGlobalStateWait {
            wait: self,
            timeout: duration,
        }
    }
}

impl<'a, F> IntoFuture for GlobalStateWait<'a, F>
where
    F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
{
    type Output = Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { run_global_state_wait(self).await })
    }
}

/// A time-bounded [`GlobalStateWait`].
///
/// This type is normally returned by [`GlobalStateWait::within`]. Awaiting it
/// yields [`Result<WaitResult>`](crate::Result), distinguishing satisfaction
/// from ordinary timeout expiry. Predicate errors and generation cancellation
/// are returned as errors rather than timeout results.
pub struct TimedGlobalStateWait<'a, F> {
    wait: GlobalStateWait<'a, F>,
    timeout: Duration,
}

impl<F> fmt::Debug for TimedGlobalStateWait<'_, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimedGlobalStateWait")
            .field("wait", &self.wait)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl<'a, F> IntoFuture for TimedGlobalStateWait<'a, F>
where
    F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
{
    type Output = Result<WaitResult>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            match self.ctx_timeout().await? {
                TimeoutResult::Completed(()) => Ok(WaitResult::Satisfied),
                TimeoutResult::TimedOut => Ok(WaitResult::TimedOut),
            }
        })
    }
}

impl<'a, F> TimedGlobalStateWait<'a, F>
where
    F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
{
    async fn ctx_timeout(self) -> Result<TimeoutResult<()>> {
        self.wait
            .ctx
            .timeout(self.timeout, run_global_state_wait(self.wait))
            .await
    }
}

impl<'a, T> IntoFuture for TimedStateWait<'a, T>
where
    T: Clone + Send + Sync + 'static,
{
    type Output = Result<WaitResult>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            match self.ctx_timeout().await? {
                TimeoutResult::Completed(()) => Ok(WaitResult::Satisfied),
                TimeoutResult::TimedOut => Ok(WaitResult::TimedOut),
            }
        })
    }
}

impl<'a, T> TimedStateWait<'a, T>
where
    T: Clone + Send + Sync + 'static,
{
    async fn ctx_timeout(self) -> Result<TimeoutResult<()>> {
        self.wait
            .ctx
            .timeout(self.timeout, run_state_wait(self.wait))
            .await
    }
}

#[derive(Clone)]
/// A builder that immediately checks one entity's decoded state.
///
/// Entity methods such as `BinarySensor::expect_on` and
/// `Sensor::expect_matching` normally return this type. Unlike [`StateWait`],
/// an expectation does not wait for a currently false condition to become
/// true. Awaiting it yields [`Result<HoldResult<T>>`](crate::Result), including
/// the actual decoded state for a mismatch or interrupted hold.
///
/// A missing current entity is an error, as is a malformed current state.
pub struct StateExpectation<'a, T = BinaryState> {
    ctx: &'a Context,
    entity_id: EntityId,
    decode: StateDecoder<T>,
    condition: StateCondition<T>,
    hold_for: Option<Duration>,
}

impl<T> fmt::Debug for StateExpectation<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateExpectation")
            .field("ctx", &self.ctx)
            .field("entity_id", &self.entity_id)
            .field("hold_for", &self.hold_for)
            .finish_non_exhaustive()
    }
}

impl<'a, T> StateExpectation<'a, T>
where
    T: Clone + Send + Sync + 'static,
{
    pub(crate) fn matching<F>(
        ctx: &'a Context,
        entity_id: EntityId,
        decode: StateDecoder<T>,
        predicate: F,
    ) -> Self
    where
        F: Fn(&T) -> bool + Send + Sync + 'static,
    {
        Self {
            ctx,
            entity_id,
            decode,
            condition: Arc::new(predicate),
            hold_for: None,
        }
    }

    /// Requires an initially matching condition to remain continuously true.
    ///
    /// If the initial state does not match, awaiting returns
    /// [`HoldResult::NotSatisfied`] immediately. If a later decoded state
    /// stops matching before `duration` elapses, it returns
    /// [`HoldResult::Interrupted`] with that state. Matching updates do not
    /// restart the timer, and a zero duration succeeds immediately.
    ///
    /// Entity deletion, malformed state, cancellation, or connection loss
    /// returns an error rather than a `HoldResult`.
    pub fn for_at_least(mut self, duration: Duration) -> Self {
        self.hold_for = Some(duration);
        self
    }
}

impl<'a, T> StateExpectation<'a, T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    pub(crate) fn new(
        ctx: &'a Context,
        entity_id: EntityId,
        decode: StateDecoder<T>,
        target: T,
    ) -> Self {
        Self::matching(ctx, entity_id, decode, move |actual| actual == &target)
    }
}

impl<'a, T> IntoFuture for StateExpectation<'a, T>
where
    T: Clone + Send + Sync + 'static,
{
    type Output = Result<HoldResult<T>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { run_state_expectation(self).await })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// The outcome of awaiting a wait builder configured with `within`.
pub enum WaitResult {
    /// The condition was satisfied, including any required hold interval.
    Satisfied,
    /// The configured timeout elapsed before the condition was satisfied.
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// The outcome of checking a [`StateExpectation`].
pub enum HoldResult<T> {
    /// The condition was initially satisfied and held for the requested time.
    Held,
    /// The current state did not satisfy the condition.
    ///
    /// `actual` is the decoded state observed by the immediate check.
    NotSatisfied { actual: T },
    /// The condition was initially satisfied but stopped matching during its
    /// required hold interval.
    ///
    /// `actual` is the first decoded non-matching state.
    Interrupted { actual: T },
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// The generic outcome of [`Context::timeout`](crate::Context::timeout).
///
/// Wait builders configured with `within` translate this type into
/// [`WaitResult`], but it is also useful when timing arbitrary
/// cancellation-aware work.
pub enum TimeoutResult<T> {
    /// The operation completed before the deadline with its output.
    Completed(T),
    /// The deadline elapsed before the operation completed.
    TimedOut,
}

async fn run_state_wait<T>(wait: StateWait<'_, T>) -> Result<()>
where
    T: Clone + Send + Sync + 'static,
{
    wait.ctx.home_assistant.ensure_generation_active()?;
    let mut changes = wait.ctx.state_changes(&wait.entity_id);
    let initial = current_state(wait.ctx, &wait.entity_id, wait.decode)?;
    let initial_matches = initial
        .as_ref()
        .is_some_and(|actual| (wait.condition)(actual));
    let mut ready_for_target = !wait.require_transition || !initial_matches;

    if !wait.require_transition && initial_matches {
        if hold_target_for(
            wait.ctx,
            &mut changes,
            &wait.entity_id,
            wait.decode,
            &wait.condition,
            wait.hold_for,
        )
        .await?
        {
            return Ok(());
        }
        ready_for_target = true;
    }

    loop {
        let event = next_state_change(wait.ctx, &mut changes).await?;
        let state = event_state(&event, &wait.entity_id, wait.decode)?;
        if (wait.condition)(&state) {
            if ready_for_target
                && hold_target_for(
                    wait.ctx,
                    &mut changes,
                    &wait.entity_id,
                    wait.decode,
                    &wait.condition,
                    wait.hold_for,
                )
                .await?
            {
                return Ok(());
            }
        } else {
            ready_for_target = true;
        }
    }
}

async fn run_global_state_wait<F>(wait: GlobalStateWait<'_, F>) -> Result<()>
where
    F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
{
    wait.ctx.home_assistant.ensure_generation_active()?;
    let mut changes = StateChangeStream::new(
        wait.ctx.home_assistant.generation.state_changes.subscribe(),
        None,
    );

    if evaluate_global_state(wait.ctx, &wait.predicate)?
        && hold_global_state_for(wait.ctx, &mut changes, &wait.predicate, wait.hold_for).await?
    {
        return Ok(());
    }

    loop {
        let _event = next_state_change(wait.ctx, &mut changes).await?;
        if evaluate_global_state(wait.ctx, &wait.predicate)?
            && hold_global_state_for(wait.ctx, &mut changes, &wait.predicate, wait.hold_for).await?
        {
            return Ok(());
        }
    }
}

async fn run_state_expectation<T>(expectation: StateExpectation<'_, T>) -> Result<HoldResult<T>>
where
    T: Clone + Send + Sync + 'static,
{
    expectation.ctx.home_assistant.ensure_generation_active()?;
    let mut changes = expectation.ctx.state_changes(&expectation.entity_id);
    let actual = current_state(expectation.ctx, &expectation.entity_id, expectation.decode)?
        .ok_or_else(|| Error::EntityNotFound(expectation.entity_id.clone()))?;
    if !(expectation.condition)(&actual) {
        return Ok(HoldResult::NotSatisfied { actual });
    }

    let Some(hold_for) = expectation.hold_for else {
        return Ok(HoldResult::Held);
    };
    if hold_for.is_zero() {
        return Ok(HoldResult::Held);
    }

    let deadline = tokio::time::sleep(hold_for);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            () = &mut deadline => return Ok(HoldResult::Held),
            event = next_state_change(expectation.ctx, &mut changes) => {
                let event = event?;
                let actual = event_state(&event, &expectation.entity_id, expectation.decode)?;
                if !(expectation.condition)(&actual) {
                    return Ok(HoldResult::Interrupted { actual });
                }
            }
        }
    }
}

fn current_state<T>(
    ctx: &Context,
    entity_id: &EntityId,
    decode: StateDecoder<T>,
) -> Result<Option<T>> {
    ctx.home_assistant
        .generation
        .cached_state(entity_id)
        .as_ref()
        .map(|state| decode(entity_id, state))
        .transpose()
}

fn event_state<T>(
    event: &StateChangedEvent,
    entity_id: &EntityId,
    decode: StateDecoder<T>,
) -> Result<T> {
    event
        .new_state
        .as_ref()
        .ok_or_else(|| Error::EntityNotFound(entity_id.clone()))
        .and_then(|state| decode(entity_id, state))
}

async fn hold_target_for<T>(
    ctx: &Context,
    changes: &mut StateChangeStream,
    entity_id: &EntityId,
    decode: StateDecoder<T>,
    condition: &StateCondition<T>,
    hold_for: Option<Duration>,
) -> Result<bool>
where
    T: Clone + Send + Sync + 'static,
{
    let Some(hold_for) = hold_for else {
        return Ok(true);
    };
    if hold_for.is_zero() {
        return Ok(true);
    }

    let deadline = tokio::time::sleep(hold_for);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => return Ok(true),
            event = next_state_change(ctx, changes) => {
                let event = event?;
                let actual = event_state(&event, entity_id, decode)?;
                if !condition(&actual) {
                    return Ok(false);
                }
            }
        }
    }
}

fn evaluate_global_state<F>(ctx: &Context, predicate: &F) -> Result<bool>
where
    F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
{
    predicate(&StateCache::new(&ctx.home_assistant.generation))
}

async fn hold_global_state_for<F>(
    ctx: &Context,
    changes: &mut StateChangeStream,
    predicate: &F,
    hold_for: Option<Duration>,
) -> Result<bool>
where
    F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
{
    let Some(hold_for) = hold_for else {
        return Ok(true);
    };
    if hold_for.is_zero() {
        return Ok(true);
    }

    let deadline = tokio::time::sleep(hold_for);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => return Ok(true),
            event = next_state_change(ctx, changes) => {
                let _event = event?;
                if !evaluate_global_state(ctx, predicate)? {
                    return Ok(false);
                }
            }
        }
    }
}

pub(crate) async fn next_state_change(
    ctx: &Context,
    changes: &mut StateChangeStream,
) -> Result<StateChangedEvent> {
    tokio::select! {
        event = changes.next() => {
            match event {
                Some(Ok(event)) => Ok(event),
                Some(Err(error)) => Err(Error::EventStream(error)),
                None => Err(Error::Connection("state change stream closed".to_string())),
            }
        }
        () = ctx.cancelled() => Err(Error::Cancelled),
    }
}
