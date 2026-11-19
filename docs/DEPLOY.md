# Deploying Stranger Thoughts v2

This repository does not deploy itself. `infra/package.json` makes `npm run deploy` exit before CDK runs. There is no AWS account, no DNS change, and no Stripe key in this pull request. The steps below are for the owner, after the security review is accepted.

## What you need before the first deploy

1. An AWS account, and an IAM principal that can bootstrap CDK and deploy this stack. The stack is `us-east-1` because the CloudFront web ACL has to live there.
2. A unique Cognito domain prefix: `-c cognitoDomainPrefix=your-prefix`.
3. An email address that can receive the budget alarm: `-c budgetEmail=you@example.com`.
4. For a production stage, an ACM certificate in `us-east-1` for the site hostname: `-c certificateArn=arn:aws:acm:us-east-1:ACCOUNT:certificate/ID`. Production synthesis refuses to run without it, so CloudFront can require TLS 1.2 (`TLSv1.2_2021`). A dev stage can synthesize without a certificate. That dev distribution uses CloudFront’s default viewer certificate. Attach a certificate before you send real mail.
5. The site hostname, passed as `-c siteHost=`. Use the hostname people will type. Callback URLs, CORS, and `APP_BASE_URL` are built from it. If you do not know the CloudFront domain yet, deploy once with a hostname you control, or deploy, read the `SiteUrl` output, set `siteHost` to that domain, and deploy again. Sign-in will not match the browser origin until `siteHost` is the origin people use.
6. Stripe **test** secret key and webhook signing secret. Write them to the two Secrets Manager secrets the stack creates. Do not commit them, paste them into a pull request, or leave them in a shell history you share.
7. `cargo-lambda` (or an equivalent cross-compile) that produces `linux/arm64` binaries for the `provided.al2023` runtime. The files in `infra/lambda-stubs/` exist so `cdk synth` works. A deploy refuses those stubs: each `bootstrap` must be an ELF binary.
8. Node 22 and the Rust toolchain pinned in `backend/rust-toolchain.toml` (1.100.0, with clippy).

Leave `-c enableGuardDuty` unset unless you want the detector. It is off. Leave `STRIPE_ALLOW_LIVE` alone. The stack sets it to the string `false`.

## Build the functions

From `backend/`:

```bash
cargo lambda build --release --arm64 --output-format zip
```

`cargo-lambda` writes one directory per binary. Point `LAMBDA_ASSET_ROOT` at the directory that contains `health/`, `profile/`, `matching/`, `address/`, `payments/`, `webhook/`, `account/`, `post_confirmation/`, and `pre_token/`, each with a `bootstrap` file. Then:

```bash
export LAMBDA_ASSET_ROOT=/absolute/path/to/those/directories
```

## Write the frontend config

The committed `frontend/js/config.js` has `mock: true`. That is what GitHub Pages and the local preview use. Before a real deploy, render a file you do not commit:

```bash
export COGNITO_DOMAIN=your-prefix.auth.us-east-1.amazoncognito.com
export COGNITO_CLIENT_ID=the-client-id-from-the-stack-output
export AWS_REGION=us-east-1
node infra/scripts/render-config.mjs
```

The script refuses a value that looks like a Stripe secret. After the file upload, restore the mock file:

```bash
git checkout -- frontend/js/config.js
```

Deploy with `-c productionConfig=true`. The stack throws if that flag is set and `config.js` still says `mock: true`.

## Bootstrap and deploy

From `infra/`, after `npm ci`:

```bash
npx cdk bootstrap aws://ACCOUNT/us-east-1
npx cdk deploy -c stage=dev \
  -c siteHost=YOUR_HOST \
  -c cognitoDomainPrefix=YOUR_PREFIX \
  -c budgetEmail=YOU@example.com \
  -c productionConfig=true
```

Add `-c certificateArn=...` for any stage where the viewer certificate should require TLS 1.2, and always for `stage=prod`.

`cdk deploy` is the owner’s command. It is not run from this pull request.

## Put the Stripe test secrets in place

The stack creates two secrets. The names are stack outputs:

- `stranger-thoughts/STAGE/stripe/secret_key`
- `stranger-thoughts/STAGE/stripe/webhook_secret`

Replace the placeholder JSON. The Lambdas read one field each.

```bash
aws secretsmanager put-secret-value \
  --secret-id stranger-thoughts/dev/stripe/secret_key \
  --secret-string '{"secret_key":"sk_test_REPLACE"}'

aws secretsmanager put-secret-value \
  --secret-id stranger-thoughts/dev/stripe/webhook_secret \
  --secret-string '{"webhook_secret":"whsec_REPLACE"}'
```

Use the test-mode secret and the signing secret from the test-mode webhook endpoint. The webhook URL is the API Gateway endpoint plus `/webhooks/stripe`, not the CloudFront `/api` path. Stripe calls API Gateway directly. One signing secret per stage.

The code rejects an `sk_live_` key unless `STRIPE_ALLOW_LIVE` is exactly `true`. The stack sets that variable to `false`. Do not change it until a later review says live mode is allowed.

In the Stripe dashboard (test mode), the webhook events to send are `checkout.session.completed` and `account.updated`.

## Custom domain, later

DNS and the certificate are separate from the first synth. When you have a name:

1. Request the certificate in `us-east-1`.
2. Deploy again with `-c siteHost=` set to that name and `-c certificateArn=` set to the certificate.
3. Point DNS at the CloudFront domain from the `SiteUrl` output.

Do not add a `CNAME` file to this repository for the waitlist site until the waitlist’s own cutover in the root README. The v2 hostname is independent of GitHub Pages.

## What a small month costs

These are list prices from the pages linked, for a rough picture at about 200 monthly active users and well under a gigabyte of data. They are not a quote.

| Piece | Basis | Rough month |
| --- | --- | --- |
| Cognito Plus | $0.020 per MAU, no free tier on Plus. 200 MAU. [Cognito pricing](https://aws.amazon.com/cognito/pricing/) | about $4 |
| WAF | $5 per web ACL, $1 per rule, $1 per managed rule group. This stack adds a rate rule plus `AWSManagedRulesCommonRuleSet` and `AWSManagedRulesKnownBadInputsRuleSet`, so about $8 before request charges ($0.60 per million). [WAF pricing](https://aws.amazon.com/waf/pricing/) | about $8 |
| KMS | $1 per customer managed key per month, plus rotation. 20,000 requests a month are free. [KMS pricing](https://aws.amazon.com/kms/pricing/) | about $1 |
| Secrets Manager | $0.40 per secret per month. Two secrets. [Secrets Manager pricing](https://aws.amazon.com/secrets-manager/pricing/) | about $0.80 |
| Lambda | $0.20 per million requests. 1 million requests and 400,000 GB-seconds free. [Lambda pricing](https://aws.amazon.com/lambda/pricing/) | about $0 at this size |
| HTTP API | $1.00 per million after the free tier. [API Gateway pricing](https://aws.amazon.com/api-gateway/pricing/) | about $0 at this size |
| DynamoDB | On-demand example rates $0.625 per million writes and $0.125 per million reads. Storage $0.25/GB after 25 GB. Point-in-time recovery $0.20 per GB-month. [DynamoDB pricing](https://aws.amazon.com/dynamodb/pricing/) | about $1 |
| CloudFront and S3 | Pay-as-you-go, price class 100. [CloudFront pricing](https://aws.amazon.com/cloudfront/pricing/), [S3 pricing](https://aws.amazon.com/s3/pricing/) | typically under $1 |
| CloudTrail | The first management-events trail is free. [CloudTrail pricing](https://aws.amazon.com/cloudtrail/pricing/) | $0 |
| Budgets | The budget alarm itself is free. | $0 |

That is about $16 in a quiet month, under the $20 alarm (80% and 100%). Cognito Plus is what moves it: 500 MAU is about $10 of Cognito before the rest of the stack. Pass `-c enableWaf=false` only if you accept dropping the web ACL. GuardDuty is off. Its price is on the [GuardDuty pricing page](https://aws.amazon.com/guardduty/pricing/); this document does not guess it.

Plus is the tier that provides the compromised-credential check. Essentials is cheaper and has a free tier, and it does not provide that check. The pool is Plus for that reason.
