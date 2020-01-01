use hyper::{
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server, StatusCode,
};
use prometheus::{Encoder, Registry, TextEncoder};
use std::{convert::Infallible, net::SocketAddr};

pub async fn create_metrics_server(registry: Registry) -> Result<(), hyper::error::Error> {
    let addr = SocketAddr::from(([0, 0, 0, 0], 8000));

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

    Server::bind(&addr).serve(make_rvc).await
}
