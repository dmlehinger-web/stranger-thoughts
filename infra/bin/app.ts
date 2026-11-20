import * as cdk from "aws-cdk-lib";
import { StrangerThoughtsStack } from "../lib/stack.js";

const app = new cdk.App();
const stage = app.node.tryGetContext("stage") ?? "dev";
if (stage !== "dev" && stage !== "prod") {
  throw new Error("stage must be dev or prod");
}

function flag(name: string): boolean {
  const value = app.node.tryGetContext(name);
  return value === true || value === "true";
}

new StrangerThoughtsStack(app, `st-${stage}`, {
  stackName: `st-${stage}`,
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: "us-east-1",
  },
  stage,
  siteHost: app.node.tryGetContext("siteHost") ?? "preview.invalid",
  cognitoDomainPrefix:
    app.node.tryGetContext("cognitoDomainPrefix") ?? `stranger-thoughts-${stage}-preview`,
  enableWaf: flag("enableWaf"),
  enableBotControl: flag("enableBotControl"),
  enableSesIdentity: flag("enableSesIdentity"),
  sesEnabled: flag("sesEnabled"),
  alertsTopicArn:
    app.node.tryGetContext("alertsTopicArn") ??
    "arn:aws:sns:us-east-1:204161828199:st-security-alerts",
  sesIdentityArn:
    app.node.tryGetContext("sesIdentityArn") ??
    "arn:aws:ses:us-east-1:204161828199:identity/astrangerreads.com",
  sesFromAddress: app.node.tryGetContext("sesFromAddress") ?? "desk@astrangerreads.com",
  maxRotatesPerDay: app.node.tryGetContext("maxRotatesPerDay") ?? "3",
  maxAddressReadsPerDay: app.node.tryGetContext("maxAddressReadsPerDay") ?? "40",
  certificateArn: app.node.tryGetContext("certificateArn"),
});
