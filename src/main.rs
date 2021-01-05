use crate::metrics_server::serve_metrics;
use crate::ruuvi_gauges::RuuviGauges;
use crate::ruuvi_listener::listen_for_tags;
use prometheus::Registry;

#[macro_use]
extern crate log;

mod metrics_server;
mod ruuvi_gauges;
mod ruuvi_listener;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();

    // Setup sensor metrics
    let registry = Registry::new();
    let gauges = RuuviGauges::create_and_register(&registry);

    // Start listening for ruuvi tags
    listen_for_tags(gauges);

    // Start serving metrics
    if let Err(e) = serve_metrics(registry).await {
        error!("server error: {}", e);
    }
}
