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

    pub fn require_transition(mut self) -> Self {
        self.require_transition = true;
        self
    }

    pub fn for_at_least(mut self, duration: Duration) -> Self {
        self.hold_for = Some(duration);
        self
    }

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

    pub fn for_at_least(mut self, duration: Duration) -> Self {
        self.hold_for = Some(duration);
        self
    }

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
pub enum WaitResult {
    Satisfied,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldResult<T> {
    Held,
    NotSatisfied { actual: T },
    Interrupted { actual: T },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeoutResult<T> {
    Completed(T),
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
