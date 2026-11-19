use std::time::Duration;

use tokio::sync::OnceCell;

use crate::http::ApiFailure;

pub struct Aws {
    pub dynamo: aws_sdk_dynamodb::Client,
    pub kms: aws_sdk_kms::Client,
    pub secrets: aws_sdk_secretsmanager::Client,
    pub cognito: aws_sdk_cognitoidentityprovider::Client,
    pub http: reqwest::Client,
}

impl Aws {
    pub async fn get() -> Result<&'static Aws, ApiFailure> {
        static CELL: OnceCell<Aws> = OnceCell::const_new();
        CELL.get_or_try_init(|| async {
            let config = aws_config::load_from_env().await;
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(8))
                .build()
                .map_err(|_| ApiFailure::Invalid)?;
            Ok(Aws {
                dynamo: aws_sdk_dynamodb::Client::new(&config),
                kms: aws_sdk_kms::Client::new(&config),
                secrets: aws_sdk_secretsmanager::Client::new(&config),
                cognito: aws_sdk_cognitoidentityprovider::Client::new(&config),
                http,
            })
        })
        .await
    }
}
