use btleplug::api::BDAddr;
use prometheus::{GaugeVec, IntGaugeVec, Opts, Registry};
use ruuvi_sensor_protocol::{
    BatteryPotential, Humidity, MeasurementSequenceNumber, MovementCounter, Pressure, SensorValues,
    Temperature,
};
use std::{collections::HashMap, sync::Arc, sync::Mutex, time::Duration};
use tokio::time::{interval, Instant};

/// Run cleanup job for old sensors every second.
const CLEANUP_PERIOD: Duration = Duration::from_secs(1);
/// Consider tags stale and lost if they haven't been sen for 10 seconds.
const STALE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct RuuviGauges {
    temperature: GaugeVec,
    humidity: GaugeVec,
    pressure: GaugeVec,
    battery_potential: GaugeVec,
    movement_counter: IntGaugeVec,
    sequence_number: IntGaugeVec,
    last_seen: Arc<Mutex<HashMap<String, Instant>>>,
}

impl RuuviGauges {
    /// Creates new gauges and registers them for the given registry.
    pub fn create_and_register(registry: &Registry) -> Self {
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
            last_seen: Default::default(),
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

        // A periodic job for removing too old sensor readings.
        // XXX: This job is not removed if the gauges gets dropped.
        tokio::spawn({
            let gauges = gauges.clone();
            async move {
                let mut interval = interval(CLEANUP_PERIOD);
                loop {
                    interval.tick().await;
                    let mut last_seen = gauges.last_seen.lock().unwrap();
                    let now = Instant::now();
                    last_seen.retain(|address, &mut value| {
                        if now - value < STALE_TIMEOUT {
                            true
                        } else {
                            gauges.remove_sensor_values(address);
                            false
                        }
                    });
                }
            }
        });

        gauges
    }

    /// Updates the exposed sensor values for the given sensor.
    pub fn update_sensor_values(&self, address: BDAddr, values: SensorValues) {
        let address = format!("{}", address);

        // Update last seen status
        {
            let mut last_seen = self.last_seen.lock().unwrap();
            last_seen.insert(address.clone(), Instant::now());
        }

        // Update each sensor reading, or remove if the value couldn't be parsed.
        if let Some(temperature) = values.temperature_as_millicelsius() {
            self.temperature
                .with_label_values(&[&address])
                .set(f64::from(temperature) * 1e-3);
        } else {
            let _ = self.temperature.remove_label_values(&[&address]);
        }

        if let Some(humidity) = values.humidity_as_ppm() {
            self.humidity
                .with_label_values(&[&address])
                .set(f64::from(humidity) * 1e-4);
        } else {
            let _ = self.humidity.remove_label_values(&[&address]);
        }

        if let Some(pressure) = values.pressure_as_pascals() {
            self.pressure
                .with_label_values(&[&address])
                .set(f64::from(pressure) * 1e-3);
        } else {
            let _ = self.pressure.remove_label_values(&[&address]);
        }

        if let Some(potential) = values.battery_potential_as_millivolts() {
            self.battery_potential
                .with_label_values(&[&address])
                .set(f64::from(potential));
        } else {
            let _ = self.battery_potential.remove_label_values(&[&address]);
        }

        if let Some(counter) = values.movement_counter() {
            self.movement_counter
                .with_label_values(&[&address])
                .set(counter.into());
        } else {
            let _ = self.movement_counter.remove_label_values(&[&address]);
        }

        if let Some(sequence) = values.measurement_sequence_number() {
            self.sequence_number
                .with_label_values(&[&address])
                .set(sequence.into());
        } else {
            let _ = self.sequence_number.remove_label_values(&[&address]);
        }
    }

    /// Removes sensor values for the given address from the exposed metrics.
    fn remove_sensor_values(&self, address: &str) {
        // Explicitly ignore the removal status. Either the value is removed correctly, or it never existed in the first
        // place.
        let _ = self.temperature.remove_label_values(&[&address]);
        let _ = self.humidity.remove_label_values(&[&address]);
        let _ = self.pressure.remove_label_values(&[&address]);
        let _ = self.battery_potential.remove_label_values(&[&address]);
        let _ = self.movement_counter.remove_label_values(&[&address]);
        let _ = self.sequence_number.remove_label_values(&[&address]);
    }
}
