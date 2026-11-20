import assert from "node:assert/strict";
import { test } from "node:test";
import { App, Aspects } from "aws-cdk-lib";
import { Annotations, Match, Template } from "aws-cdk-lib/assertions";
import { AwsSolutionsChecks } from "cdk-nag";
import { StrangerThoughtsStack } from "../lib/stack.js";
import { contentSecurityPolicy } from "../lib/headers.js";

function stack() {
  const app = new App();
  return new StrangerThoughtsStack(app, "Test", {
    env: { account: "204161828199", region: "us-east-1" },
    stage: "dev",
    siteHost: "preview.invalid",
    cognitoDomainPrefix: "stranger-thoughts-dev-preview",
    enableWaf: false,
    enableBotControl: false,
    enableSesIdentity: false,
    sesEnabled: false,
    alertsTopicArn: "arn:aws:sns:us-east-1:204161828199:st-security-alerts",
    sesIdentityArn: "arn:aws:ses:us-east-1:204161828199:identity/astrangerreads.com",
    sesFromAddress: "desk@astrangerreads.com",
    maxRotatesPerDay: "3",
    maxAddressReadsPerDay: "40",
    certificateArn: "arn:aws:acm:us-east-1:204161828199:certificate/00000000-0000-0000-0000-000000000000",
  });
}

function template() {
  return Template.fromStack(stack());
}

test("content security policy names the required directives", () => {
  const policy = contentSecurityPolicy({
    cognitoDomain: "stranger-thoughts-dev-preview",
    region: "us-east-1",
  });
  for (const directive of [
    "default-src 'self'",
    "script-src 'self'",
    "object-src 'none'",
    "base-uri 'self'",
    "frame-ancestors 'none'",
    "form-action 'self' https://checkout.stripe.com https://connect.stripe.com",
  ]) {
    assert.ok(policy.includes(directive), directive);
  }
  assert.equal(policy.includes("'unsafe-inline'"), false);
  assert.equal(policy.includes("https://js.stripe.com"), false);
  assert.ok(policy.includes("https://stranger-thoughts-dev-preview.auth.us-east-1.amazoncognito.com"));
});

test("synth keeps the bucket private and uses origin access control", () => {
  const synthesized = template();
  synthesized.hasResourceProperties("AWS::S3::Bucket", {
    PublicAccessBlockConfiguration: {
      BlockPublicAcls: true,
      BlockPublicPolicy: true,
      IgnorePublicAcls: true,
      RestrictPublicBuckets: true,
    },
    OwnershipControls: {
      Rules: [{ ObjectOwnership: "BucketOwnerEnforced" }],
    },
  });
  synthesized.resourceCountIs("AWS::CloudFront::OriginAccessControl", 1);
  synthesized.resourceCountIs("AWS::CloudFront::CloudFrontOriginAccessIdentity", 0);
  const distribution = JSON.stringify(synthesized.findResources("AWS::CloudFront::Distribution"));
  assert.match(distribution, /TLSv1\.2_2021/);
  assert.match(distribution, /TLSv1\.2/);
  const headers = JSON.stringify(synthesized.findResources("AWS::CloudFront::ResponseHeadersPolicy"));
  assert.match(headers, /default-src 'self'/);
  assert.match(headers, /frame-ancestors 'none'/);
  assert.match(headers, /strict-origin-when-cross-origin/);
  assert.match(headers, /ContentTypeOptions/);
  assert.match(headers, /63072000/);
});

test("only the address function can decrypt, and not with a wildcard", () => {
  const synthesized = template();
  const policies = Object.values(synthesized.findResources("AWS::IAM::Policy"));
  const decryptRoles: string[] = [];
  for (const policy of policies) {
    const statements = policy.Properties?.PolicyDocument?.Statement ?? [];
    const list = Array.isArray(statements) ? statements : [statements];
    for (const statement of list) {
      const actions = [].concat(statement.Action ?? []);
      const resources = [].concat(statement.Resource ?? []);
      const sensitive = actions.some((action: string) =>
        /^(dynamodb|kms|s3|ssm|secretsmanager):/.test(action) || action === "*"
      );
      if (sensitive) {
        assert.equal(resources.includes("*"), false, JSON.stringify(statement));
      }
      if (actions.includes("kms:Decrypt")) {
        decryptRoles.push(statement);
        assert.equal(actions.includes("kms:GenerateDataKey"), false);
      }
      if (actions.some((action: string) => action.startsWith("ssm:"))) {
        assert.deepEqual(actions, ["ssm:GetParameter"]);
      }
      assert.equal(actions.some((action: string) => String(action).startsWith("secretsmanager:")), false);
      if (actions.some((action: string) => action.startsWith("dynamodb:"))) {
        for (const resource of resources) {
          assert.notEqual(resource, "*");
        }
      }
    }
  }
  const plainDecrypt = decryptRoles.filter((statement) => !JSON.stringify(statement).includes("kms:ViaService"));
  const viaDecrypt = decryptRoles.filter((statement) => JSON.stringify(statement).includes("kms:ViaService"));
  assert.equal(plainDecrypt.length, 1);
  assert.ok(viaDecrypt.length >= 1);
  for (const statement of viaDecrypt) {
    assert.match(JSON.stringify(statement), /ssm\.us-east-1\.amazonaws\.com/);
  }
});

test("stripe stays in test mode and the default template stays inside the boundary", () => {
  const synthesized = template();
  assert.equal(stack().stackName, "st-dev");
  const body = JSON.stringify(synthesized.toJSON());
  assert.match(body, /STRIPE_ALLOW_LIVE/);
  assert.match(body, /\/st\/stripe\/api-key/);
  assert.match(body, /\/st\/stripe\/webhook-secret/);
  assert.equal(body.includes("sk_live_"), false);
  assert.equal(body.includes("sk_test_"), false);
  synthesized.resourceCountIs("AWS::WAFv2::WebACL", 0);
  synthesized.resourceCountIs("AWS::KMS::Key", 1);
  synthesized.resourceCountIs("AWS::Budgets::Budget", 0);
  synthesized.resourceCountIs("AWS::CloudTrail::Trail", 0);
  synthesized.resourceCountIs("AWS::GuardDuty::Detector", 0);
  synthesized.resourceCountIs("AWS::SecretsManager::Secret", 0);
  synthesized.resourceCountIs("AWS::SSM::Parameter", 0);
  synthesized.resourceCountIs("AWS::SES::EmailIdentity", 0);
  synthesized.resourceCountIs("AWS::SES::ConfigurationSet", 0);
  synthesized.resourceCountIs("AWS::SNS::Topic", 0);
  synthesized.resourceCountIs("AWS::SNS::Subscription", 0);
  const roles = synthesized.findResources("AWS::IAM::Role");
  for (const role of Object.values(roles)) {
    assert.match(JSON.stringify(role.Properties?.PermissionsBoundary ?? ""), /st-boundary/);
  }
  const functions = synthesized.findResources("AWS::Lambda::Function");
  for (const fn of Object.values(functions)) {
    assert.ok(Number(fn.Properties?.ReservedConcurrentExecutions) >= 1);
  }
  const tables = synthesized.findResources("AWS::DynamoDB::Table");
  assert.ok(Object.keys(tables).length >= 7);
  for (const item of Object.values(tables)) {
    assert.equal(item.Properties?.DeletionProtectionEnabled, true);
    assert.equal(item.Properties?.SSESpecification?.SSEType, "KMS");
  }
  const alarms = synthesized.findResources("AWS::CloudWatch::Alarm");
  const names = Object.values(alarms).map((item) => item.Properties?.AlarmName);
  for (const name of [
    "st-dev-api-5xx",
    "st-dev-api-4xx",
    "st-dev-lambda-errors",
    "st-dev-lambda-throttles",
    "st-dev-lambda-concurrency",
    "st-dev-dynamodb-throttles",
  ]) {
    assert.ok(names.includes(name), name);
  }
  assert.equal(Object.keys(alarms).length, 6);
  const routes = Object.values(synthesized.findResources("AWS::ApiGatewayV2::Route"));
  const open = routes
    .filter((route) => route.Properties?.AuthorizationType !== "JWT")
    .map((route) => route.Properties?.RouteKey)
    .sort();
  assert.deepEqual(open, ["GET /health", "POST /webhooks/stripe"]);
});

test("cdk-nag AwsSolutionsChecks has no unsuppressed findings", () => {
  const app = new App();
  Aspects.of(app).add(new AwsSolutionsChecks({ verbose: true }));
  const synthesized = new StrangerThoughtsStack(app, "Nag", {
    env: { account: "204161828199", region: "us-east-1" },
    stage: "dev",
    siteHost: "preview.invalid",
    cognitoDomainPrefix: "stranger-thoughts-dev-preview",
    enableWaf: false,
    enableBotControl: false,
    enableSesIdentity: false,
    sesEnabled: false,
    alertsTopicArn: "arn:aws:sns:us-east-1:204161828199:st-security-alerts",
    sesIdentityArn: "arn:aws:ses:us-east-1:204161828199:identity/astrangerreads.com",
    sesFromAddress: "desk@astrangerreads.com",
    maxRotatesPerDay: "3",
    maxAddressReadsPerDay: "40",
    certificateArn: "arn:aws:acm:us-east-1:204161828199:certificate/00000000-0000-0000-0000-000000000000",
  });
  const errors = Annotations.fromStack(synthesized).findError("*", Match.anyValue());
  const warnings = Annotations.fromStack(synthesized).findWarning("*", Match.anyValue());
  const lines = [...errors, ...warnings].map((item) => `${item.id} ${String(item.entry.data).slice(0, 180)}`);
  assert.deepEqual(lines, []);
});

test("cognito client has no secret and refresh rotation is on", () => {
  const synthesized = template();
  synthesized.hasResourceProperties("AWS::Cognito::UserPoolClient", {
    GenerateSecret: false,
    AllowedOAuthFlows: ["code"],
    EnableTokenRevocation: true,
    AccessTokenValidity: 15,
    RefreshTokenRotation: { Feature: "ENABLED" },
  });
  synthesized.hasResourceProperties("AWS::Cognito::UserPool", {
    UserPoolTier: "PLUS",
  });
});
