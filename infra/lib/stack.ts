import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import * as cdk from "aws-cdk-lib";
import * as acm from "aws-cdk-lib/aws-certificatemanager";
import * as apigwv2 from "aws-cdk-lib/aws-apigatewayv2";
import { HttpJwtAuthorizer } from "aws-cdk-lib/aws-apigatewayv2-authorizers";
import { HttpLambdaIntegration } from "aws-cdk-lib/aws-apigatewayv2-integrations";
import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import * as origins from "aws-cdk-lib/aws-cloudfront-origins";
import * as cloudwatch from "aws-cdk-lib/aws-cloudwatch";
import * as cw_actions from "aws-cdk-lib/aws-cloudwatch-actions";
import * as cognito from "aws-cdk-lib/aws-cognito";
import * as dynamodb from "aws-cdk-lib/aws-dynamodb";
import * as iam from "aws-cdk-lib/aws-iam";
import * as kms from "aws-cdk-lib/aws-kms";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as s3deploy from "aws-cdk-lib/aws-s3-deployment";
import * as sns from "aws-cdk-lib/aws-sns";
import * as wafv2 from "aws-cdk-lib/aws-wafv2";
import type { IConstruct } from "constructs";
import type { Construct } from "constructs";
import { NagSuppressions } from "cdk-nag";
import routes from "../../backend/routes.json" with { type: "json" };
import { contentSecurityPolicy } from "./headers.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));

export interface StrangerThoughtsProps extends cdk.StackProps {
  stage: "dev" | "prod";
  siteHost: string;
  cognitoDomainPrefix: string;
  enableWaf: boolean;
  enableBotControl: boolean;
  enableSesIdentity: boolean;
  sesEnabled: boolean;
  alertsTopicArn: string;
  sesIdentityArn: string;
  sesFromAddress: string;
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
    super(scope, id, {
      ...props,
      stackName: props.stackName ?? `st-${props.stage}`,
      permissionsBoundary: props.permissionsBoundary ?? cdk.PermissionsBoundary.fromName("st-boundary"),
    });
    if (props.enableSesIdentity) {
      throw new Error(
        "enableSesIdentity stays off. st-cfn-exec cannot create SES identities. Verify astrangerreads.com outside this stack.",
      );
    }
    cdk.Aspects.of(this).add(new ReservedConcurrency());
    const { stage } = props;
    const siteOrigin = `https://${props.siteHost}`;
    const removal = stage === "prod" ? cdk.RemovalPolicy.RETAIN : cdk.RemovalPolicy.DESTROY;
    const stripeApiParameter = "/st/stripe/api-key";
    const stripeWebhookParameter = "/st/stripe/webhook-secret";
    const stripeApiArn = `arn:aws:ssm:${this.region}:${this.account}:parameter/st/stripe/api-key`;
    const stripeWebhookArn = `arn:aws:ssm:${this.region}:${this.account}:parameter/st/stripe/webhook-secret`;

    const addressKey = new kms.Key(this, "AddressKey", {
      alias: `alias/st-${stage}`,
      description: "App key for addresses, DynamoDB, and Stripe SSM parameters",
      enableKeyRotation: true,
      removalPolicy: removal,
      pendingWindow: cdk.Duration.days(stage === "prod" ? 30 : 7),
    });

    const users = table(this, "Users", "userId", "sk", removal, addressKey);
    const readers = table(this, "Readers", "readerId", "sk", removal, addressKey);
    readers.addGlobalSecondaryIndex({
      indexName: "ActiveReaders",
      partitionKey: { name: "activePk", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "readerId", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.INCLUDE,
      nonKeyAttributes: ["pseudonym", "blockedWriterIds"],
    });
    const matches = table(this, "Matches", "writerId", "matchedAt", removal, addressKey);
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
    const tips = table(this, "Tips", "tipId", undefined, removal, addressKey);
    const correspondence = table(this, "Correspondence", "matchId", "sk", removal, addressKey);
    const preservation = table(this, "Preservation", "ownerId", "sk", removal, addressKey);
    const rates = table(this, "RateLimits", "subject", "sk", removal, addressKey);
    (rates.node.defaultChild as dynamodb.CfnTable).timeToLiveSpecification = {
      attributeName: "ttl",
      enabled: true,
    };
    tips.addGlobalSecondaryIndex({
      indexName: "ByReader",
      partitionKey: { name: "readerId", type: dynamodb.AttributeType.STRING },
      sortKey: { name: "createdAt", type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.INCLUDE,
      nonKeyAttributes: ["amountCents", "status", "kind"],
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
      CORRESPONDENCE_TABLE: correspondence.tableName,
      PRESERVATION_TABLE: preservation.tableName,
      RATE_TABLE: rates.tableName,
    };
    const healthFn = rustFunction(this, "Health", "health", stage, tables, 2);
    const profileFn = rustFunction(this, "Profile", "profile", stage, {
      ...tables,
      ADDRESS_KEY_ID: addressKey.keyArn,
    }, 2);
    const matchingFn = rustFunction(this, "Matching", "matching", stage, {
      ...tables,
      MAX_ROTATES_PER_DAY: props.maxRotatesPerDay,
    }, 2);
    const addressFn = rustFunction(this, "Address", "address", stage, {
      ...tables,
      ADDRESS_KEY_ID: addressKey.keyArn,
      MAX_ADDRESS_READS_PER_DAY: props.maxAddressReadsPerDay,
      MAILING_DESK_READY: "false",
    }, 2);
    const paymentsFn = rustFunction(this, "Payments", "payments", stage, {
      ...tables,
      STRIPE_API_KEY_PARAMETER: stripeApiParameter,
      STRIPE_ALLOW_LIVE: "false",
      APP_BASE_URL: siteOrigin,
    }, 2);
    const webhookFn = rustFunction(this, "Webhook", "webhook", stage, {
      ...tables,
      STRIPE_WEBHOOK_SECRET_PARAMETER: stripeWebhookParameter,
      STRIPE_ALLOW_LIVE: "false",
    }, 2);
    const accountFn = rustFunction(this, "Account", "account", stage, {
      ...tables,
      USER_POOL_ID: pool.userPoolId,
    }, 2);
    const preservationFn = rustFunction(this, "PreservationApi", "preservation", stage, tables, 2);
    const notifyFn = rustFunction(this, "Notify", "notify", stage, {
      ...tables,
      USER_POOL_ID: pool.userPoolId,
      SES_ENABLED: props.sesEnabled ? "true" : "false",
      SES_FROM_ADDRESS: props.sesFromAddress,
      SITE_ORIGIN: siteOrigin,
    }, 1);
    matchingFn.addEnvironment("NOTIFY_FUNCTION_NAME", notifyFn.functionName);
    const postConfirmFn = rustFunction(this, "PostConfirmation", "post_confirmation", stage, {}, 1);
    const preTokenFn = rustFunction(this, "PreToken", "pre_token", stage, {}, 1);
    pool.addTrigger(cognito.UserPoolOperation.POST_CONFIRMATION, postConfirmFn);
    pool.addTrigger(
      cognito.UserPoolOperation.PRE_TOKEN_GENERATION_CONFIG,
      preTokenFn,
      cognito.LambdaVersion.V2_0,
    );

    grantTable(profileFn, users, ["dynamodb:UpdateItem"]);
    grantTable(profileFn, readers, ["dynamodb:UpdateItem", "dynamodb:PutItem"]);
    addressKey.grant(profileFn, "kms:GenerateDataKey");

    grantTable(healthFn, rates, ["dynamodb:GetItem", "dynamodb:PutItem"]);

    grantTable(matchingFn, users, ["dynamodb:GetItem"]);
    grantTable(matchingFn, matches, ["dynamodb:Query", "dynamodb:UpdateItem"]);
    grantTable(matchingFn, correspondence, ["dynamodb:Query", "dynamodb:PutItem", "dynamodb:UpdateItem"]);
    grantTable(matchingFn, rates, ["dynamodb:GetItem", "dynamodb:PutItem"]);
    matchingFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["lambda:InvokeFunction"],
      resources: [notifyFn.functionArn],
    }));
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
    grantParameter(paymentsFn, stripeApiArn, addressKey);

    grantTable(webhookFn, rates, ["dynamodb:GetItem", "dynamodb:PutItem"]);
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
    grantParameter(webhookFn, stripeWebhookArn, addressKey);

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
            "mailAlerts",
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
          "dynamodb:Attributes": ["blockedWriterIds", "activePk", "mailAlerts"],
        },
      },
    }));
    grantTable(accountFn, readers, ["dynamodb:DeleteItem"]);
    grantTable(accountFn, matches, ["dynamodb:Query", "dynamodb:UpdateItem"]);
    grantIndex(accountFn, matches, "ByReader", ["dynamodb:Query"]);
    grantIndex(accountFn, tips, "ByReader", ["dynamodb:Query"]);
    accountFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["dynamodb:GetItem"],
      resources: [users.tableArn],
      conditions: {
        "ForAllValues:StringEquals": {
          "dynamodb:Attributes": ["userId", "sk", "mailAlerts"],
        },
        StringEquals: { "dynamodb:Select": "SPECIFIC_ATTRIBUTES" },
      },
    }));
    grantTable(accountFn, preservation, ["dynamodb:Query", "dynamodb:DeleteItem"]);
    accountFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["cognito-idp:AdminDeleteUser"],
      resources: [pool.userPoolArn],
    }));

    grantTable(preservationFn, preservation, ["dynamodb:PutItem", "dynamodb:Query", "dynamodb:DeleteItem"]);
    grantTable(preservationFn, rates, ["dynamodb:GetItem", "dynamodb:PutItem"]);
    grantTable(preservationFn, matches, ["dynamodb:Query"]);
    grantIndex(preservationFn, matches, "ByReader", ["dynamodb:Query"]);

    grantTable(notifyFn, correspondence, ["dynamodb:UpdateItem"]);
    for (const [profileTable, idName] of [
      [users, "userId"],
      [readers, "readerId"],
    ] as const) {
      notifyFn.addToRolePolicy(new iam.PolicyStatement({
        actions: ["dynamodb:GetItem"],
        resources: [profileTable.tableArn],
        conditions: {
          "ForAllValues:StringEquals": {
            "dynamodb:Attributes": [idName, "sk", "mailAlerts"],
          },
          StringEquals: { "dynamodb:Select": "SPECIFIC_ATTRIBUTES" },
        },
      }));
    }
    notifyFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["ses:SendEmail"],
      resources: [props.sesIdentityArn],
      conditions: { StringEquals: { "ses:FromAddress": props.sesFromAddress } },
    }));
    notifyFn.addToRolePolicy(new iam.PolicyStatement({
      actions: ["cognito-idp:ListUsers"],
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
      ["preservation", new HttpLambdaIntegration("PreservationInt", preservationFn)],
    ]);
    const ownerOf = (route: Route): string => {
      if (route.path === "/health") return "health";
      if (route.path === "/webhooks/stripe") return "webhook";
      if (route.path.startsWith("/preservation")) return "preservation";
      if (route.path.startsWith("/writers/profile") || route.path.includes("demographics") || route.path === "/readers/profile") {
        return "profile";
      }
      if (
        route.path.startsWith("/writers/")
        || route.path.endsWith("/sent")
        || route.path.endsWith("/mailed")
        || route.path.endsWith("/received")
      ) {
        return "matching";
      }
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
    const apiAccess = new logs.LogGroup(this, "ApiAccessLogs", {
      retention: logs.RetentionDays.ONE_MONTH,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });
    apiAccess.addToResourcePolicy(new iam.PolicyStatement({
      principals: [new iam.ServicePrincipal("delivery.logs.amazonaws.com")],
      actions: ["logs:CreateLogStream", "logs:PutLogEvents"],
      resources: [`${apiAccess.logGroupArn}:*`],
      conditions: {
        ArnLike: { "aws:SourceArn": `arn:aws:logs:${this.region}:${this.account}:*` },
        StringEquals: { "aws:SourceAccount": this.account },
      },
    }));
    const cfnStage = httpApi.defaultStage?.node.defaultChild as apigwv2.CfnStage;
    cfnStage.accessLogSettings = {
      destinationArn: apiAccess.logGroupArn,
      format: JSON.stringify({
        requestId: "$context.requestId",
        routeKey: "$context.routeKey",
        status: "$context.status",
      }),
    };
    const tight = { throttlingBurstLimit: 2, throttlingRateLimit: 1 };
    cfnStage.defaultRouteSettings = { throttlingBurstLimit: 4, throttlingRateLimit: 2 };
    cfnStage.routeSettings = {
      "POST /writers/rotate": tight,
      "GET /matches/{matchId}/address": tight,
      "POST /matches/{matchId}/sent": tight,
      "POST /matches/{matchId}/mailed": tight,
      "POST /matches/{matchId}/received": tight,
      "POST /preservation/private": tight,
      "POST /preservation/archive": tight,
      "PUT /account/settings": tight,
      "GET /health": tight,
      "POST /webhooks/stripe": { throttlingBurstLimit: 10, throttlingRateLimit: 5 },
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
    const robotsHeader = new cloudfront.Function(this, "RobotsHeader", {
      code: cloudfront.FunctionCode.fromInline(`function handler(event) {
  var response = event.response;
  var uri = event.request.uri;
  if (uri.indexOf("/write/") === 0 || uri.indexOf("/read/") === 0 || uri.indexOf("/auth/") === 0 || uri.indexOf("/archive/") === 0) {
    response.headers["x-robots-tag"] = { value: "noindex, noarchive" };
  }
  return response;
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
          ...(props.enableBotControl
            ? [managedRule("Bots", 4, "AWSManagedRulesBotControlRuleSet", `st-${stage}-bots`)]
            : []),
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
        functionAssociations: [
          {
            function: spaRewrite,
            eventType: cloudfront.FunctionEventType.VIEWER_REQUEST,
          },
          {
            function: robotsHeader,
            eventType: cloudfront.FunctionEventType.VIEWER_RESPONSE,
          },
        ],
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

    const alerts = sns.Topic.fromTopicArn(this, "Alerts", props.alertsTopicArn);
    const action = new cw_actions.SnsAction(alerts);
    const reserved = 8 * 2 + 3;
    const searchSum = (namespace: string, dimension: string, metricName: string) => new cloudwatch.MathExpression({
      expression: `SUM(SEARCH('{${namespace},${dimension}} MetricName="${metricName}"', 'Sum', 300))`,
      usingMetrics: {},
      period: cdk.Duration.minutes(5),
    });
    const alarm = (id: string, name: string, metric: cloudwatch.IMetric, threshold: number) => {
      const item = new cloudwatch.Alarm(this, id, {
        alarmName: name,
        metric,
        threshold,
        evaluationPeriods: 1,
        datapointsToAlarm: 1,
        comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
        treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
      });
      item.addAlarmAction(action);
    };
    const apiMetric = (metricName: string) => new cloudwatch.Metric({
      namespace: "AWS/ApiGateway",
      metricName,
      dimensionsMap: { ApiId: httpApi.apiId, Stage: "$default" },
      statistic: "Sum",
      period: cdk.Duration.minutes(5),
    });
    alarm("Api5xx", `st-${stage}-api-5xx`, apiMetric("5xx"), 1);
    alarm("Api4xx", `st-${stage}-api-4xx`, apiMetric("4xx"), 50);
    alarm("LambdaErrors", `st-${stage}-lambda-errors`, searchSum("AWS/Lambda", "FunctionName", "Errors"), 1);
    alarm("LambdaThrottles", `st-${stage}-lambda-throttles`, searchSum("AWS/Lambda", "FunctionName", "Throttles"), 1);
    alarm(
      "LambdaConcurrency",
      `st-${stage}-lambda-concurrency`,
      new cloudwatch.MathExpression({
        expression: "SUM(SEARCH('{AWS/Lambda,FunctionName} MetricName=\"ConcurrentExecutions\"', 'Maximum', 300))",
        usingMetrics: {},
        period: cdk.Duration.minutes(5),
      }),
      Math.ceil(reserved * 0.8),
    );
    alarm(
      "DynamoThrottles",
      `st-${stage}-dynamodb-throttles`,
      new cloudwatch.MathExpression({
        expression: [
          "SUM(SEARCH('{AWS/DynamoDB,TableName} MetricName=\"ReadThrottleEvents\"', 'Sum', 300))",
          "SUM(SEARCH('{AWS/DynamoDB,TableName} MetricName=\"WriteThrottleEvents\"', 'Sum', 300))",
        ].join("+"),
        usingMetrics: {},
        period: cdk.Duration.minutes(5),
      }),
      1,
    );

    new cdk.CfnOutput(this, "SiteUrl", { value: `https://${distribution.distributionDomainName}` });
    new cdk.CfnOutput(this, "ApiUrl", { value: httpApi.apiEndpoint });
    new cdk.CfnOutput(this, "UserPoolId", { value: pool.userPoolId });
    new cdk.CfnOutput(this, "UserPoolClientId", { value: client.userPoolClientId });
    new cdk.CfnOutput(this, "CognitoDomain", {
      value: `${props.cognitoDomainPrefix}.auth.${this.region}.amazoncognito.com`,
    });
    new cdk.CfnOutput(this, "StripeApiKeyParameter", { value: stripeApiParameter });
    new cdk.CfnOutput(this, "StripeWebhookSecretParameter", { value: stripeWebhookParameter });
    new cdk.CfnOutput(this, "AppKeyAlias", { value: `alias/st-${stage}` });
    for (const child of this.node.findAll()) {
      reserveLambda(child);
    }
    const reason = {
      cog1: "A passphrase of at least 15 characters is the control. Character-class rules are not required, so a person can use a sentence.",
      cog2: "Authenticator MFA is offered after signup and stays optional. Requiring it would block people who only send letters and do not store a mailing address.",
      apig4: "GET /health and POST /webhooks/stripe are the only routes without a Cognito JWT. Health is a status check. Stripe verifies the webhook signature. Both are throttled.",
      iam5logs: "logs:CreateLogStream and logs:PutLogEvents need the stream suffix on this function's own log group. The resource is that group, not every group.",
      iam5pool: "AdminAddUserToGroup uses userpool/* because the post-confirmation trigger and the pool would otherwise form a cycle. This stack creates one pool.",
      iam5deploy: "The CDK bucket deployment copies objects into the private site bucket and invalidates CloudFront. Those actions use object suffixes on that bucket and the CDK asset bucket.",
      s1: "S3 server access logs store raw client IPs. This stack does not store raw IPs. The bucket is private, SSL-only, and reached through origin access control.",
      cfr1: "There is no geographic block. A letter can come from anywhere a person can post one.",
      cfr2: "WAF and Bot Control stay off unless both flags are set. Stage throttles and reserved concurrency are the spend and scraping control.",
      cfr3: "CloudFront access logs store raw client IPs. This stack does not store raw IPs. API 4xx and 5xx alarms cover abuse.",
    };
    NagSuppressions.addResourceSuppressions(pool, [
      { id: "AwsSolutions-COG1", reason: reason.cog1 },
      { id: "AwsSolutions-COG2", reason: reason.cog2 },
    ], true);
    NagSuppressions.addResourceSuppressionsByPath(this, [
      `${this.node.path}/HttpApi/GET--health/Resource`,
      `${this.node.path}/HttpApi/POST--webhooks--stripe/Resource`,
    ], [{ id: "AwsSolutions-APIG4", reason: reason.apig4 }]);
    for (const fn of [healthFn, profileFn, matchingFn, addressFn, paymentsFn, webhookFn, accountFn, preservationFn, notifyFn, preTokenFn]) {
      NagSuppressions.addResourceSuppressions(fn.role!, [
        { id: "AwsSolutions-IAM5", reason: reason.iam5logs },
      ], true);
    }
    NagSuppressions.addResourceSuppressions(postConfirmFn.role!, [
      { id: "AwsSolutions-IAM5", reason: `${reason.iam5logs} ${reason.iam5pool}` },
    ], true);
    NagSuppressions.addResourceSuppressions(siteBucket, [
      { id: "AwsSolutions-S1", reason: reason.s1 },
    ], true);
    NagSuppressions.addResourceSuppressions(distribution, [
      { id: "AwsSolutions-CFR1", reason: reason.cfr1 },
      { id: "AwsSolutions-CFR2", reason: reason.cfr2 },
      { id: "AwsSolutions-CFR3", reason: reason.cfr3 },
    ], true);
    for (const child of this.node.findAll()) {
      if (child.node.path.includes("CDKBucketDeployment") && child.node.id === "ServiceRole") {
        NagSuppressions.addResourceSuppressions(child, [
          { id: "AwsSolutions-IAM4", reason: reason.iam5deploy },
          { id: "AwsSolutions-IAM5", reason: reason.iam5deploy },
        ], true);
      }
    }
  }
}

function table(
  stack: cdk.Stack,
  id: string,
  partitionKey: string,
  sortKey: string | undefined,
  removal: cdk.RemovalPolicy,
  key: kms.IKey,
) {
  return new dynamodb.Table(stack, id, {
    partitionKey: { name: partitionKey, type: dynamodb.AttributeType.STRING },
    sortKey: sortKey ? { name: sortKey, type: dynamodb.AttributeType.STRING } : undefined,
    billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
    encryption: dynamodb.TableEncryption.CUSTOMER_MANAGED,
    encryptionKey: key,
    deletionProtection: true,
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
  reserved: number,
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
    reservedConcurrentExecutions: reserved,
    environment: { STAGE: stage, ...environment },
    tracing: lambda.Tracing.DISABLED,
  });
}

function grantParameter(fn: lambda.Function, parameterArn: string, key: kms.IKey) {
  fn.addToRolePolicy(new iam.PolicyStatement({
    actions: ["ssm:GetParameter"],
    resources: [parameterArn],
  }));
  fn.addToRolePolicy(new iam.PolicyStatement({
    actions: ["kms:Decrypt"],
    resources: [key.keyArn],
    conditions: {
      StringEquals: { "kms:ViaService": `ssm.${cdk.Stack.of(fn).region}.amazonaws.com` },
    },
  }));
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

function reserveLambda(node: IConstruct) {
  if (node instanceof lambda.CfnFunction) {
    if (node.reservedConcurrentExecutions == null) node.reservedConcurrentExecutions = 1;
    return;
  }
  if (cdk.CfnResource.isCfnResource(node) && node.cfnResourceType === "AWS::Lambda::Function") {
    node.addPropertyOverride("ReservedConcurrentExecutions", 1);
  }
}

class ReservedConcurrency implements cdk.IAspect {
  visit(node: IConstruct): void {
    reserveLambda(node);
  }
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
