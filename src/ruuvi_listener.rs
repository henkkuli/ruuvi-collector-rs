use crate::ruuvi_gauges::RuuviGauges;
use btleplug::{
    api::{BDAddr, Central, CentralEvent, Peripheral},
    bluez::{adapter::ConnectedAdapter, manager::Manager},
};
use ruuvi_sensor_protocol::SensorValues;
use std::{
    convert::TryInto,
    sync::{atomic::Ordering, Arc},
    thread::spawn,
};

/// Parses manufacturer-specific data.
fn parse_manufacturer_data(data: &Vec<u8>) -> Option<SensorValues> {
    if data.len() >= 2 {
        let id = u16::from_le_bytes(data[0..2].try_into().unwrap());
        ruuvi_sensor_protocol::SensorValues::from_manufacturer_specific_data(id, &data[2..]).ok()
    } else {
        None
    }
}

/// Handles bluetooth event.
fn on_event_with_address(
    central: &ConnectedAdapter,
    address: BDAddr,
) -> Option<(BDAddr, SensorValues)> {
    parse_manufacturer_data(
        &central
            .peripheral(address)?
            .properties()
            .manufacturer_data?,
    )
    .and_then(|data| Some((address, data)))
}

/// Parses bluetooth event.
pub fn parse_event(
    central: &ConnectedAdapter,
    event: CentralEvent,
) -> Option<(BDAddr, SensorValues)> {
    match event {
        CentralEvent::DeviceDiscovered(address) => on_event_with_address(central, address),
        CentralEvent::DeviceUpdated(address) => on_event_with_address(central, address),
        CentralEvent::DeviceDisconnected(_) => None,
        CentralEvent::DeviceLost(_) => None,
        CentralEvent::DeviceConnected(_) => None,
    }
}

/// Starts for listening for tags.
pub fn listen_for_tags(gauges: RuuviGauges) {
    // Get a bluetooth adapter and setup it
    let manager = Manager::new().unwrap();
    let adapters = manager.adapters().unwrap();
    let adapter = adapters.into_iter().nth(0).unwrap();
    let adapter = manager.down(&adapter).unwrap();
    let adapter = manager.up(&adapter).unwrap();
    let central = Arc::new(adapter.connect().unwrap());
    central.scan_enabled.store(false, Ordering::SeqCst);
    central.filter_duplicates(false);

    // Create a channel between rumble callback events and tokio async handler
    let receiver = central.event_receiver().unwrap();

    // Handle ble events from the channel
    spawn({
        let central = central.clone();
        move || loop {
            if let Ok(event) = receiver.recv() {
                if let Some((address, values)) = parse_event(&central, event) {
                    gauges.update_sensor_values(address, values);
                }
            } else {
                eprintln!("Error receiving messages");
            }
        }
    });

    // Listen for tags
    central.start_scan().unwrap();
}
