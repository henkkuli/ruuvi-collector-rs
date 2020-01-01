use crate::metrics_server::create_metrics_server;
use crate::ruuvi_gauges::RuuviGauges;
use crate::ruuvi_listener::listen_for_tags;
use prometheus::Registry;

#[macro_use]
extern crate log;

mod metrics_server;
mod ruuvi_gauges;
mod ruuvi_listener;
mod watchdog;

#[tokio::main]
async fn main() {
    env_logger::init();

    // Setup sensor metrics
    let registry = Registry::new();
    let gauges = RuuviGauges::create_and_register(&registry);

    // Start listening for ruuvi tags
    tokio::spawn(async {
        listen_for_tags(gauges).await;
    });

    // Start serving metrics
    if let Err(e) = create_metrics_server(registry).await {
        error!("server error: {}", e);
    }
}
