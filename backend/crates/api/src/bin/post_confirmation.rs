use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<(), Error> {
    stranger_api::init_tracing();
    lambda_runtime::run(service_fn(|event: LambdaEvent<Value>| async move {
        stranger_api::handle_post_confirmation(event.payload).await
    }))
    .await
}
