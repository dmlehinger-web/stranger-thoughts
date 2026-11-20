use lambda_runtime::{service_fn, Error, LambdaEvent};

#[tokio::main]
async fn main() -> Result<(), Error> {
    stranger_api::init_tracing();
    lambda_runtime::run(service_fn(|event: LambdaEvent<serde_json::Value>| async move {
        stranger_api::handle_notify(event.payload).await
    }))
    .await
}
