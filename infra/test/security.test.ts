import assert from "node:assert/strict";
import { test } from "node:test";
import { App } from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { StrangerThoughtsStack } from "../lib/stack.js";
import { contentSecurityPolicy } from "../lib/headers.js";

function template() {
  const app = new App();
  const stack = new StrangerThoughtsStack(app, "Test", {
    stage: "dev",
    siteHost: "preview.invalid",
    cognitoDomainPrefix: "stranger-thoughts-dev-preview",
    budgetEmail: "budget-alerts@example.com",
    enableWaf: true,
    enableGuardDuty: false,
    maxRotatesPerDay: "3",
    maxAddressReadsPerDay: "40",
    certificateArn: "arn:aws:acm:us-east-1:123456789012:certificate/00000000-0000-0000-0000-000000000000",
  });
  return Template.fromStack(stack);
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
        decryptRoles.push(JSON.stringify(statement.Resource));
        assert.equal(actions.includes("kms:GenerateDataKey"), false);
      }
      if (actions.some((action: string) => action.startsWith("dynamodb:"))) {
        for (const resource of resources) {
          assert.notEqual(resource, "*");
        }
      }
    }
  }
  assert.equal(decryptRoles.length, 1);
});

test("stripe live mode is hard-coded off and the budget is 20 dollars", () => {
  const synthesized = template();
  const functions = synthesized.findResources("AWS::Lambda::Function");
  const body = JSON.stringify(functions);
  assert.match(body, /STRIPE_ALLOW_LIVE/);
  assert.match(body, /"false"/);
  assert.equal(body.includes("sk_live_"), false);
  assert.equal(body.includes("sk_test_"), false);
  synthesized.hasResourceProperties("AWS::Budgets::Budget", {
    Budget: {
      BudgetLimit: { Amount: 20, Unit: "USD" },
      BudgetType: "COST",
      TimeUnit: "MONTHLY",
    },
  });
  synthesized.resourceCountIs("AWS::WAFv2::WebACL", 1);
  synthesized.resourceCountIs("AWS::KMS::Key", 1);
});

test("cognito client has no secret and refresh rotation is on", () => {
  const synthesized = template();
  synthesized.hasResourceProperties("AWS::Cognito::UserPoolClient", {
    GenerateSecret: false,
    AllowedOAuthFlows: ["code"],
    GenerateSecret: false,
    EnableTokenRevocation: true,
    AccessTokenValidity: 15,
    RefreshTokenRotation: { Feature: "ENABLED" },
  });
  synthesized.hasResourceProperties("AWS::Cognito::UserPool", {
    UserPoolTier: "PLUS",
  });
});
