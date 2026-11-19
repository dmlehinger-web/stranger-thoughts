import { client } from "./api.js";

export function mountTip(root) {
  if (document.querySelector("[data-tip-dialog]")) return;
  const api = client();
  const overlay = document.createElement("div");
  overlay.className = "overlay";
  overlay.hidden = true;
  overlay.dataset.tipDialog = "true";
  overlay.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="tip-title">
      <h2 id="tip-title">Pay what you think this is worth.</h2>
      <p class="quiet" data-tip-copy>A thank-you for the people who keep this place running. Optional, and never in place of a tip to a reader.</p>
      <div class="amounts">
        <button class="amount" type="button" data-amount="300">$3</button>
        <button class="amount" type="button" data-amount="500" aria-pressed="true">$5</button>
        <button class="amount" type="button" data-amount="1000">$10</button>
      </div>
      <p class="hint">Or another amount</p>
      <div class="money">
        <span>$</span>
        <input id="custom-amount" inputmode="numeric" value="0" aria-label="Custom amount in dollars">
      </div>
      <div class="stack tip-actions">
        <button class="btn btn-primary" type="button" data-submit-tip>Leave this tip</button>
        <button class="btn" type="button" data-close-tip>Not now</button>
      </div>
      <p class="status" data-tip-status></p>
    </div>`;
  root.append(overlay);

  let mode = "builder";
  let matchId = null;
  let selected = 500;

  function select(cents) {
    selected = cents;
    overlay.querySelectorAll(".amount").forEach((button) => {
      button.setAttribute("aria-pressed", button.dataset.amount === String(cents) ? "true" : "false");
    });
  }

  overlay.addEventListener("click", (event) => {
    const amount = event.target.closest("[data-amount]");
    if (amount) select(Number(amount.dataset.amount));
    if (event.target === overlay || event.target.closest("[data-close-tip]")) overlay.hidden = true;
  });

  overlay.querySelector("[data-submit-tip]").addEventListener("click", async () => {
    const custom = Number(overlay.querySelector("#custom-amount").value);
    const cents = custom > 0 ? Math.round(custom * 100) : selected;
    const status = overlay.querySelector("[data-tip-status]");
    try {
      const result = mode === "reader"
        ? await api.tipReader(matchId, cents)
        : await api.tipBuilder(cents);
      if (result.url) {
        window.location.assign(result.url);
        return;
      }
      status.textContent = mode === "reader"
        ? `Saved a $${(cents / 100).toFixed(0)} thank-you for ${result.reader}. Nothing was charged.`
        : `Saved a $${(cents / 100).toFixed(0)} note for the people who keep this running. Nothing was charged.`;
    } catch (error) {
      status.textContent = error.code === "payouts_unavailable"
        ? "That reader has not finished payout setup."
        : "That amount needs to be between $1 and $200.";
    }
  });

  document.addEventListener("click", (event) => {
    const button = event.target.closest("[data-tip]");
    if (!button) return;
    mode = button.dataset.tip;
    matchId = button.dataset.match || null;
    const title = overlay.querySelector("#tip-title");
    const copy = overlay.querySelector("[data-tip-copy]");
    if (mode === "reader") {
      title.textContent = `Tip ${button.dataset.name}.`;
      copy.textContent = "This thank-you goes to that reader, through Stripe. Writers never see their legal name.";
    } else {
      title.textContent = "Pay what you think this is worth.";
      copy.textContent = "A thank-you for the people who keep this place running. Optional, and never in place of a tip to a reader.";
    }
    select(500);
    overlay.querySelector("#custom-amount").value = "0";
    overlay.querySelector("[data-tip-status]").textContent = "";
    overlay.hidden = false;
  });
}

export function showStatus(node, error) {
  if (!node) return;
  const messages = {
    rate_limited: "That’s enough rotates for today.",
    no_reader: "No other stranger is available right now.",
    child_age_rejected: "Age ranges start at 18.",
    email_unverified: "Confirm your email before saving this.",
    invalid_request: "Check the fields and try again.",
  };
  node.textContent = messages[error.code] || "Something went wrong. Nothing was saved.";
}
