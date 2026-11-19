import { client } from "../api.js";
import { signUp } from "../auth.js";
import { showStatus } from "../ui.js";

const form = document.querySelector("#signup");
const about = document.querySelector("#about");
document.querySelector("#hide").addEventListener("click", () => {
  about.hidden = !about.hidden;
  document.querySelector("#hide").textContent = about.hidden ? "Show" : "Hide";
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const data = new FormData(form);
  const pseudonym = String(data.get("pseudonym") || "").trim();
  const filled = (name) => {
    const value = String(data.get(name) || "").trim();
    return value || null;
  };
  const demographics = about.hidden
    ? null
    : {
        age_range: filled("age_range"),
        location_bucket: filled("location_bucket"),
        race: filled("race"),
        economic_status: filled("economic_status"),
      };
  try {
    if (!window.ST_CONFIG.mock) {
      await signUp("writer", String(data.get("email") || ""), String(data.get("password") || ""));
      document.querySelector("[data-status]").textContent =
        "Check your email. Descriptors are saved only after the email is confirmed and you sign in.";
      return;
    }
    await client().saveWriter(pseudonym, demographics);
    window.location.assign("home/");
  } catch (error) {
    showStatus(document.querySelector("[data-status]"), error);
  }
});
