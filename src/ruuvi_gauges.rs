use crate::watchdog::Watchdog;
use prometheus::{GaugeVec, IntGaugeVec, Opts, Registry};
use rumble::api::BDAddr;
use ruuvi_sensor_protocol::{
    BatteryPotential, Humidity, MeasurementSequenceNumber, MovementCounter, Pressure, SensorValues,
    Temperature,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct RuuviGauges {
    watchdogs: Arc<Mutex<HashMap<BDAddr, Watchdog>>>,
    temperature: GaugeVec,
    humidity: GaugeVec,
    pressure: GaugeVec,
    battery_potential: GaugeVec,
    movement_counter: IntGaugeVec,
    sequence_number: IntGaugeVec,
}

impl RuuviGauges {
    pub fn create_and_register(registry: &Registry) -> Self {
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

    pub async fn update_sensor_values(&self, address: BDAddr, values: SensorValues) {
        let watchdogs = &mut self.watchdogs.lock().await;
        let watchdog = {
            let watchdog = watchdogs.get(&address);
            if let Some(watchdog) = watchdog {
                watchdog
            } else {
                info!("Found {}", address);

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
                        info!("Lost {}", address);

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
            error!("Failed to remove sensor {} from the export", address);
        }
    }
}
