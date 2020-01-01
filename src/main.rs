use hyper::{
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server, StatusCode,
};
use prometheus::{Encoder, GaugeVec, IntGaugeVec, Opts, Registry, TextEncoder};
use rumble::{
    api::{BDAddr, Central, CentralEvent, Peripheral},
    bluez::{adapter::ConnectedAdapter, manager::Manager},
};
use ruuvi_sensor_protocol::{
    BatteryPotential, Humidity, MeasurementSequenceNumber, MovementCounter, Pressure, SensorValues,
    Temperature,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    convert::{Infallible, TryInto},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::Mutex,
    time::{delay_for, Instant},
};

fn parse_data(data: &Vec<u8>) -> Option<SensorValues> {
    if data.len() >= 2 {
        let id = u16::from_le_bytes(data[0..2].try_into().unwrap());
        ruuvi_sensor_protocol::SensorValues::from_manufacturer_specific_data(id, &data[2..]).ok()
    } else {
        None
    }
}

fn on_event_with_address(
    central: &ConnectedAdapter,
    address: BDAddr,
) -> Option<(BDAddr, SensorValues)> {
    parse_data(
        &central
            .peripheral(address)?
            .properties()
            .manufacturer_data?,
    )
    .and_then(|data| Some((address, data)))
}

fn parse_event(central: &ConnectedAdapter, event: CentralEvent) -> Option<(BDAddr, SensorValues)> {
    match event {
        CentralEvent::DeviceDiscovered(address) => on_event_with_address(central, address),
        CentralEvent::DeviceUpdated(address) => on_event_with_address(central, address),
        CentralEvent::DeviceDisconnected(_) => None,
        CentralEvent::DeviceLost(_) => None,
        CentralEvent::DeviceConnected(_) => None,
    }
}

struct WatchdogImpl {
    duration: Duration,
    end: Instant,
}

#[derive(Clone)]
struct Watchdog(Arc<Mutex<WatchdogImpl>>);

impl Watchdog {
    fn new(duration: Duration) -> Self {
        Watchdog(Arc::new(Mutex::new(WatchdogImpl {
            duration,
            end: Instant::now() + duration,
        })))
    }

    async fn pet(&self) {
        let lock = &mut self.0.lock().await;
        lock.end = Instant::now() + lock.duration;
    }

    async fn wait(&self) {
        loop {
            if let Ok(lock) = self.0.try_lock() {
                let now = Instant::now();
                if now >= lock.end {
                    return;
                }
                delay_for(lock.end - now).await;
            } else {
                delay_for(Duration::from_millis(100)).await;
            }
        }
    }
}

#[derive(Clone)]
struct RuuviGauges {
    watchdogs: Arc<Mutex<HashMap<BDAddr, Watchdog>>>,
    temperature: GaugeVec,
    humidity: GaugeVec,
    pressure: GaugeVec,
    battery_potential: GaugeVec,
    movement_counter: IntGaugeVec,
    sequence_number: IntGaugeVec,
}

impl RuuviGauges {
    pub async fn update_sensor_values(&self, address: BDAddr, values: SensorValues) {
        let watchdogs = &mut self.watchdogs.lock().await;
        let watchdog = {
            let watchdog = watchdogs.get(&address);
            if let Some(watchdog) = watchdog {
                watchdog
            } else {
                println!("Found {}", address);

                // Create new watchdog
                let watchdog = Watchdog::new(Duration::from_secs(5));
                watchdogs.insert(address.clone(), watchdog.clone());

                // Implement removing functionality
                {
                    let saved_address = address.clone();
                    let watchdogs = self.watchdogs.clone();
                    let me = self.clone();
                    tokio::spawn(async move {
                        watchdog.wait().await;
                        let watchdogs = &mut watchdogs.lock().await;
                        watchdogs.remove(&saved_address);
                        println!("Lost {}", address);

                        me.remove_sensor_values(address);
                    });
                }
                watchdogs.get(&address).unwrap()
            }
        };
        watchdog.pet().await;

        let address = format!("{}", address);
        if let Some(temperature) = values.temperature_as_millicelsius() {
            self.temperature
                .with_label_values(&[&address])
                .set(f64::from(temperature) * 1e-3);
        }
        if let Some(humidity) = values.humidity_as_ppm() {
            self.humidity
                .with_label_values(&[&address])
                .set(f64::from(humidity) * 1e-4);
        }
        if let Some(pressure) = values.pressure_as_pascals() {
            self.pressure
                .with_label_values(&[&address])
                .set(f64::from(pressure) * 1e-3);
        }
        if let Some(potential) = values.battery_potential_as_millivolts() {
            self.battery_potential
                .with_label_values(&[&address])
                .set(f64::from(potential));
        }
        if let Some(counter) = values.movement_counter() {
            self.movement_counter
                .with_label_values(&[&address])
                .set(counter.into());
        }
        if let Some(sequence) = values.measurement_sequence_number() {
            self.sequence_number
                .with_label_values(&[&address])
                .set(sequence.into());
        }
    }

    fn remove_sensor_values(&self, address: BDAddr) {
        let address = format!("{}", address);

        if let Err(_) = self
            .temperature
            .remove_label_values(&[&address])
            .and_then(|_| self.humidity.remove_label_values(&[&address]))
            .and_then(|_| self.pressure.remove_label_values(&[&address]))
            .and_then(|_| self.battery_potential.remove_label_values(&[&address]))
            .and_then(|_| self.movement_counter.remove_label_values(&[&address]))
            .and_then(|_| self.sequence_number.remove_label_values(&[&address]))
        {
            println!("Failed to remove sensor {} from the export", address);
        }
    }
}

fn create_sensor_metrics(registry: &Registry) -> RuuviGauges {
    // Fill in sensor metrics
    let gauges = RuuviGauges {
        watchdogs: Arc::new(Mutex::new(HashMap::new())),
        temperature: GaugeVec::new(
            Opts::new("ruuvi_temperature", "temperature reported by ruuvi sensor"),
            &["address"],
        )
        .unwrap(),
        humidity: GaugeVec::new(
            Opts::new("ruuvi_humidity", "humidity reported by ruuvi sensor"),
            &["address"],
        )
        .unwrap(),
        pressure: GaugeVec::new(
            Opts::new("ruuvi_pressure", "pressure reported by ruuvi sensor"),
            &["address"],
        )
        .unwrap(),
        battery_potential: GaugeVec::new(
            Opts::new(
                "ruuvi_battery_potential",
                "battery_potential reported by ruuvi sensor",
            ),
            &["address"],
        )
        .unwrap(),
        movement_counter: IntGaugeVec::new(
            Opts::new(
                "ruuvi_movement_counter",
                "movement_counter reported by ruuvi sensor",
            ),
            &["address"],
        )
        .unwrap(),
        sequence_number: IntGaugeVec::new(
            Opts::new(
                "ruuvi_sequence_number",
                "sequence_number reported by ruuvi sensor",
            ),
            &["address"],
        )
        .unwrap(),
    };

    // Register all of them
    registry
        .register(Box::new(gauges.temperature.clone()))
        .unwrap();
    registry
        .register(Box::new(gauges.humidity.clone()))
        .unwrap();
    registry
        .register(Box::new(gauges.pressure.clone()))
        .unwrap();
    registry
        .register(Box::new(gauges.battery_potential.clone()))
        .unwrap();
    registry
        .register(Box::new(gauges.movement_counter.clone()))
        .unwrap();
    registry
        .register(Box::new(gauges.sequence_number.clone()))
        .unwrap();

    gauges
}

#[tokio::main]
async fn main() {
    // Setup sensor metrics
    let registry = Registry::new();
    let gauges = Arc::new(create_sensor_metrics(&registry));

    // Get a bluetooth adapter and setup it
    let manager = Manager::new().unwrap();
    let adapters = manager.adapters().unwrap();
    let adapter = adapters.into_iter().nth(0).unwrap();
    let adapter = manager.down(&adapter).unwrap();
    let adapter = manager.up(&adapter).unwrap();
    let central = Arc::new(adapter.connect().unwrap());

    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);

    // Setup event handler for ble events
    let closure_central = central.clone();
    let closure_gauges = gauges.clone();
    let tx = RefCell::new(tx);
    central.on_event(Box::new(move |event| {
        tx.borrow_mut()
            .try_send(event)
            .expect("Failed to send BLE event for handling");
    }));
    central.active(false);
    central.filter_duplicates(false);

    tokio::spawn(async move {
        loop {
            if let Some(event) = rx.recv().await {
                if let Some((address, values)) = parse_event(&closure_central, event) {
                    closure_gauges.update_sensor_values(address, values).await;
                }
            }
        }
    });

    tokio::spawn(async move {
        loop {
            central.start_scan().unwrap();
            tokio::time::delay_for(Duration::from_secs(120)).await;
            central.stop_scan().unwrap();
        }
    });

    let addr = SocketAddr::from(([0, 0, 0, 0], 8000));

    let registry = Arc::new(registry);
    let make_rvc = make_service_fn(|_conn| {
        let registry = registry.clone();
        async move {
            let service = service_fn(move |req: Request<Body>| {
                let registry = registry.clone();
                async move {
                    let mut response = Response::new(Body::empty());

                    match (req.method(), req.uri().path()) {
                        (&Method::GET, "/metrics") => {
                            let mut buffer = vec![];
                            let encoder = TextEncoder::new();
                            let metric_families = registry.gather();
                            encoder.encode(&metric_families, &mut buffer).unwrap();
                            *response.body_mut() = Body::from(buffer);
                        }
                        _ => {
                            *response.status_mut() = StatusCode::NOT_FOUND;
                        }
                    }

                    Ok::<_, Infallible>(response)
                }
            });
            Ok::<_, Infallible>(service)
        }
    });

    let server = Server::bind(&addr).serve(make_rvc);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
