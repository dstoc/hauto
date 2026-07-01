use std::{fmt, future::Future, sync::Arc, time::Duration};

use tokio::task::JoinSet;
use url::Url;

use crate::{
    Error, Result,
    runtime::{BoxFuture, Context},
};

#[cfg(test)]
mod tests;

/// Configures and runs a set of automations against Home Assistant.
///
/// [`App::run`] operates in connection generations. For each generation it
/// connects and authenticates, subscribes to `state_changed` events, loads the
/// initial state snapshot, and only then starts the registered automations.
/// Subscribing before loading the snapshot avoids a gap in which state changes
/// could be missed.
///
/// When the connection is lost, the generation is cancelled. Automation tasks
/// are given a short opportunity to observe cancellation and finish, after
/// which any remaining tasks are aborted. After a one-second delay, a new
/// connection generation is created and every automation is started again with
/// a new [`Context`].
#[derive(Clone)]
pub struct App {
    pub(crate) home_assistant_url: String,
    pub(crate) websocket_url: String,
    pub(crate) rest_states_url: String,
    access_token: String,
    registrations: Vec<AutomationRegistration>,
}

type AutomationRunner = Arc<dyn Fn(Context) -> BoxFuture<Result<()>> + Send + Sync + 'static>;

#[derive(Clone)]
struct AutomationRegistration {
    name: String,
    run: AutomationRunner,
}

impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App")
            .field("home_assistant_url", &self.home_assistant_url)
            .field("websocket_url", &self.websocket_url)
            .field("rest_states_url", &self.rest_states_url)
            .field("registrations", &self.automation_names())
            .finish_non_exhaustive()
    }
}

impl App {
    /// Creates an application for a Home Assistant base URL and access token.
    ///
    /// The URL must use `http` or `https`. Its query and fragment are ignored;
    /// the WebSocket and REST state endpoints are derived as `/api/websocket`
    /// and `/api/states`.
    ///
    /// # Panics
    ///
    /// Panics if `url` is not a valid URL or does not use `http` or `https`.
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        let urls = HomeAssistantUrls::from_base_url(url.into());
        Self {
            home_assistant_url: urls.base_url,
            websocket_url: urls.websocket_url,
            rest_states_url: urls.rest_states_url,
            access_token: token.into(),
            registrations: Vec::new(),
        }
    }

    /// Registers an [`Automation`] factory under `name`.
    ///
    /// The factory is retained by the application and called to create a fresh
    /// automation instance whenever a connection generation starts. Registered
    /// names need not be unique.
    pub fn automation<A, F>(mut self, name: impl Into<String>, factory: F) -> Self
    where
        A: Automation,
        F: Fn() -> A + Send + Sync + 'static,
    {
        self.registrations.push(AutomationRegistration {
            name: name.into(),
            run: Arc::new(move |ctx| factory().run(ctx)),
        });
        self
    }

    /// Registers an async automation function under `name`.
    ///
    /// The function is called once per connection generation with that
    /// generation's [`Context`]. It must therefore be safe to call again after
    /// connection loss. Registered names need not be unique.
    ///
    /// Returning an error while the generation is active causes [`App::run`]
    /// to fail with [`Error::AutomationTask`]. [`Error::Cancelled`] is treated
    /// as normal shutdown only when the generation has in fact been cancelled.
    pub fn automation_fn<F, Fut>(mut self, name: impl Into<String>, run: F) -> Self
    where
        F: Fn(Context) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.registrations.push(AutomationRegistration {
            name: name.into(),
            run: Arc::new(move |ctx| Box::pin(run(ctx))),
        });
        self
    }

    /// Returns registered automation names in registration order.
    pub fn automation_names(&self) -> Vec<&str> {
        self.registrations
            .iter()
            .map(|registration| registration.name.as_str())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn new_context_generation(&self) -> Result<Context> {
        Context::new_generation_with_rest_states(
            self.home_assistant_url.as_str(),
            self.access_token.clone(),
        )
    }

    /// Runs the application, reconnecting after connection loss.
    ///
    /// Initial connection failures reported as [`Error::Connection`] and
    /// connection losses after startup are retried after one second. Each retry
    /// creates a new generation, reloads the initial snapshot, and invokes all
    /// automation factories again.
    ///
    /// An automation error or panic cancels its generation, aborts its sibling
    /// automation tasks, and is returned as [`Error::AutomationTask`]. Other
    /// non-connection errors encountered while setting up a generation are
    /// returned unchanged. An automation that finishes successfully is not run
    /// again until the next generation; if all automations finish, the
    /// application continues waiting for connection loss.
    ///
    /// This method does not normally return successfully.
    pub async fn run(self) -> Result<()> {
        loop {
            match self.run_one_generation().await {
                Ok(GenerationOutcome::ConnectionLost) => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(Error::Connection(_)) => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) async fn run_one_generation(&self) -> Result<GenerationOutcome> {
        let ctx = Context::new_generation_with_websocket_and_rest_states(
            self.rest_states_url.as_str(),
            self.websocket_url.as_str(),
            self.access_token.clone(),
        )
        .await?;
        let home_assistant = ctx.home_assistant();

        let _state_changed_events = home_assistant.subscribe_state_changed_events().await?;
        home_assistant.refresh_states_from_websocket().await?;

        let mut automations = JoinSet::new();
        for registration in self.registrations.iter().cloned() {
            let ctx = ctx.clone();
            automations.spawn(async move {
                (registration.run)(ctx)
                    .await
                    .map_err(|error| (registration.name, error))
            });
        }

        let mut remaining = self.registrations.len();
        loop {
            if remaining == 0 {
                ctx.cancelled().await;
                return Ok(GenerationOutcome::ConnectionLost);
            }

            tokio::select! {
                () = ctx.cancelled() => {
                    drain_cancelled_automations(&mut automations).await;
                    return Ok(GenerationOutcome::ConnectionLost);
                }
                joined = automations.join_next() => {
                    let Some(joined) = joined else {
                        ctx.cancelled().await;
                        return Ok(GenerationOutcome::ConnectionLost);
                    };
                    remaining -= 1;
                    match joined {
                        Ok(Ok(())) => {}
                        Ok(Err((_, Error::Cancelled))) if home_assistant.ensure_generation_active().is_err() => {}
                        Ok(Err((name, error))) => {
                            ctx.cancel_generation();
                            automations.abort_all();
                            return Err(Error::AutomationTask(format!("{name}: {error}")));
                        }
                        Err(error) if error.is_cancelled() && home_assistant.ensure_generation_active().is_err() => {}
                        Err(error) => {
                            ctx.cancel_generation();
                            automations.abort_all();
                            return Err(Error::AutomationTask(format!("automation task panicked or was aborted: {error}")));
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationOutcome {
    ConnectionLost,
}

async fn drain_cancelled_automations(automations: &mut JoinSet<Result<(), (String, Error)>>) {
    let grace = tokio::time::sleep(Duration::from_millis(100));
    tokio::pin!(grace);

    loop {
        tokio::select! {
            () = &mut grace => {
                automations.abort_all();
                while automations.join_next().await.is_some() {}
                return;
            }
            joined = automations.join_next() => {
                if joined.is_none() {
                    return;
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HomeAssistantUrls {
    base_url: String,
    pub(crate) websocket_url: String,
    pub(crate) rest_states_url: String,
}

impl HomeAssistantUrls {
    fn from_base_url(base_url: String) -> Self {
        let mut base = Url::parse(&base_url).unwrap_or_else(|error| {
            panic!("invalid Home Assistant base URL `{base_url}`: {error}")
        });
        match base.scheme() {
            "http" | "https" => {}
            scheme => panic!("Home Assistant base URL must use http or https, got `{scheme}`"),
        }
        base.set_query(None);
        base.set_fragment(None);

        let mut websocket = base.clone();
        websocket
            .set_scheme(match base.scheme() {
                "http" => "ws",
                "https" => "wss",
                _ => unreachable!("scheme checked above"),
            })
            .expect("ws/wss are valid URL schemes");
        websocket.set_path("/api/websocket");

        let mut states = base.clone();
        states.set_path("/api/states");

        Self {
            base_url: base.to_string().trim_end_matches('/').to_string(),
            websocket_url: websocket.to_string(),
            rest_states_url: states.to_string().trim_end_matches('/').to_string(),
        }
    }
}

/// An automation that can be instantiated anew for each connection generation.
///
/// Register implementations with [`App::automation`]. Connection loss cancels
/// the supplied [`Context`], and [`App`] creates another instance with a new
/// context after reconnecting.
pub trait Automation: Send + 'static {
    /// Runs this automation within `ctx`'s connection generation.
    ///
    /// The returned future should use cancellation-aware [`Context`] operations
    /// or await [`Context::cancelled`] when doing its own coordination.
    /// Returning an error while the generation remains active stops the
    /// application and is surfaced by [`App::run`] as
    /// [`Error::AutomationTask`].
    fn run(self, ctx: Context) -> BoxFuture<Result<()>>
    where
        Self: Sized;
}
