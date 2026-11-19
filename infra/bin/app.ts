import * as cdk from "aws-cdk-lib";
import { StrangerThoughtsStack } from "../lib/stack.js";

const app = new cdk.App();
const stage = app.node.tryGetContext("stage") ?? "dev";
if (stage !== "dev" && stage !== "prod") {
  throw new Error("stage must be dev or prod");
}

new StrangerThoughtsStack(app, `StrangerThoughts-${stage}`, {
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: "us-east-1",
  },
  stage,
  siteHost: app.node.tryGetContext("siteHost") ?? "preview.invalid",
  cognitoDomainPrefix:
    app.node.tryGetContext("cognitoDomainPrefix") ?? `stranger-thoughts-${stage}-preview`,
  budgetEmail: app.node.tryGetContext("budgetEmail") ?? "budget-alerts@example.com",
  enableWaf: app.node.tryGetContext("enableWaf") !== "false",
  enableGuardDuty: app.node.tryGetContext("enableGuardDuty") === "true",
  maxRotatesPerDay: app.node.tryGetContext("maxRotatesPerDay") ?? "3",
  maxAddressReadsPerDay: app.node.tryGetContext("maxAddressReadsPerDay") ?? "40",
  certificateArn: app.node.tryGetContext("certificateArn"),
});
