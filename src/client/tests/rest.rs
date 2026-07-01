use super::HomeAssistantClient;
use crate::{
    EntityId, Error, RestStateError, RestStateMethod, RestStateRequest, RestStateResponse,
    RestStateTransport,
    runtime::BoxFuture,
    state::{DeleteStateResult, EntityState, SetStateResult, StateWrite},
    test_support::{run_async, sample_state},
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[test]
fn set_state_raw_posts_to_rest_path_and_maps_created_updated_without_cache_write() {
    run_async(async {
        let state = sample_state("sensor.generated", "ready");
        let transport = RecordingRestTransport::new([
            Ok(RestStateResponse {
                status: 201,
                state: Some(state.clone()),
            }),
            Ok(RestStateResponse {
                status: 200,
                state: Some(EntityState {
                    state: "updated".to_string(),
                    ..state.clone()
                }),
            }),
        ]);
        let requests = transport.requests.clone();
        let ha = HomeAssistantClient::with_rest_states_transport(transport);
        let entity_id = state.entity_id.clone();
        let write = StateWrite::new("ready", json!({ "source": "hauto" })).unwrap();

        assert_eq!(
            ha.set_state_raw(&entity_id, write.clone()).await.unwrap(),
            SetStateResult::Created(state.clone())
        );
        assert!(matches!(
            ha.get_state_raw(&entity_id).await,
            Err(Error::EntityNotFound(missing)) if missing == entity_id
        ));

        assert!(matches!(
            ha.set_state_raw(&entity_id, write.clone()).await.unwrap(),
            SetStateResult::Updated(returned) if returned.state == "updated"
        ));

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, RestStateMethod::Post);
        assert_eq!(requests[0].path, "/api/states/sensor.generated");
        assert_eq!(requests[0].entity_id, entity_id);
        assert_eq!(requests[0].body, Some(write));
    });
}

#[test]
fn set_state_raw_validates_attributes_before_transport() {
    run_async(async {
        let transport = RecordingRestTransport::new([Ok(RestStateResponse {
            status: 201,
            state: Some(sample_state("sensor.generated", "ready")),
        })]);
        let requests = transport.requests.clone();
        let ha = HomeAssistantClient::with_rest_states_transport(transport);
        let entity_id = EntityId::new("sensor.generated").unwrap();

        assert!(matches!(
            ha.set_state_raw(
                &entity_id,
                StateWrite {
                    state: "bad".to_string(),
                    attributes: json!(["not", "object"]),
                },
            )
            .await,
            Err(Error::InvalidServiceOptions(_))
        ));
        assert!(requests.lock().unwrap().is_empty());
    });
}

#[test]
fn delete_state_raw_deletes_or_reports_not_found() {
    run_async(async {
        let transport = RecordingRestTransport::new([
            Ok(RestStateResponse {
                status: 200,
                state: None,
            }),
            Ok(RestStateResponse {
                status: 404,
                state: None,
            }),
        ]);
        let requests = transport.requests.clone();
        let ha = HomeAssistantClient::with_rest_states_transport(transport);
        let entity_id = EntityId::new("sensor.generated").unwrap();

        assert_eq!(
            ha.delete_state_raw(&entity_id).await.unwrap(),
            DeleteStateResult::Deleted
        );
        assert_eq!(
            ha.delete_state_raw(&entity_id).await.unwrap(),
            DeleteStateResult::NotFound
        );

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, RestStateMethod::Delete);
        assert_eq!(requests[0].path, "/api/states/sensor.generated");
        assert_eq!(requests[0].body, None);
    });
}

#[test]
fn rest_state_connection_loss_can_map_to_outcome_unknown() {
    run_async(async {
        let transport = RecordingRestTransport::new([
            Err(RestStateError::outcome_unknown(
                "connection closed after write",
            )),
            Err(RestStateError::outcome_unknown(
                "connection closed after delete",
            )),
        ]);
        let ha = HomeAssistantClient::with_rest_states_transport(transport);
        let entity_id = EntityId::new("sensor.generated").unwrap();

        assert!(matches!(
            ha.set_state_raw(
                &entity_id,
                StateWrite::new("ready", json!({ "source": "hauto" })).unwrap(),
            )
            .await,
            Err(Error::OutcomeUnknown(message))
                if message == "connection closed after write"
        ));
        assert!(matches!(
            ha.delete_state_raw(&entity_id).await,
            Err(Error::OutcomeUnknown(message))
                if message == "connection closed after delete"
        ));
    });
}

struct RecordingRestTransport {
    requests: Arc<Mutex<Vec<RestStateRequest>>>,
    responses: Arc<Mutex<Vec<Result<RestStateResponse, RestStateError>>>>,
}

impl RecordingRestTransport {
    fn new(responses: impl IntoIterator<Item = Result<RestStateResponse, RestStateError>>) -> Self {
        let mut responses = responses.into_iter().collect::<Vec<_>>();
        responses.reverse();
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(responses)),
        }
    }
}

impl RestStateTransport for RecordingRestTransport {
    fn send(
        &self,
        request: RestStateRequest,
    ) -> BoxFuture<Result<RestStateResponse, RestStateError>> {
        self.requests.lock().unwrap().push(request);
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Err(RestStateError::connection("no queued response")));
        Box::pin(async move { response })
    }
}
