import assert from "node:assert/strict";
import { test } from "node:test";
import { App, Aspects } from "aws-cdk-lib";
import { Annotations, Match, Template } from "aws-cdk-lib/assertions";
import { AwsSolutionsChecks } from "cdk-nag";
import { StrangerThoughtsStack } from "../lib/stack.js";
import { contentSecurityPolicy, PERMISSIONS_POLICY } from "../lib/headers.js";

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
  assert.match(headers, /Permissions-Policy/);
  assert.ok(headers.includes(PERMISSIONS_POLICY));
  const robots = JSON.stringify(synthesized.findResources("AWS::CloudFront::Function"));
  assert.ok(robots.includes("uri === roots[i]"));
  assert.ok(robots.includes("noindex, noarchive"));
  assert.match(distribution, /ApiDomain/);
  const domains = JSON.stringify(synthesized.findResources("AWS::ApiGatewayV2::DomainName"));
  assert.match(domains, /api\.preview\.invalid/);
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
    "st-dev-lambda-errors-api",
    "st-dev-lambda-errors-workers",
    "st-dev-lambda-throttles-api",
    "st-dev-lambda-throttles-workers",
    "st-dev-lambda-concurrency-api",
    "st-dev-lambda-concurrency-workers",
    "st-dev-dynamodb-read-throttles",
    "st-dev-dynamodb-write-throttles",
  ]) {
    assert.ok(names.includes(name), name);
  }
  assert.equal(Object.keys(alarms).length, 10);
  const alarmBody = JSON.stringify(alarms);
  assert.equal(alarmBody.includes("SEARCH("), false);
  assert.equal(alarmBody.includes("SELECT "), false);
  assert.equal(alarmBody.includes(" OR "), false);
  const resourceName = (value: unknown) => {
    if (typeof value === "string") {
      return value;
    }
    const ref = value && typeof value === "object" && "Ref" in value ? String((value as { Ref: string }).Ref) : "";
    const functionName = functions[ref]?.Properties?.FunctionName;
    const tableName = tables[ref]?.Properties?.TableName;
    return typeof functionName === "string" ? functionName : typeof tableName === "string" ? tableName : "";
  };
  const metricStats = (alarm: { Properties?: { Metrics?: Array<{ MetricStat?: { Metric?: { MetricName?: string, Dimensions?: Array<{ Name?: string, Value?: unknown }> }, Stat?: string, Period?: number } }>, MetricName?: string, Namespace?: string, Dimensions?: Array<{ Name?: string, Value?: unknown }>, Statistic?: string, Period?: number, AlarmName?: string, Threshold?: number } }) => {
    const metrics = alarm.Properties?.Metrics;
    if (Array.isArray(metrics)) {
      return metrics.flatMap((item) => item.MetricStat ? [item.MetricStat] : []);
    }
    if (alarm.Properties?.MetricName) {
      return [{
        Metric: {
          MetricName: alarm.Properties.MetricName,
          Dimensions: alarm.Properties.Dimensions,
        },
        Stat: alarm.Properties.Statistic,
        Period: alarm.Properties.Period,
      }];
    }
    return [];
  };
  const covered = (metricName: string) => {
    const values: string[] = [];
    for (const alarm of Object.values(alarms)) {
      assert.match(String(alarm.Properties?.AlarmName), /^st-/);
      const stats = metricStats(alarm);
      assert.ok(stats.length >= 1 && stats.length <= 10, alarm.Properties?.AlarmName);
      for (const stat of stats) {
        assert.equal(stat.Period, 300);
        if (stat.Metric?.MetricName !== metricName) {
          continue;
        }
        if (metricName === "ConcurrentExecutions") {
          assert.equal(stat.Stat, "Maximum");
        } else {
          assert.equal(stat.Stat, "Sum");
        }
        for (const dimension of stat.Metric?.Dimensions ?? []) {
          const resolved = resourceName(dimension.Value);
          if (resolved) {
            values.push(resolved);
          }
        }
      }
    }
    return values;
  };
  const functionNames = Object.values(functions)
    .map((fn) => fn.Properties?.FunctionName)
    .filter((name): name is string => typeof name === "string" && name.startsWith("st-dev-"));
  assert.equal(functionNames.length, 11);
  for (const name of functionNames) {
    for (const metric of ["Errors", "Throttles", "ConcurrentExecutions"]) {
      assert.ok(covered(metric).includes(name), `${metric} ${name}`);
    }
  }
  const tableNames = Object.values(tables)
    .map((item) => item.Properties?.TableName)
    .filter((name): name is string => typeof name === "string" && name.startsWith("st-dev-"));
  assert.equal(tableNames.length, 7);
  for (const name of tableNames) {
    assert.ok(covered("ReadThrottleEvents").includes(name), name);
    assert.ok(covered("WriteThrottleEvents").includes(name), name);
  }
  const byName = Object.fromEntries(Object.values(alarms).map((alarm) => [alarm.Properties?.AlarmName, alarm.Properties?.Threshold]));
  assert.equal(byName["st-dev-lambda-concurrency-api"], 13);
  assert.equal(byName["st-dev-lambda-concurrency-workers"], 3);
  const outputs = JSON.stringify(synthesized.toJSON().Outputs ?? {});
  assert.match(outputs, /ApiCustomDomainTarget/);
  assert.match(outputs, /RegionalDomainName/);
  const apis = synthesized.findResources("AWS::ApiGatewayV2::Api");
  for (const api of Object.values(apis)) {
    assert.equal(api.Properties?.DisableExecuteApiEndpoint, true);
  }
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

test("data grants name the keys and the routes that read them", () => {
  const synthesized = template();
  const functions = synthesized.findResources("AWS::Lambda::Function");
  const policies = synthesized.findResources("AWS::IAM::Policy");
  const statementsFor = (functionName: string) => {
    const fn = Object.values(functions).find((item) => item.Properties?.FunctionName === functionName);
    assert.ok(fn, functionName);
    const roleId = fn.Properties.Role["Fn::GetAtt"][0];
    return Object.values(policies).flatMap((policy) => {
      const roles = [].concat(policy.Properties?.Roles ?? []);
      const attached = roles.some((role) => JSON.stringify(role).includes(roleId));
      if (!attached) {
        return [];
      }
      const statements = policy.Properties?.PolicyDocument?.Statement ?? [];
      return Array.isArray(statements) ? statements : [statements];
    });
  };
  const resourcesOf = (statement) => [].concat(statement.Resource ?? []).map((item) => JSON.stringify(item));
  const actionsOf = (statement) => [].concat(statement.Action ?? []);
  const matching = statementsFor("st-dev-matching");
  assert.ok(matching.some((statement) =>
    actionsOf(statement).includes("dynamodb:Query")
    && resourcesOf(statement).some((resource) => resource.includes("index/ByReader"))
  ));
  const account = statementsFor("st-dev-account");
  assert.ok(account.some((statement) =>
    actionsOf(statement).includes("dynamodb:Query")
    && resourcesOf(statement).some((resource) => resource.includes("Correspondence"))
    && resourcesOf(statement).every((resource) => !resource.includes("index/"))
  ));
  const preservation = statementsFor("st-dev-preservation");
  assert.ok(preservation.some((statement) => {
    const actions = actionsOf(statement);
    return ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:Query", "dynamodb:DeleteItem"]
      .every((action) => actions.includes(action));
  }));
  const attributeLists = Object.values(policies).flatMap((policy) => {
    const statements = policy.Properties?.PolicyDocument?.Statement ?? [];
    return (Array.isArray(statements) ? statements : [statements])
      .map((statement) => statement.Condition?.["ForAllValues:StringEquals"]?.["dynamodb:Attributes"])
      .filter(Boolean);
  });
  assert.ok(attributeLists.length >= 4);
  for (const attributes of attributeLists) {
    assert.ok(attributes.includes("sk"), JSON.stringify(attributes));
    assert.ok(attributes.includes("userId") || attributes.includes("readerId"), JSON.stringify(attributes));
  }
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
