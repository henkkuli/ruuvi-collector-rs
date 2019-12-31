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
    BatteryPotential, Humidity, MovementCounter, Pressure, SensorValues, Temperature,MeasurementSequenceNumber
};
use std::{
    convert::{Infallible, TryInto},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};

fn parse_data(data: &Vec<u8>) -> Option<SensorValues> {
    if data.len() > 2 {
        let id = u16::from_le_bytes(data[0..2].try_into().unwrap());
        ruuvi_sensor_protocol::SensorValues::from_manufacturer_specific_data(id, &data[2..]).ok()
    } else {
        None
    }
}

fn on_event_with_address(
    central: &ConnectedAdapter,
    address: BDAddr,
    gauges: &RuuviGauges,
) -> Option<()> {
    let peripheral = central.peripheral(address).unwrap();

    print!("{:?} ", address);

    let data = match peripheral.properties().manufacturer_data {
        Some(data) => parse_data(&data),
        None => None,
    };

    println!("{:?}", data);

    if let Some(data) = data {
        let address = format!("{}", address);
        if let Some(temperature) = data.temperature_as_millicelsius() {
            gauges
                .temperature
                .with_label_values(&[&address])
                .set(f64::from(temperature) * 1e-3);
        }
        if let Some(humidity) = data.humidity_as_ppm() {
            gauges
                .humidity
                .with_label_values(&[&address])
                .set(f64::from(humidity) * 1e-4);
        }
        if let Some(pressure) = data.pressure_as_pascals() {
            gauges
                .pressure
                .with_label_values(&[&address])
                .set(f64::from(pressure) * 1e-3);
        }
        if let Some(potential) = data.battery_potential_as_millivolts() {
            gauges
                .battery_potential
                .with_label_values(&[&address])
                .set(f64::from(potential));
        }
        if let Some(counter) = data.movement_counter() {
            gauges
                .movement_counter
                .with_label_values(&[&address])
                .set(counter.into());
        }
        if let Some(sequence) = data.measurement_sequence_number() {
            gauges
                .sequence_number
                .with_label_values(&[&address])
                .set(sequence.into());
        }
    }

    None
}

fn on_event(central: &ConnectedAdapter, event: CentralEvent, gauges: &RuuviGauges) {
    match event {
        CentralEvent::DeviceDiscovered(address) => on_event_with_address(central, address, gauges),
        CentralEvent::DeviceUpdated(address) => on_event_with_address(central, address, gauges),
        CentralEvent::DeviceDisconnected(_) => None,
        CentralEvent::DeviceLost(_) => None,
        CentralEvent::DeviceConnected(_) => None,
    };
}

#[derive(Clone)]
struct RuuviGauges {
    temperature: GaugeVec,
    humidity: GaugeVec,
    pressure: GaugeVec,
    battery_potential: GaugeVec,
    movement_counter: IntGaugeVec,
    sequence_number: IntGaugeVec,
}

fn create_sensor_metrics(registry: &Registry) -> RuuviGauges {
    // Fill in sensor metrics
    let gauges = RuuviGauges {
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
    let gauges = create_sensor_metrics(&registry);

    // Get a bluetooth adapter and setup it
    let manager = Manager::new().unwrap();
    let adapters = manager.adapters().unwrap();
    let adapter = adapters.into_iter().nth(0).unwrap();
    let adapter = manager.down(&adapter).unwrap();
    let adapter = manager.up(&adapter).unwrap();
    let central = Arc::new(adapter.connect().unwrap());

    // Setup event handler for ble events
    let closure_central = central.clone();
    let closure_gauges = gauges.clone();
    central.on_event(Box::new(move |event| {
        on_event(&closure_central, event, &closure_gauges)
    }));
    central.active(false);
    central.filter_duplicates(false);

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
