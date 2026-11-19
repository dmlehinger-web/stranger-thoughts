import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const domain = process.env.COGNITO_DOMAIN || "";
const clientId = process.env.COGNITO_CLIENT_ID || "";
const region = process.env.AWS_REGION || "us-east-1";

for (const value of [domain, clientId, region]) {
  if (/sk_(live|test)_|whsec_|rk_live_|rk_test_/.test(value)) {
    console.error("Refusing to write a Stripe secret into config.js");
    process.exit(1);
  }
}
if (!domain || !clientId) {
  console.error("Set COGNITO_DOMAIN and COGNITO_CLIENT_ID from the stack outputs.");
  process.exit(1);
}

const file = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../frontend/js/config.js");
const body = `window.ST_CONFIG = {
  mock: false,
  apiBase: "/api",
  region: ${JSON.stringify(region)},
  cognitoDomain: ${JSON.stringify(domain)},
  clientId: ${JSON.stringify(clientId)},
};
`;
fs.writeFileSync(file, body);
console.log(`Wrote ${file}. Do not commit this file. Run git checkout -- frontend/js/config.js after deploy.`);
