use std::{
    future::{Future, IntoFuture},
    pin::Pin,
    time::Duration,
};

use crate::{BinaryState, Context, EntityId, Error, Result, StateChangeStream, StateChangedEvent};

#[derive(Clone, Debug)]
pub struct StateWait<'a> {
    ctx: &'a Context,
    entity_id: EntityId,
    target: BinaryState,
    require_transition: bool,
    hold_for: Option<Duration>,
}

impl<'a> StateWait<'a> {
    pub(crate) fn new(ctx: &'a Context, entity_id: EntityId, target: BinaryState) -> Self {
        Self {
            ctx,
            entity_id,
            target,
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

    pub fn within(self, duration: Duration) -> TimedStateWait<'a> {
        TimedStateWait {
            wait: self,
            timeout: duration,
        }
    }
}

impl<'a> IntoFuture for StateWait<'a> {
    type Output = Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { run_state_wait(self).await.map(|_| ()) })
    }
}

#[derive(Clone, Debug)]
pub struct TimedStateWait<'a> {
    wait: StateWait<'a>,
    timeout: Duration,
}

impl<'a> IntoFuture for TimedStateWait<'a> {
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

impl<'a> TimedStateWait<'a> {
    async fn ctx_timeout(self) -> Result<TimeoutResult<()>> {
        self.wait
            .ctx
            .timeout(self.timeout, run_state_wait(self.wait))
            .await
    }
}

#[derive(Clone, Debug)]
pub struct StateExpectation<'a> {
    ctx: &'a Context,
    entity_id: EntityId,
    target: BinaryState,
    hold_for: Option<Duration>,
}

impl<'a> StateExpectation<'a> {
    pub(crate) fn new(ctx: &'a Context, entity_id: EntityId, target: BinaryState) -> Self {
        Self {
            ctx,
            entity_id,
            target,
            hold_for: None,
        }
    }

    pub fn for_at_least(mut self, duration: Duration) -> Self {
        self.hold_for = Some(duration);
        self
    }
}

impl<'a> IntoFuture for StateExpectation<'a> {
    type Output = Result<HoldResult<BinaryState>>;
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

async fn run_state_wait(wait: StateWait<'_>) -> Result<()> {
    wait.ctx.home_assistant.ensure_generation_active()?;
    let mut changes = wait.ctx.state_changes(&wait.entity_id);
    let initial = current_binary_state(wait.ctx, &wait.entity_id)?;
    let mut ready_for_target = !wait.require_transition || initial != Some(wait.target);

    if !wait.require_transition && initial == Some(wait.target) {
        if hold_target_for(
            wait.ctx,
            &mut changes,
            &wait.entity_id,
            wait.target,
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
        let state = event_binary_state(&event, &wait.entity_id)?;
        if state == wait.target {
            if ready_for_target
                && hold_target_for(
                    wait.ctx,
                    &mut changes,
                    &wait.entity_id,
                    wait.target,
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

async fn run_state_expectation(
    expectation: StateExpectation<'_>,
) -> Result<HoldResult<BinaryState>> {
    expectation.ctx.home_assistant.ensure_generation_active()?;
    let mut changes = expectation.ctx.state_changes(&expectation.entity_id);
    let actual = current_binary_state(expectation.ctx, &expectation.entity_id)?
        .ok_or_else(|| Error::EntityNotFound(expectation.entity_id.clone()))?;
    if actual != expectation.target {
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
                let actual = event_binary_state(&event, &expectation.entity_id)?;
                if actual != expectation.target {
                    return Ok(HoldResult::Interrupted { actual });
                }
            }
        }
    }
}

fn current_binary_state(ctx: &Context, entity_id: &EntityId) -> Result<Option<BinaryState>> {
    ctx.home_assistant
        .generation
        .cached_state(entity_id)
        .as_ref()
        .map(|state| BinaryState::decode(&state.state))
        .transpose()
}

fn event_binary_state(event: &StateChangedEvent, entity_id: &EntityId) -> Result<BinaryState> {
    event
        .new_state
        .as_ref()
        .ok_or_else(|| Error::EntityNotFound(entity_id.clone()))
        .and_then(|state| BinaryState::decode(&state.state))
}

pub(crate) async fn hold_target_for(
    ctx: &Context,
    changes: &mut StateChangeStream,
    entity_id: &EntityId,
    target: BinaryState,
    hold_for: Option<Duration>,
) -> Result<bool> {
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
                let actual = event_binary_state(&event, entity_id)?;
                if actual != target {
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
