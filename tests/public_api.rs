use hauto::{
    App, Automation, BinarySensor, BinaryState, Context, EntityId, Error, HoldResult, Light,
    LightTurnOff, LightTurnOn, Result, Sensor, SensorValue, Switch, TimeoutResult, WaitResult,
};

#[test]
fn common_root_api_is_importable() -> Result<()> {
    fn assert_automation<A: Automation>() {}
    let _ = assert_automation::<RootAutomation>;

    let _app = App::new("http://homeassistant.local:8123", "token");
    let _context = Context::default();
    let _binary_sensor = BinarySensor::new("binary_sensor.office")?;
    let _entity_id = EntityId::new("sensor.office")?;
    let _light = Light::new("light.office")?;
    let _sensor = Sensor::<f64>::new("sensor.temperature")?;
    let _switch = Switch::new("switch.office")?;
    let _binary_state = BinaryState::On;
    let _sensor_value = SensorValue::Value(1_u8);
    let _turn_on = LightTurnOn::default();
    let _turn_off = LightTurnOff::default();
    let _hold = HoldResult::<()>::Held;
    let _timeout = TimeoutResult::Completed(());
    let _wait = WaitResult::Satisfied;
    let _error: Option<Error> = None;

    Ok(())
}

struct RootAutomation;

impl Automation for RootAutomation {
    fn run(self, _ctx: Context) -> hauto::runtime::BoxFuture<Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn advanced_module_api_is_importable() {
    use hauto::{
        client::{EventStreamError, HomeAssistantClient, RawEventStream, StateChangeStream},
        discovery::{AreaId, AreaInfo, DiscoveredEntity, EntityCatalog, EntityQuery, EntitySet},
        entity::{
            BinarySensor as ModuleBinarySensor, BinaryState as ModuleBinaryState,
            EntityId as ModuleEntityId, Light as ModuleLight, Sensor as ModuleSensor,
            SensorValue as ModuleSensorValue, Switch as ModuleSwitch,
        },
        runtime::{
            App as RuntimeApp, Automation as RuntimeAutomation, BoxFuture,
            Context as RuntimeContext, TaskHandle, TimerHandle,
        },
        service::{LightTurnOff as ServiceLightTurnOff, LightTurnOn as ServiceLightTurnOn},
        state::{
            DeleteStateResult, EntityState, SetStateResult, StateCache, StateChangedEvent,
            StateWrite,
        },
        wait::{
            GlobalStateWait, HoldResult as ModuleHoldResult, StateExpectation, StateWait,
            TimedGlobalStateWait, TimedStateWait, TimeoutResult as ModuleTimeoutResult,
            WaitResult as ModuleWaitResult,
        },
    };

    fn entity_paths(
        _: Option<ModuleEntityId>,
        _: Option<ModuleBinarySensor>,
        _: Option<ModuleLight>,
        _: Option<ModuleSensor<f64>>,
        _: Option<ModuleSwitch>,
        _: Option<ModuleBinaryState>,
        _: Option<ModuleSensorValue<f64>>,
    ) {
    }

    fn discovery_paths(
        _: Option<AreaId>,
        _: Option<AreaInfo>,
        _: Option<DiscoveredEntity>,
        _: Option<EntityCatalog>,
        _: Option<EntityQuery>,
        _: Option<EntitySet>,
    ) {
    }

    fn wait_paths<'a, F>(
        _: Option<GlobalStateWait<'a, F>>,
        _: Option<StateExpectation<'a>>,
        _: Option<StateWait<'a>>,
        _: Option<TimedGlobalStateWait<'a, F>>,
        _: Option<TimedStateWait<'a>>,
        _: Option<ModuleHoldResult<()>>,
        _: Option<ModuleTimeoutResult<()>>,
        _: Option<ModuleWaitResult>,
    ) {
    }

    fn state_paths(
        _: Option<StateCache<'_>>,
        _: Option<DeleteStateResult>,
        _: Option<EntityState>,
        _: Option<SetStateResult>,
        _: Option<StateChangedEvent>,
        _: Option<StateWrite>,
    ) {
    }

    fn runtime_paths<A: RuntimeAutomation>(
        _: Option<RuntimeApp>,
        _: Option<RuntimeContext>,
        _: Option<BoxFuture<()>>,
        _: Option<TaskHandle<()>>,
        _: Option<TimerHandle<()>>,
    ) {
    }

    fn service_paths(_: Option<ServiceLightTurnOn>, _: Option<ServiceLightTurnOff>) {}

    fn client_paths(
        _: Option<EventStreamError>,
        _: Option<HomeAssistantClient>,
        _: Option<RawEventStream>,
        _: Option<StateChangeStream>,
    ) {
    }

    let _ = entity_paths;
    let _ = discovery_paths;
    let _ = wait_paths::<fn(&StateCache<'_>) -> Result<bool>>;
    let _ = state_paths;
    let _ = runtime_paths::<RootAutomation>;
    let _ = service_paths;
    let _ = client_paths;
}
