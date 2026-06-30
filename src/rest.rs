use crate::{
    BoxFuture, DeleteStateResult, EntityId, EntityState, Error, Result, SetStateResult, StateWrite,
};

pub(crate) trait RestStateTransport: Send + Sync + 'static {
    fn send(
        &self,
        request: RestStateRequest,
    ) -> BoxFuture<Result<RestStateResponse, RestStateError>>;
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
    #[cfg(test)]
    pub(crate) fn connection(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            outcome_unknown: false,
        }
    }

    #[cfg(test)]
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
