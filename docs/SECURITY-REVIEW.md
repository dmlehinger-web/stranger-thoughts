# Security review

Status: review artifact for the pull request. Nothing here is deployed. This agent did not create an AWS account, change DNS, or call Stripe.

The owner has approved two sensitive collections, and the schema keeps them: optional writer descriptors (including race and an economic-status range), and reader mailing addresses. The controls below are the condition for that approval.

## Data inventory

| Data | Why | Where | Who can read it |
| --- | --- | --- | --- |
| Email | Account recovery and the verification gate | Cognito only. The pre-token Lambda strips it from both tokens and from the event it returns | Cognito. API Lambdas do not store it and do not log it |
| Pseudonym | The name the other person sees | Users or Readers profile row | The account owner, and the person matched with them (the name only) |
| Age range, region, race, economic status | Optional self-description. Not an input to matching | Separate attributes on the writer profile (`ageRange`, `locationBucket`, `race`, `economicStatus`) | The writer, via their own session. Readers never receive them. Matching reads the ActiveReaders index, which projects `pseudonym` and `blockedWriterIds` only |
| Mailing address | So the current writer can send a letter | Readers table, sort key `ADDRESS`: `ciphertext`, `encryptedDataKey`, `nonce`. Plaintext is not stored | The address Lambda, after the app check, and only the writer whose match is current. Decrypt permission exists on that function alone |
| Match log | History, the sent checkbox, and tip routing | Matches table: `matchId`, `writerId`, `matchedAt`, both pseudonyms, `sent`, `current`, `readerId` | The writer sees pseudonym, date, and sent. The reader sees the writer pseudonym and date. No address attribute exists on the item |
| Block list | A reader can refuse a writer | `blockedWriterIds` on the reader profile | Used as a safety filter before the random draw. It is not a demographic |
| Stripe connected account id | Route a tip to the matched reader | Readers profile, `stripeAccountId` | Payments Lambda, by a projected read. The browser never sends an account id |
| Tip amount and status | Reconciliation | Tips table. Idempotency rows use `tipId = event#{id}` | The reader sees a total. The writer sees the Checkout redirect |
| Rate counters | Cap address reads | Users sort key `RATE`: window start and count | The address Lambda |

Age ranges are `18-24`, `25-34`, `35-44`, `45-54`, `55-64`, and `65+`. `under-13`, `0-12`, `13-17`, `child`, and similar buckets are rejected. The form does not offer them.

Street addresses are accepted because the owner asked for real mail. The product copy asks for a PO box or a virtual mailbox. The encryption and the reveal rule do not get weaker for a street address.

## Trust boundaries

```
Browser (public SPA, PKCE, no client secret)
  |  HTTPS, TLS 1.2+ on the API origin
  v
CloudFront  -- WAF (rate, common rules, known bad inputs)
  |  OAC to a private S3 bucket     |  /api/* strips the prefix
  v                                 v
S3 (static files)              API Gateway HTTP API
                                 |  JWT authorizer, except the two public routes
                                 v
                               Rust Lambdas (role checked again)
                                 |            |            |
                                 v            v            v
                              DynamoDB     KMS CMK      Secrets Manager
                              (ciphertext) (address     (Stripe test key,
                                            data key)    webhook secret)
Cognito user pool  ---- groups writer | reader ----> Lambdas
Stripe (hosted Checkout and Connect) -- signed webhook --> webhook Lambda
```

The browser is not a trust boundary for authorization. Every Lambda that touches a person checks the Cognito group. The UI hiding a button is not the control.

## Public routes

`backend/routes.json` is the list. A unit test includes that file and checks it.

| Method | Path | Why it is public |
| --- | --- | --- |
| GET | `/health` | Liveness. Returns `{"ok":true}` and nothing else |
| POST | `/webhooks/stripe` | Stripe cannot present a Cognito token. The handler rejects a missing or bad `Stripe-Signature`, uses the raw body for the HMAC, and ignores a timestamp older than 300 seconds |

Every other route in that file has the JWT authorizer. Sign-up and confirm-sign-up go to `cognito-idp` from the browser with the public client id. The password does not pass through this API.

## Authentication

- One public app client. `GenerateSecret` is false. The OAuth flow is authorization code with PKCE (`S256`). There is no client secret in the frontend or the repository.
- Access and ID tokens last 15 minutes. Refresh tokens last 30 days. Refresh token rotation is enabled with a 10 second grace period. Token revocation is enabled. Logout calls the Cognito revoke endpoint and then the logout endpoint.
- The password policy is a minimum of 15 characters with no composition rules, which matches the length guidance in NIST SP 800-63B. The pool is Cognito Plus with threat protection enforced, which is the compromised-credential check Cognito offers. Cognito does not expose a general HIBP-style lookup beyond that.
- Email is required and auto-verified. Profile, demographics, address, payouts, and account deletion call `require_verified` and refuse the write unless the token says `email_verified` is the string `true`.
- Groups are `writer` and `reader`. A post-confirmation Lambda adds the user to the group named by the immutable `custom:role` attribute (`writer` or `reader`). An invalid role gets no group, and the API then returns 403. A token in both groups is rejected.
- MFA is optional TOTP. SMS is off. The reader form tells people who store a mailing address to turn on an authenticator app.
- Tokens in the browser use `sessionStorage`. The mock preview may use `localStorage` for its fake letters. That store is not used for access tokens.

The post-confirmation role’s Cognito resource is `userpool/*` in the account. A policy that names this pool’s ARN cycles in CloudFormation, because the pool’s trigger depends on the function and the function’s policy would depend on the pool. The only action on that role is `cognito-idp:AdminAddUserToGroup`. Account deletion uses `cognito-idp:AdminDeleteUser` on this pool’s ARN, because that function is not a pool trigger. The username comes from the token, and it is not logged.

## Authorization and matching

`MatchCandidate` has four fields: reader id, pseudonym, active, and blocked writer ids. `choose_reader` takes a cryptographic RNG (`OsRng` in the Lambda, `ChaCha20Rng` in tests) and `SliceRandom::choose`, which is a uniform draw. The source test `source_does_not_reference_demographic_fields` fails the build if the matcher file, above its tests, names a demographic field. A distribution test draws 5,000 times across five readers and expects each count in a wide even band.

Blocks are applied before the draw. They are a safety filter.

Address reveal, in `authorize_address_read`:

- The caller is a verified writer.
- The match’s writer id is that caller.
- The match is the current one.

Anything else is `not_found`, including a historical match and a different writer, so the response is not an existence oracle. A reader principal cannot read an address. The address Lambda’s `GetItem` is limited to the address attributes and `Select = SPECIFIC_ATTRIBUTES`. No other function has `kms:Decrypt`. List responses (`WriterHomeView`, `LetterSummary`, `ReaderHomeView`) have no address, no email, and the writer home has no reader id.

Historical matches can still be tipped. The tip destination is the connected account stored for the match’s reader, loaded on the server. The request body has no reader id and no Stripe account field (`deny_unknown_fields`). A missing account is `payouts_unavailable`, with no fallback to another account.

Rate limits:

- Rotate: at most 3 matches in the last 24 hours (`MAX_ROTATES_PER_DAY`). API Gateway also throttles `POST /writers/rotate` to burst 2 and rate 1.
- Address reads: 40 per writer per day, plus API Gateway burst 5 and rate 2 on `GET /matches/{matchId}/address`.
- Default stage throttle is burst 20 and rate 10. The webhook route is burst 20 and rate 5.
- Cognito’s own auth endpoints are additionally covered by the user-pool threat protection and by the WAF on the site. The hosted auth calls go to Cognito’s domain, which is outside this API.

JSON bodies are capped at 8 KB. Unknown fields are rejected. Error bodies are static codes (`unauthorized`, `forbidden`, `email_unverified`, `not_found`, `invalid_request`, `child_age_rejected`, `body_too_large`, `rate_limited`, `no_reader`, `payouts_unavailable`, `live_mode_blocked`). They do not echo the request.

## Keys and retention

The address key is a customer-managed CMK with rotation enabled, alias `alias/stranger-thoughts-${stage}-addresses`. The profile Lambda may call `kms:GenerateDataKey` on that key. The address Lambda may call `kms:Decrypt` on that key. No other function is granted decrypt. Envelope encryption is AES-256-GCM. The nonce is 12 bytes. The additional data is `st-address-v1:{readerId}`. The data key is wrapped by KMS with encryption context `purpose = reader-mailing-address` and `readerId`. The plaintext key is zeroized on drop. `PostalAddress`’s debug output is redacted.

Stripe secrets live in Secrets Manager under the AWS-managed `aws/secretsmanager` key. Callers need `secretsmanager:GetSecretValue` on that secret’s ARN. They do not need `kms:Decrypt`, which keeps the “address Lambda only” decrypt rule intact. The placeholder value is the string `replace-me`. Templates do not contain `sk_test_`, `sk_live_`, or `whsec_`.

DynamoDB tables use the AWS-managed table key, on-demand billing, and point-in-time recovery. Streams are off. X-Ray tracing on the functions is off, so a trace does not capture an address payload.

Deleting an account:

- Writer: delete the profile (descriptors included), set match pseudonyms to `Removed`, clear `current`.
- Reader: delete the address ciphertext, remove `activePk` so matching stops, set the reader pseudonym to `Removed`, delete the profile.
- Tip rows stay, as ids and amounts, for reconciliation.
- Cognito `AdminDeleteUser` runs with the token’s `username`.

Point-in-time recovery can retain ciphertext for up to 35 days after a delete. That limit is a property of the backup, and the privacy draft says so.

## Stripe Connect

- Hosted Checkout and hosted Connect onboarding only. The client posts to `https://api.stripe.com/v1/checkout/sessions`, `/v1/accounts`, and `/v1/account_links`. Card fields are not accepted and are not present in the form builder.
- A reader tip is a destination charge to the Express account on the match. `application_fee_amount` is omitted because the platform fee is 0 (`platform_fee_cents()` returns 0). A later change that sets a fee has to send a positive integer; Stripe rejects zero.
- The builder tip is pay-what-you-want, with presets, and has no destination.
- Amounts are 100 to 20000 cents.
- Live mode: `enforce_stripe_mode` accepts an `sk_test_` key and rejects `sk_live_` unless the environment value is exactly `true`. The stack sets `STRIPE_ALLOW_LIVE=false`. There is no other switch.
- The webhook checks `Stripe-Signature` (HMAC-SHA256 over `{timestamp}.{raw body}`, tolerance 300 seconds, multiple `v1` signatures accepted). Unsigned, stale, and bad signatures are 401. Processing is idempotent: a conditional put of `event#{id}` with `attribute_not_exists`. The parser does not surface a customer email or address from the event.
- Connect Express KYC is Stripe’s. This service stores the resulting account id and the payouts-ready flag.

## Edge

- The site bucket blocks all public access, enforces SSL, uses bucket-owner-enforced ownership (no public ACLs), and is read through an origin access control. The template has no origin access identity.
- Response headers: HSTS two years with subdomains and preload; the CSP from `infra/lib/headers.ts`; `nosniff`; frame deny; referrer `strict-origin-when-cross-origin`.
- CSP: `default-src 'self'`; `script-src 'self'` (no third-party script, no `'unsafe-inline'`); `object-src 'none'`; `base-uri 'self'`; `frame-ancestors 'none'`; `form-action 'self' https://checkout.stripe.com https://connect.stripe.com`. `connect-src` adds this user pool’s hosted-UI host and the regional `cognito-idp` host, because sign-up and the token exchange are browser calls. There is no `js.stripe.com` and no Google Fonts host. Fonts are the woff2 files in `frontend/fonts/`, under the SIL Open Font License (`frontend/fonts/OFL.txt`).
- The HTML meta policy is a little wider on `connect-src` (a Cognito wildcard) so it does not intersect the header down to nothing. Browsers enforce both. `frame-ancestors` is sent as a header; a meta tag would ignore it. `frontend/devserver.py` sends the headers locally.
- API CORS allows `https://${siteHost}` only.
- The API origin speaks TLS 1.2 to API Gateway. API Gateway’s execute-api endpoint is TLS 1.2. Production synthesis requires an ACM certificate and sets the viewer policy to `TLSv1.2_2021`. A dev synth without `certificateArn` uses CloudFront’s default viewer certificate, which is the one case where this stack cannot raise the viewer minimum. Do not put real addresses on that distribution.

## IAM

Each function has its own role. Log permissions are `CreateLogStream` and `PutLogEvents` on that function’s log group. There is no `logs:*` managed policy. The policy test fails if a DynamoDB, KMS, S3, SSM, or Secrets Manager action uses `Resource: "*"`, and it fails if more than one statement grants `kms:Decrypt`.

| Function | DynamoDB | KMS | Other |
| --- | --- | --- | --- |
| health | none | none | logs |
| profile | Users update. Readers update and put (the sealed address item) | `GenerateDataKey` on the address CMK | |
| matching | Users get (pseudonym). Matches query, update, put, transact-write. ActiveReaders query. ByMatch query | none | |
| address | Users get and put (the rate row). Matches query and ByMatch query. Readers get, address attributes only | `Decrypt` on the address CMK | |
| payments | Matches query. Tips get, put, update. Readers get, profile attributes only. Readers update of `stripeAccountId` only | none | `GetSecretValue` on the secret-key secret |
| webhook | Tips get, put, update. Readers update of `payoutsReady` only | none | `GetSecretValue` on the webhook secret |
| account | Users update and delete. Readers get (profile attributes, not the address), update of `blockedWriterIds` and `activePk`, delete of the address item. Matches query and update. ByReader queries | none | `AdminDeleteUser` on this pool |
| post_confirmation | none | none | `AdminAddUserToGroup` on `userpool/*` in this account |
| pre_token | none | none | logs |

S3 writes for the site happen from the CDK deployment resource at deploy time, not from these functions. The bucket policy that origin access control installs is the CloudFront service principal with the distribution source ARN.

## Logging

Handlers log the path and the status code through `SafeLog`. The redactor masks emails, `sk_live_`, `sk_test_`, `rk_`, `pk_`, `whsec_`, and fields whose names are email, address, line, postal, token, secret, password, ciphertext, or stripe. Lambdas do not log bodies. Store errors are discarded rather than formatted, so an AWS SDK error cannot print an item.

## Threat notes

**IDOR on a match or an address.** The address route keys off the match id. The check is the caller’s writer id plus `current`, in the Lambda, and the row itself has no address to leak through the list APIs. A wrong id and a stale match both return `not_found`. The IAM projection is the second control: a bug that forgot the filter still could not ask DynamoDB for the ciphertext from the payments or account roles.

**Webhook replay.** The signature covers the raw body and the timestamp. A captured body fails after 300 seconds, and a new body fails the HMAC. A repeated delivery of a valid event hits the conditional put and is ignored. The signing secret is per stage.

**Cognito token theft.** Access tokens are short. Refresh tokens rotate, and logout revokes them where Cognito’s revoke endpoint allows. The client has no secret to steal from the bundle. PKCE binds the code exchange to the browser that started it. Tokens are not in `localStorage`.

**Stripe Connect fraud.** The connected account is read from the match’s reader, not from the request. A writer cannot aim a tip at an account they name. Live keys are rejected by the mode gate. Checkout is hosted, so this origin does not see card data. Connect Express identity checks are Stripe’s; a fake reader still has to pass Stripe before payouts flip to ready, and the webhook requires both `charges_enabled` and `payouts_enabled`.

## How to verify

Without an AWS account:

1. `cd backend && cargo test --workspace` — matching uniformity and the demographic source ban, rotate history, tip routing to the reader on the match, address authorization (current writer only), child-age rejection, webhook signature and idempotency, live-key rejection, redaction.
2. `cd backend && cargo clippy --workspace --all-targets -- -D warnings`.
3. `cd infra && npm test` — private bucket, origin access control and no origin access identity, CSP and HSTS on the response-headers policy, a single `kms:Decrypt`, no wildcard resource on DynamoDB, KMS, S3, SSM, or Secrets Manager, `STRIPE_ALLOW_LIVE` false, budget 20 USD, Cognito client `GenerateSecret: false`, refresh rotation enabled, Plus tier.
4. `cd infra && npx cdk synth -c stage=dev` — succeeds on the stub binaries. `npx cdk synth -c stage=prod` fails without `certificateArn`.
5. `python3 frontend/check.py` — no third-party script host, no inline handlers, committed config stays in mock mode, font files present.
6. `python3 scripts/build.py` — the waitlist site still generates, and its own CSP is untouched.
7. `python3 frontend/devserver.py` and walk the mock screens. Confirm the address card is only the current stranger, a rotate keeps the older names, the sent checkbox sticks, a letter tip names that reader, and the builder tip does not.

When an account exists, before real personal information:

8. Confirm the two secrets are JSON with a test key and a test webhook secret, and that CloudTrail shows no `GetSecretValue` from a principal other than the two functions.
9. Sign up, leave the email unconfirmed, and confirm the profile and address routes return `email_unverified`.
10. As writer A, request writer B’s match id and confirm `not_found` with an empty address.
11. Replay a webhook body without `Stripe-Signature` and confirm 401. Replay a signed event and confirm the second delivery does not double-apply.
12. Read a function log and confirm it has a path and a status, and no email, address, or token.

## Process notes for when an account exists

- CloudTrail is in the stack: management events, not data events, so DynamoDB item contents are not in the trail.
- GuardDuty is off unless `-c enableGuardDuty=true`. Turn it on when the account is real. Price it from the GuardDuty pricing page rather than from this document.
- WAF is on unless `-c enableWaf=false`. The common rule group can be sensitive to body patterns. Address bodies are small and go to the API, which is behind the same distribution. If a real address is blocked, note the sampled request before loosening a rule.
- The ActiveReaders index uses one partition key, `ACTIVE`. That is acceptable at the size in `docs/DEPLOY.md`. It is the wrong shape for a very large reader pool.
- Dependabot watches Cargo (`/backend`), npm (`/infra`), and GitHub Actions. CI runs tests, clippy, `cargo deny`, the CDK policy tests, synth, and the frontend checks.

## What is intentionally not live

- Lambda assets in the synth are stubs. Deploy refuses a non-ELF `bootstrap`.
- `frontend/js/config.js` is mock mode. Production config is rendered at deploy time and not committed.
- Stripe secret values are `replace-me` until the owner writes test-mode secrets.
- Privacy and terms are drafts (`docs/PRIVACY.md`, `docs/TERMS.md`, and the matching pages under `frontend/`).
