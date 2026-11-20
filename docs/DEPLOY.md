# Deploying Stranger Thoughts v2

This repository does not deploy itself. `infra/package.json` makes `npm run deploy` exit before CDK runs. There is no AWS account, no DNS change, and no Stripe key in this pull request. The steps below are for the owner, after the security review is accepted.

## What you need before the first deploy

1. AWS account `204161828199`, region `us-east-1` only. The stack name is `st-<stage>`. Every role uses the permissions boundary `st-boundary`.
2. A unique Cognito domain prefix: `-c cognitoDomainPrefix=your-prefix`.
3. For a production stage, an ACM certificate in `us-east-1` for the site hostname: `-c certificateArn=arn:aws:acm:us-east-1:204161828199:certificate/ID`. Production synthesis refuses to run without it, so CloudFront can require TLS 1.2 (`TLSv1.2_2021`). A dev stage can synthesize without a certificate.
4. The site hostname, passed as `-c siteHost=`. Use `astrangerreads.com` when DNS is ready. Callback URLs, CORS, and `APP_BASE_URL` are built from it.
5. Stripe test key and webhook signing secret, as SSM SecureString parameters you create yourself. The stack does not create them. See below. Select the app key `alias/st-<stage>` when you encrypt them.
6. `cargo-lambda` that produces `linux/arm64` binaries for `provided.al2023`. The files in `infra/lambda-stubs/` exist so `cdk synth` works. A deploy refuses those stubs.
7. Node 22 and the Rust toolchain pinned in `backend/rust-toolchain.toml` (1.100.0, with clippy).

Leave `enableWaf` and `enableBotControl` unset. Both default off. Leave `enableSesIdentity` unset. The stack refuses to create an SES identity because `st-cfn-exec` has no `ses:*`. Leave `sesEnabled` unset so mailed-letter email stays off until the domain identity is verified. `STRIPE_ALLOW_LIVE` is the string `false`. `MAILING_DESK_READY` is `false`. There is no budget resource: `st-boundary` denies budget changes. The account baseline already has CloudTrail, GuardDuty, and the spend alarm topic.

## Build the functions

From `backend/`:

```bash
cargo lambda build --release --arm64 --output-format zip
```

`cargo-lambda` writes one directory per binary, including `preservation/` and `notify/`. Point `LAMBDA_ASSET_ROOT` at the directory that contains those directories, each with a `bootstrap` file. Then:

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
npx --yes aws-cdk@2.1101.0 bootstrap aws://204161828199/us-east-1 \
  --cloudformation-execution-policies "arn:aws:iam::204161828199:policy/st-cfn-exec,arn:aws:iam::204161828199:policy/st-cfn-exec-guardrails" \
  --custom-permissions-boundary st-boundary \
  --termination-protection
```

`npm run deploy` still exits 1. A real deploy is the GitHub Actions workflow `.github/workflows/deploy.yml` (`workflow_dispatch`, mode `diff` or `deploy`, environment `production`), which assumes `arn:aws:iam::204161828199:role/st-deploy`. Deploy mode runs `cdk deploy` twice: the first pass reads `UserPoolClientId`, `CognitoDomain`, and `ApiUrl`, `infra/scripts/render-config.mjs` writes `config.js`, and the second pass uses `-c productionConfig=true`. Diff mode posts `cdk diff` and does not deploy.

Add `-c certificateArn=...` for any stage where the viewer certificate should require TLS 1.2, and always for `stage=prod`.

## Put the Stripe test parameters in place

Create these two SecureString parameters yourself, encrypted with `alias/st-<stage>`. The stack never writes `/st/stripe/*`. `st-cfn-exec` denies SSM writes on that path.

- `/st/stripe/api-key`
- `/st/stripe/webhook-secret`

Use a test-mode restricted key or a test-mode secret key, and the signing secret from the test-mode webhook endpoint. The webhook URL is the API Gateway endpoint plus `/webhooks/stripe`, not the CloudFront `/api` path. Stripe calls API Gateway directly.

The code rejects a live key (`rk_live_` or `sk_live_`) unless `STRIPE_ALLOW_LIVE` is exactly `true`. The stack sets that variable to `false`. Do not change it until a later review says live mode is allowed.

In the Stripe dashboard (test mode), the webhook events to send are `checkout.session.completed` and `account.updated`.

## Custom domain, later

DNS and the certificate are separate from the first synth. When you have a name:

1. Request the certificate in `us-east-1`.
2. Deploy again with `-c siteHost=` set to that name and `-c certificateArn=` set to the certificate.
3. Point DNS at the CloudFront domain from the `SiteUrl` output.

Do not add a `CNAME` file to this repository for the waitlist site until the waitlist’s own cutover in the root README. The v2 hostname is independent of GitHub Pages.

## What a spike can cost

WAF is off, so there is no $5 web ACL in the default stack. The blast-radius control is throttling and reserved concurrency, aimed at about $40 in a sustained spike:

- Stage default: 2 requests per second, burst 4.
- Sensitive routes (rotate, address, mail marks, preservation writes, account settings, health): 1 per second, burst 2.
- Stripe webhook: 5 per second, burst 10.
- API functions: reserved concurrency 2. Cognito triggers and the notifier: 1.

At 2 requests per second sustained, a month is about 5.2 million requests. API Gateway HTTP APIs, Lambda at 256 MB, DynamoDB on-demand writes, and logs for that volume stay under about $40. There is no CloudFormation budget. The account baseline owns CloudTrail and GuardDuty.

Plus is the Cognito tier that provides the compromised-credential check. The pool is Plus for that reason.

## Mailed-letter email

The notifier sends one static plain-text message when someone marks a letter or a reply mailed, if `sesEnabled` is true and the recipient has not opted out. The from address defaults to `desk@astrangerreads.com`. The identity ARN defaults to `arn:aws:ses:us-east-1:204161828199:identity/astrangerreads.com`. Verify that identity, a MAIL FROM subdomain, three DKIM CNAMEs, and DMARC outside this stack. New SES accounts are sandboxed until AWS approves production access. The apex can stay a null MX. Leave the apex SPF at `v=spf1 -all` until that DNS change. A failed send does not undo the in-app status.
