import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import * as cdk from "aws-cdk-lib";
import * as acm from "aws-cdk-lib/aws-certificatemanager";
import * as apigwv2 from "aws-cdk-lib/aws-apigatewayv2";
import { HttpJwtAuthorizer } from "aws-cdk-lib/aws-apigatewayv2-authorizers";
import { HttpLambdaIntegration } from "aws-cdk-lib/aws-apigatewayv2-integrations";
import * as budgets from "aws-cdk-lib/aws-budgets";
import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import * as origins from "aws-cdk-lib/aws-cloudfront-origins";
import * as cloudtrail from "aws-cdk-lib/aws-cloudtrail";
import * as cognito from "aws-cdk-lib/aws-cognito";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as guardduty from "aws-cdk-lib/aws-guardduty";
import * as iam from "aws-cdk-lib/aws-iam";
import * as kms from "aws-cdk-lib/aws-kms";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as s3deploy from "aws-cdk-lib/aws-s3-deployment";
import * as secretsmanager from "aws-cdk-lib/aws-secretsmanager";
import * as wafv2 from "aws-cdk-lib/aws-wafv2";
import type { Construct } from "constructs";
import routes from "../../backend/routes.json" with { type: "json" };
import { contentSecurityPolicy } from "./headers.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));

export interface StrangerThoughtsProps extends cdk.StackProps {
  stage: "dev" | "prod";
  siteHost: string;
  cognitoDomainPrefix: string;
  budgetEmail: string;
  enableWaf: boolean;
  enableGuardDuty: boolean;
  maxRotatesPerDay: string;
  maxAddressReadsPerDay: string;
  certificateArn?: string;
}

interface Route {
  method: string;
  path: string;
}

export class StrangerThoughtsStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: StrangerThoughtsProps) {
    super(scope, id, props);
    const { stage } = props;
    const siteOrigin = `https://${props.siteHost}`;
    const removal = stage === "prod" ? cdk.RemovalPolicy.RETAIN : cdk.RemovalPolicy.DESTROY;

    const addressKey = new kms.Key(this, "AddressKey", {
      alias: `alias/stranger-thoughts-${stage}-addresses`,
      description: "Envelope encryption for reader mailing addresses",
      enableKeyRotation: true,
      removalPolicy: removal,
      pendingWindow: cdk.Duration.days(stage === "prod" ? 30 : 7),
    });

    const users = table(this, "Users", "userId", "sk", removal);
    const readers = table(this, "Readers", "readerId", "sk", removal);
    readers.addGlobalSecondaryIndex({
      indexName: "ActiveReaders",
      partitionKey: { name: "activePk", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "readerId", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.INCLUDE,
      nonKeyAttributes: ["pseudonym", "blockedWriterIds"],
    });
    const matches = table(this, "Matches", "writerId", "matchedAt", removal);
    matches.addGlobalSecondaryIndex({
      indexName: "ByMatch",
      partitionKey: { name: "matchId", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.ALL,
    });
    matches.addGlobalSecondaryIndex({
      indexName: "ByReader",
      partitionKey: { name: "readerId", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "matchedAt", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.ALL,
    });
    const tips = table(this, "Tips", "tipId", undefined, removal);
    tips.addGlobalSecondaryIndex({
      indexName: "ByReader",
      partitionKey: { name: "readerId", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "createdAt", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.INCLUDE,
      nonKeyAttributes: ["amountCents", "status", "kind"],
    });

    const stripeKey = new secretsmanager.Secret(this, "StripeSecretKey", {
      secretName: `stranger-thoughts/${stage}/stripe/secret_key`,
      description: "Replace secret_key with the Stripe test-mode key after deploy. Test mode only until security clears live mode.",
      generateSecretString: {
        secretStringTemplate: JSON.stringify({ secret_key: "replace-me" }),
        generateStringKey: "unused",
      },
    });
    const stripeWebhook = new secretsmanager.Secret(this, "StripeWebhookSecret", {
      secretName: `stranger-thoughts/${stage}/stripe/webhook_secret`,
      description: "Replace webhook_secret with the Stripe test-mode signing secret. One secret per stage.",
      generateSecretString: {
        secretStringTemplate: JSON.stringify({ webhook_secret: "replace-me" }),
        generateStringKey: "unused",
      },
    });

    const pool = new cognito.UserPool(this, "UsersPool", {
      userPoolName: `stranger-thoughts-${stage}`,
      selfSignUpEnabled: true,
      signInAliases: { email: true },
      autoVerify: { email: true },
      standardAttributes: {
        email: { required: true, mutable: false },
      },
      customAttributes: {
        role: new cognito.StringAttribute({ minLen: 6, maxLen: 6, mutable: false }),
      },
      passwordPolicy: {
        minLength: 15,
        requireLowercase: false,
        requireUppercase: false,
        requireDigits: false,
        requireSymbols: false,
        tempPasswordValidity: cdk.Duration.days(1),
      },
      mfa: cognito.Mfa.OPTIONAL,
      mfaSecondFactor: { sms: false, otp: true },
      accountRecovery: cognito.AccountRecovery.EMAIL_ONLY,
      featurePlan: cognito.FeaturePlan.PLUS,
      standardThreatProtectionMode: cognito.StandardThreatProtectionMode.FULL_FUNCTION,
      removalPolicy: removal,
    });

    const callbacks = [`${siteOrigin}/auth/callback/`];
    const logouts = [`${siteOrigin}/`];
    if (stage === "dev") {
      callbacks.push("http://localhost:8080/auth/callback/");
      logouts.push("http://localhost:8080/");
    }
    const client = pool.addClient("Spa", {
      userPoolClientName: `stranger-thoughts-${stage}-spa`,
      generateSecret: false,
      authFlows: { userSrp: true },
      oAuth: {
        flows: { authorizationCodeGrant: true },
        scopes: [cognito.OAuthScope.OPENID, cognito.OAuthScope.EMAIL],
        callbackUrls: callbacks,
        logoutUrls: logouts,
      },
      preventUserExistenceErrors: true,
      accessTokenValidity: cdk.Duration.minutes(15),
      idTokenValidity: cdk.Duration.minutes(15),
      refreshTokenValidity: cdk.Duration.days(30),
      refreshTokenRotationGracePeriod: cdk.Duration.seconds(10),
      enableTokenRevocation: true,
      writeAttributes: new cognito.ClientAttributes()
        .withStandardAttributes({ email: true })
        .withCustomAttributes("role"),
      readAttributes: new cognito.ClientAttributes().withStandardAttributes({
        email: true,
        emailVerified: true,
      }),
    });
    pool.addDomain("Hosted", {
      cognitoDomain: { domainPrefix: props.cognitoDomainPrefix },
    });
    new cognito.CfnUserPoolGroup(this, "WriterGroup", {
      userPoolId: pool.userPoolId,
      groupName: "writer",
      description: "People who send letters",
    });
    new cognito.CfnUserPoolGroup(this, "ReaderGroup", {
      userPoolId: pool.userPoolId,
      groupName: "reader",
      description: "People who receive letters",
    });

    const tables = {
      USERS_TABLE: users.tableName,
      READERS_TABLE: readers.tableName,
      MATCHES_TABLE: matches.tableName,
      TIPS_TABLE: tips.tableName,
    };
    const healthFn = rustFunction(this, "Health", "health", stage, {});
    const profileFn = rustFunction(this, "Profile", "profile", stage, {
      ...tables,
      ADDRESS_KEY_ID: addressKey.keyArn,
    });
    const matchingFn = rustFunction(this, "Matching", "matching", stage, {
      ...tables,
      MAX_ROTATES_PER_DAY: props.maxRotatesPerDay,
    });
    const addressFn = rustFunction(this, "Address", "address", stage, {
      ...tables,
      ADDRESS_KEY_ID: addressKey.keyArn,
      MAX_ADDRESS_READS_PER_DAY: props.maxAddressReadsPerDay,
    });
    const paymentsFn = rustFunction(this, "Payments", "payments", stage, {
      ...tables,
      STRIPE_SECRET_ID: stripeKey.secretArn,
      STRIPE_ALLOW_LIVE: "false",
      APP_BASE_URL: siteOrigin,
    });
    const webhookFn = rustFunction(this, "Webhook", "webhook", stage, {
      ...tables,
      STRIPE_WEBHOOK_SECRET_ID: stripeWebhook.secretArn,
    });
    const accountFn = rustFunction(this, "Account", "account", stage, {
      ...tables,
      USER_POOL_ID: pool.userPoolId,
    });
    const postConfirmFn = rustFunction(this, "PostConfirmation", "post_confirmation", stage, {});
    const preTokenFn = rustFunction(this, "PreToken", "pre_token", stage, {});
    pool.addTrigger(cognito.UserPoolOperation.POST_CONFIRMATION, postConfirmFn);
    pool.addTrigger(
      cognito.UserPoolOperation.PRE_TOKEN_GENERATION_CONFIG,
      preTokenFn,
      cognito.LambdaVersion.V2_0,
    );

    grantTable(profileFn, users, ["dynamodb:UpdateItem"]);
    grantTable(profileFn, readers, ["dynamodb:UpdateItem", "dynamodb:PutItem"]);
    addressKey.grant(profileFn, "kms:GenerateDataKey");

    grantTable(matchingFn, users, ["dynamodb:GetItem"]);
    grantTable(matchingFn, matches, ["dynamodb:Query", "dynamodb:UpdateItem"]);
    grantIndex(matchingFn, matches, "ByMatch", ["dynamodb:Query"]);
    grantIndex(matchingFn, readers, "ActiveReaders", ["dynamodb:Query"]);
    matchingFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["dynamodb:PutItem", "dynamodb:TransactWriteItems"],
      resources: [matches.tableArn],
    }));

    grantTable(addressFn, users, ["dynamodb:GetItem", "dynamodb:PutItem"]);
    grantIndex(addressFn, matches, "ByMatch", ["dynamodb:Query"]);
    grantTable(addressFn, matches, ["dynamodb:Query"]);
    addressFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["dynamodb:GetItem"],
      resources: [readers.tableArn],
      conditions: {
        "ForAllValues:StringEquals": {
          "dynamodb:Attributes": [
            "readerId",
            "sk",
            "ciphertext",
            "encryptedDataKey",
            "nonce",
            "updatedAt",
          ],
        },
        StringEquals: { "dynamodb:Select": "SPECIFIC_ATTRIBUTES" },
      },
    }));
    addressKey.grantDecrypt(addressFn);

    grantTable(paymentsFn, matches, ["dynamodb:Query"]);
    grantTable(paymentsFn, tips, ["dynamodb:PutItem", "dynamodb:UpdateItem", "dynamodb:GetItem"]);
    paymentsFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["dynamodb:GetItem"],
      resources: [readers.tableArn],
      conditions: {
        "ForAllValues:StringEquals": {
          "dynamodb:Attributes": [
            "readerId",
            "sk",
            "pseudonym",
            "stripeAccountId",
            "payoutsReady",
            "activePk",
          ],
        },
        StringEquals: { "dynamodb:Select": "SPECIFIC_ATTRIBUTES" },
      },
    }));
    paymentsFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["dynamodb:UpdateItem"],
      resources: [readers.tableArn],
      conditions: {
        "ForAllValues:StringEquals": {
          "dynamodb:Attributes": ["stripeAccountId"],
        },
      },
    }));
    stripeKey.grantRead(paymentsFn);

    grantTable(webhookFn, tips, ["dynamodb:PutItem", "dynamodb:UpdateItem", "dynamodb:GetItem"]);
    webhookFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["dynamodb:UpdateItem"],
      resources: [readers.tableArn],
      conditions: {
        "ForAllValues:StringEquals": {
          "dynamodb:Attributes": ["payoutsReady"],
        },
      },
    }));
    stripeWebhook.grantRead(webhookFn);

    grantTable(accountFn, users, ["dynamodb:UpdateItem", "dynamodb:DeleteItem"]);
    accountFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["dynamodb:GetItem"],
      resources: [readers.tableArn],
      conditions: {
        "ForAllValues:StringEquals": {
          "dynamodb:Attributes": [
            "readerId",
            "sk",
            "pseudonym",
            "stripeAccountId",
            "payoutsReady",
            "activePk",
            "blockedWriterIds",
          ],
        },
        StringEquals: { "dynamodb:Select": "SPECIFIC_ATTRIBUTES" },
      },
    }));
    accountFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["dynamodb:UpdateItem"],
      resources: [readers.tableArn],
      conditions: {
        "ForAllValues:StringEquals": {
          "dynamodb:Attributes": ["blockedWriterIds", "activePk"],
        },
      },
    }));
    grantTable(accountFn, readers, ["dynamodb:DeleteItem"]);
    grantTable(accountFn, matches, ["dynamodb:Query", "dynamodb:UpdateItem"]);
    grantIndex(accountFn, matches, "ByReader", ["dynamodb:Query"]);
    grantIndex(accountFn, tips, "ByReader", ["dynamodb:Query"]);
    accountFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["cognito-idp:AdminDeleteUser"],
      resources: [pool.userPoolArn],
    }));

    // A specific pool ARN here cycles: the pool's trigger depends on this
    // function, and the function's policy would depend on the pool. The
    // action is only AdminAddUserToGroup. This stack creates one pool.
    postConfirmFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["cognito-idp:AdminAddUserToGroup"],
      resources: [`arn:aws:cognito-idp:${this.region}:${this.account}:userpool/*`],
    }));

    const httpApi = new apigwv2.HttpApi(this, "HttpApi", {
      apiName: `stranger-thoughts-${stage}`,
      corsPreflight: {
        allowOrigins: [siteOrigin],
        allowMethods: [
          apigwv2.CorsHttpMethod.GET,
          apigwv2.CorsHttpMethod.POST,
          apigwv2.CorsHttpMethod.PUT,
          apigwv2.CorsHttpMethod.DELETE,
          apigwv2.CorsHttpMethod.OPTIONS,
        ],
        allowHeaders: ["authorization", "content-type", "stripe-signature"],
        maxAge: cdk.Duration.hours(1),
      },
    });
    const authorizer = new HttpJwtAuthorizer(
      "Jwt",
      `https://cognito-idp.${this.region}.amazonaws.com/${pool.userPoolId}`,
      { jwtAudience: [client.userPoolClientId] },
    );
    const integrations = new Map<string, HttpLambdaIntegration>([
      ["health", new HttpLambdaIntegration("HealthInt", healthFn)],
      ["profile", new HttpLambdaIntegration("ProfileInt", profileFn)],
      ["matching", new HttpLambdaIntegration("MatchingInt", matchingFn)],
      ["address", new HttpLambdaIntegration("AddressInt", addressFn)],
      ["payments", new HttpLambdaIntegration("PaymentsInt", paymentsFn)],
      ["webhook", new HttpLambdaIntegration("WebhookInt", webhookFn)],
      ["account", new HttpLambdaIntegration("AccountInt", accountFn)],
    ]);
    const ownerOf = (route: Route): string => {
      if (route.path === "/health") return "health";
      if (route.path === "/webhooks/stripe") return "webhook";
      if (route.path.startsWith("/writers/profile") || route.path.includes("demographics") || route.path === "/readers/profile") {
        return "profile";
      }
      if (route.path.startsWith("/writers/") || route.path.endsWith("/sent")) return "matching";
      if (route.path.endsWith("/address")) return "address";
      if (route.path.startsWith("/tips/") || route.path.endsWith("/link")) return "payments";
      return "account";
    };
    const allRoutes = [...(routes.public as Route[]), ...(routes.authenticated as Route[])];
    const publicKeys = new Set((routes.public as Route[]).map((route) => `${route.method} ${route.path}`));
    for (const route of allRoutes) {
      const method = apigwv2.HttpMethod[route.method as keyof typeof apigwv2.HttpMethod];
      httpApi.addRoutes({
        path: route.path,
        methods: [method],
        integration: integrations.get(ownerOf(route))!,
        authorizer: publicKeys.has(`${route.method} ${route.path}`) ? undefined : authorizer,
      });
    }
    const cfnStage = httpApi.defaultStage?.node.defaultChild as apigwv2.CfnStage;
    cfnStage.defaultRouteSettings = { throttlingBurstLimit: 20, throttlingRateLimit: 10 };
    cfnStage.routeSettings = {
      "POST /writers/rotate": { throttlingBurstLimit: 2, throttlingRateLimit: 1 },
      "GET /matches/{matchId}/address": { throttlingBurstLimit: 5, throttlingRateLimit: 2 },
      "POST /webhooks/stripe": { throttlingBurstLimit: 20, throttlingRateLimit: 5 },
    };

    const siteBucket = new s3.Bucket(this, "Site", {
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      encryption: s3.BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      objectOwnership: s3.ObjectOwnership.BUCKET_OWNER_ENFORCED,
      versioned: true,
      removalPolicy: removal,
      autoDeleteObjects: stage === "dev",
    });
    const csp = contentSecurityPolicy({
      cognitoDomain: props.cognitoDomainPrefix,
      region: this.region,
    });
    const headers = new cloudfront.ResponseHeadersPolicy(this, "SecurityHeaders", {
      securityHeadersBehavior: {
        strictTransportSecurity: {
          accessControlMaxAge: cdk.Duration.days(730),
          includeSubdomains: true,
          preload: true,
          override: true,
        },
        contentTypeOptions: { override: true },
        frameOptions: { frameOption: cloudfront.HeadersFrameOption.DENY, override: true },
        referrerPolicy: {
          referrerPolicy: cloudfront.HeadersReferrerPolicy.STRICT_ORIGIN_WHEN_CROSS_ORIGIN,
          override: true,
        },
        contentSecurityPolicy: { contentSecurityPolicy: csp, override: true },
      },
    });
    const spaRewrite = new cloudfront.Function(this, "SpaRewrite", {
      code: cloudfront.FunctionCode.fromInline(`function handler(event) {
  var uri = event.request.uri;
  if (uri.endsWith("/")) event.request.uri += "index.html";
  else if (uri.indexOf(".") === -1) event.request.uri += "/index.html";
  return event.request;
}`),
    });
    const apiRewrite = new cloudfront.Function(this, "ApiRewrite", {
      code: cloudfront.FunctionCode.fromInline(`function handler(event) {
  var uri = event.request.uri;
  uri = uri.replace(/^\\/api/, "");
  if (uri === "") uri = "/";
  event.request.uri = uri;
  return event.request;
}`),
    });
    let webAclArn: string | undefined;
    if (props.enableWaf) {
      const acl = new wafv2.CfnWebACL(this, "WebAcl", {
        name: `stranger-thoughts-${stage}`,
        scope: "CLOUDFRONT",
        defaultAction: { allow: {} },
        visibilityConfig: visibility(`st-${stage}`),
        rules: [
          {
            name: "RateLimit",
            priority: 1,
            action: { block: {} },
            statement: { rateBasedStatement: { limit: 1000, aggregateKeyType: "IP" } },
            visibilityConfig: visibility(`st-${stage}-rate`),
          },
          managedRule("Common", 2, "AWSManagedRulesCommonRuleSet", `st-${stage}-common`),
          managedRule("BadInputs", 3, "AWSManagedRulesKnownBadInputsRuleSet", `st-${stage}-bad`),
        ],
      });
      webAclArn = acl.attrArn;
    }
    if (stage === "prod" && !props.certificateArn) {
      throw new Error("prod requires -c certificateArn= so CloudFront can require TLS 1.2");
    }
    const distribution = new cloudfront.Distribution(this, "Cdn", {
      defaultRootObject: "index.html",
      ...(props.certificateArn
        ? {
            certificate: acm.Certificate.fromCertificateArn(this, "ViewerCert", props.certificateArn),
            domainNames: [props.siteHost],
            minimumProtocolVersion: cloudfront.SecurityPolicyProtocol.TLS_V1_2_2021,
          }
        : {}),
      priceClass: cloudfront.PriceClass.PRICE_CLASS_100,
      webAclId: webAclArn,
      defaultBehavior: {
        origin: origins.S3BucketOrigin.withOriginAccessControl(siteBucket),
        viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        responseHeadersPolicy: headers,
        functionAssociations: [{
          function: spaRewrite,
          eventType: cloudfront.FunctionEventType.VIEWER_REQUEST,
        }],
      },
      additionalBehaviors: {
        "/api/*": {
          origin: new origins.HttpOrigin(cdk.Fn.select(2, cdk.Fn.split("/", httpApi.apiEndpoint)), {
            protocolPolicy: cloudfront.OriginProtocolPolicy.HTTPS_ONLY,
          }),
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.HTTPS_ONLY,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          responseHeadersPolicy: headers,
          functionAssociations: [{
            function: apiRewrite,
            eventType: cloudfront.FunctionEventType.VIEWER_REQUEST,
          }],
        },
      },
    });

    const frontendDir = path.resolve(HERE, "../../frontend");
    const configText = fs.readFileSync(path.join(frontendDir, "js/config.js"), "utf8");
    if (this.node.tryGetContext("productionConfig") === "true" && configText.includes("mock: true")) {
      throw new Error("frontend/js/config.js is still in mock mode");
    }
    new s3deploy.BucketDeployment(this, "SiteFiles", {
      sources: [s3deploy.Source.asset(frontendDir)],
      destinationBucket: siteBucket,
      distribution,
      distributionPaths: ["/*"],
      prune: true,
    });

    new cloudtrail.Trail(this, "Trail", {
      trailName: `stranger-thoughts-${stage}`,
      managementEvents: cloudtrail.ReadWriteType.ALL,
      includeGlobalServiceEvents: true,
    });
    if (props.enableGuardDuty) {
      new guardduty.CfnDetector(this, "GuardDuty", { enable: true });
    }

    new budgets.CfnBudget(this, "MonthlyBudget", {
      budget: {
        budgetName: `stranger-thoughts-${stage}`,
        budgetType: "COST",
        timeUnit: "MONTHLY",
        budgetLimit: { amount: 20, unit: "USD" },
      },
      notificationsWithSubscribers: [80, 100].map((threshold) => ({
        notification: {
          comparisonOperator: "GREATER_THAN",
          notificationType: "ACTUAL",
          threshold,
          thresholdType: "PERCENTAGE",
        },
        subscribers: [{ address: props.budgetEmail, subscriptionType: "EMAIL" }],
      })),
    });

    new cdk.CfnOutput(this, "SiteUrl", { value: `https://${distribution.distributionDomainName}` });
    new cdk.CfnOutput(this, "ApiUrl", { value: httpApi.apiEndpoint });
    new cdk.CfnOutput(this, "UserPoolId", { value: pool.userPoolId });
    new cdk.CfnOutput(this, "UserPoolClientId", { value: client.userPoolClientId });
    new cdk.CfnOutput(this, "CognitoDomain", {
      value: `${props.cognitoDomainPrefix}.auth.${this.region}.amazoncognito.com`,
    });
    new cdk.CfnOutput(this, "StripeSecretName", { value: stripeKey.secretName });
    new cdk.CfnOutput(this, "StripeWebhookSecretName", { value: stripeWebhook.secretName });
  }
}

function table(
  stack: cdk.Stack,
  id: string,
  partitionKey: string,
  sortKey: string | undefined,
  removal: cdk.RemovalPolicy,
) {
  return new dynamodb.Table(stack, id, {
    partitionKey: { name: partitionKey, type: dynamodb.AttributeType.STRING },
    sortKey: sortKey ? { name: sortKey, type: dynamodb.AttributeType.STRING } : undefined,
    billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
    encryption: dynamodb.TableEncryption.AWS_MANAGED,
    pointInTimeRecoverySpecification: { pointInTimeRecoveryEnabled: true },
    removalPolicy: removal,
  });
}

function rustFunction(
  stack: cdk.Stack,
  id: string,
  bin: string,
  stage: string,
  environment: Record<string, string>,
) {
  const logGroup = new logs.LogGroup(stack, `${id}Logs`, {
    retention: logs.RetentionDays.ONE_MONTH,
    removalPolicy: cdk.RemovalPolicy.DESTROY,
  });
  const role = new iam.Role(stack, `${id}Role`, {
    assumedBy: new iam.ServicePrincipal("lambda.amazonaws.com"),
  });
  role.addToPolicy(new iam.PolicyStatement({
    actions: ["logs:CreateLogStream", "logs:PutLogEvents"],
    resources: [`${logGroup.logGroupArn}:*`],
  }));
  const assetRoot = process.env.LAMBDA_ASSET_ROOT;
  const directory = assetRoot
    ? path.join(assetRoot, bin)
    : path.resolve(HERE, "../lambda-stubs", bin);
  if (process.argv.includes("deploy")) {
    const bootstrap = fs.readFileSync(path.join(directory, "bootstrap"));
    const elf = bootstrap[0] === 0x7f && bootstrap.subarray(1, 4).toString() === "ELF";
    if (!elf) {
      throw new Error(`Refusing to deploy ${bin}: bootstrap is not an ELF binary. Build with cargo-lambda first.`);
    }
  }
  return new lambda.Function(stack, id, {
    functionName: `st-${stage}-${bin}`.slice(0, 64),
    runtime: lambda.Runtime.PROVIDED_AL2023,
    architecture: lambda.Architecture.ARM_64,
    handler: "bootstrap",
    code: lambda.Code.fromAsset(directory),
    role,
    logGroup,
    timeout: cdk.Duration.seconds(10),
    memorySize: 256,
    environment: { STAGE: stage, ...environment },
    tracing: lambda.Tracing.DISABLED,
  });
}

function grantTable(fn: lambda.Function, table: dynamodb.Table, actions: string[]) {
  fn.addToRolePolicy(new iam.PolicyStatement({ actions, resources: [table.tableArn] }));
}

function grantIndex(fn: lambda.Function, table: dynamodb.Table, index: string, actions: string[]) {
  fn.addToRolePolicy(new iam.PolicyStatement({
    actions,
    resources: [`${table.tableArn}/index/${index}`],
  }));
}

function visibility(name: string): wafv2.CfnWebACL.VisibilityConfigProperty {
  return { cloudWatchMetricsEnabled: true, metricName: name, sampledRequestsEnabled: true };
}

function managedRule(name: string, priority: number, group: string, metric: string) {
  return {
    name,
    priority,
    overrideAction: { none: {} },
    statement: { managedRuleGroupStatement: { vendorName: "AWS", name: group } },
    visibilityConfig: visibility(metric),
  };
}
