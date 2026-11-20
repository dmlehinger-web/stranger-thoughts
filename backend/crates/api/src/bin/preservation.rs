use lambda_http::{service_fn, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    stranger_api::init_tracing();
    lambda_http::run(service_fn(stranger_api::preservation)).await
}
