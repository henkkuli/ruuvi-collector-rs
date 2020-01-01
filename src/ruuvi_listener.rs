use crate::ruuvi_gauges::RuuviGauges;
use rumble::{
    api::{BDAddr, Central, CentralEvent, Peripheral},
    bluez::{adapter::ConnectedAdapter, manager::Manager},
};
use ruuvi_sensor_protocol::SensorValues;
use std::{cell::RefCell, convert::TryInto, sync::Arc, time::Duration};

fn parse_manufacturer_data(data: &Vec<u8>) -> Option<SensorValues> {
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
    parse_manufacturer_data(
        &central
            .peripheral(address)?
            .properties()
            .manufacturer_data?,
    )
    .and_then(|data| Some((address, data)))
}

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

pub async fn listen_for_tags(gauges: RuuviGauges) {
    // Get a bluetooth adapter and setup it
    let manager = Manager::new().unwrap();
    let adapters = manager.adapters().unwrap();
    let adapter = adapters.into_iter().nth(0).unwrap();
    let adapter = manager.down(&adapter).unwrap();
    let adapter = manager.up(&adapter).unwrap();
    let central = Arc::new(adapter.connect().unwrap());
    central.active(false);
    central.filter_duplicates(false);

    // Create a channel between rumble callback events and tokio async handler
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);

    let tx = RefCell::new(tx);
    central.on_event(Box::new(move |event| {
        if let Err(e) = tx.borrow_mut().try_send(event) {
            println!("Failed to send BLE event for handling: {}", e);
        }
    }));

    let closure_central = central.clone();
    // Handle ble events from the channel
    tokio::spawn(async move {
        loop {
            if let Some(event) = rx.recv().await {
                if let Some((address, values)) = parse_event(&closure_central, event) {
                    gauges.update_sensor_values(address, values).await;
                }
            }
        }
    });

    // Listen for tags
    loop {
        central.start_scan().unwrap();
        tokio::time::delay_for(Duration::from_secs(120)).await;
        central.stop_scan().unwrap();
    }
}
