import { client } from "../api.js";
import { signUp } from "../auth.js";
import { showStatus } from "../ui.js";

document.querySelector("#signup").addEventListener("submit", async (event) => {
  event.preventDefault();
  const data = new FormData(event.currentTarget);
  const address = {
    line1: String(data.get("line1") || ""),
    line2: String(data.get("line2") || ""),
    city: String(data.get("city") || ""),
    region: String(data.get("region") || ""),
    postal_code: String(data.get("postal_code") || ""),
    country: String(data.get("country") || "US"),
  };
  try {
    if (!window.ST_CONFIG.mock) {
      await signUp("reader", String(data.get("email") || ""), String(data.get("password") || ""));
      document.querySelector("[data-status]").textContent =
        "Check your email. The mailing address is saved only after it is confirmed and you sign in.";
      return;
    }
    await client().saveReader(String(data.get("pseudonym") || "").trim(), address);
    window.location.assign("home/");
  } catch (error) {
    showStatus(document.querySelector("[data-status]"), error);
  }
});
