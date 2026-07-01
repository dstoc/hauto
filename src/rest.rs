use crate::{
    Error, Result,
    entity::EntityId,
    runtime::BoxFuture,
    state::{DeleteStateResult, EntityState, SetStateResult, StateWrite},
};

use url::Url;

pub(crate) trait RestStateTransport: Send + Sync + 'static {
    fn send(
        &self,
        request: RestStateRequest,
    ) -> BoxFuture<Result<RestStateResponse, RestStateError>>;
}

#[derive(Clone, Debug)]
pub(crate) struct ReqwestRestStateTransport {
    client: reqwest::Client,
    base_url: Url,
    access_token: String,
}

impl ReqwestRestStateTransport {
    pub(crate) fn new(base_url: impl AsRef<str>, access_token: impl Into<String>) -> Result<Self> {
        let base_url = Url::parse(base_url.as_ref())
            .map_err(|error| Error::Connection(format!("invalid REST base URL: {error}")))?;
        match base_url.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(Error::Connection(format!(
                    "REST base URL must use http or https, got `{scheme}`"
                )));
            }
        }

        Ok(Self {
            client: reqwest::Client::new(),
            base_url,
            access_token: access_token.into(),
        })
    }

    fn url_for(&self, request: &RestStateRequest) -> Result<Url, RestStateError> {
        self.base_url
            .join(&request.path)
            .map_err(|error| RestStateError::connection(format!("invalid REST state URL: {error}")))
    }
}

impl RestStateTransport for ReqwestRestStateTransport {
    fn send(
        &self,
        request: RestStateRequest,
    ) -> BoxFuture<Result<RestStateResponse, RestStateError>> {
        let transport = self.clone();
        Box::pin(async move {
            let url = transport.url_for(&request)?;
            let builder = match request.method {
                RestStateMethod::Post => {
                    let body = request.body.as_ref().ok_or_else(|| {
                        RestStateError::connection("POST state request missing body")
                    })?;
                    transport.client.post(url).json(body)
                }
                RestStateMethod::Delete => transport.client.delete(url),
            }
            .bearer_auth(&transport.access_token);

            // reqwest does not expose whether an async request error happened before
            // any bytes were sent, mid-send, or while reading the response. Once
            // `send()` is awaited, report ambiguity conservatively and do not retry.
            let response = builder.send().await.map_err(|error| {
                RestStateError::outcome_unknown(format!("REST state request failed: {error}"))
            })?;
            let status = response.status().as_u16();
            let state = if matches!((request.method, status), (RestStateMethod::Post, 200 | 201)) {
                Some(response.json::<EntityState>().await.map_err(|error| {
                    RestStateError::connection(format!(
                        "REST state response could not be decoded: {error}"
                    ))
                })?)
            } else {
                None
            };

            Ok(RestStateResponse { status, state })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestStateMethod {
    Post,
    Delete,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RestStateRequest {
    pub(crate) method: RestStateMethod,
    pub(crate) entity_id: EntityId,
    pub(crate) path: String,
    pub(crate) body: Option<StateWrite>,
}

impl RestStateRequest {
    pub(crate) fn set(entity_id: EntityId, body: StateWrite) -> Self {
        let path = rest_state_path(&entity_id);
        Self {
            method: RestStateMethod::Post,
            entity_id,
            path,
            body: Some(body),
        }
    }

    pub(crate) fn delete(entity_id: EntityId) -> Self {
        let path = rest_state_path(&entity_id);
        Self {
            method: RestStateMethod::Delete,
            entity_id,
            path,
            body: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RestStateResponse {
    pub(crate) status: u16,
    pub(crate) state: Option<EntityState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestStateError {
    pub(crate) message: String,
    pub(crate) outcome_unknown: bool,
}

impl RestStateError {
    pub(crate) fn connection(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            outcome_unknown: false,
        }
    }

    pub(crate) fn outcome_unknown(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            outcome_unknown: true,
        }
    }
}

pub(crate) fn rest_state_path(entity_id: &EntityId) -> String {
    format!("/api/states/{entity_id}")
}

pub(crate) fn map_set_state_response(response: RestStateResponse) -> Result<SetStateResult> {
    match response.status {
        201 => Ok(SetStateResult::Created(response.state.ok_or_else(
            || {
                Error::Connection(
                    "REST state create response did not include an entity state".to_string(),
                )
            },
        )?)),
        200 => Ok(SetStateResult::Updated(response.state.ok_or_else(
            || {
                Error::Connection(
                    "REST state update response did not include an entity state".to_string(),
                )
            },
        )?)),
        status => Err(Error::ServiceRejected(format!(
            "unexpected REST set-state status {status}"
        ))),
    }
}

pub(crate) fn map_delete_state_response(response: RestStateResponse) -> Result<DeleteStateResult> {
    match response.status {
        200 | 204 => Ok(DeleteStateResult::Deleted),
        404 => Ok(DeleteStateResult::NotFound),
        status => Err(Error::ServiceRejected(format!(
            "unexpected REST delete-state status {status}"
        ))),
    }
}
