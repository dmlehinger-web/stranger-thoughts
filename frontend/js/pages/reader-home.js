import { client } from "../api.js";
import { showStatus } from "../ui.js";

const api = client();
const status = document.querySelector("[data-status]");
const preserveMatch = document.querySelector("#preserve-match");

const LABELS = {
  mark_mailed: "I mailed my letter",
  mark_received: "Mark letter received",
  mark_reply_mailed: "Mark reply mailed",
};

function money(cents) {
  return `$${(cents / 100).toFixed(0)}`;
}

function fillMatches(letters) {
  preserveMatch.replaceChildren();
  for (const letter of letters) {
    const option = document.createElement("option");
    option.value = letter.match_id || letter.id;
    option.textContent = letter.writer_pseudonym || letter.writer;
    preserveMatch.append(option);
  }
}

async function render() {
  const home = await api.readerHome();
  document.querySelector("#reader-name").textContent = home.pseudonym;
  document.querySelector("#tips").textContent = money(home.tips_received_cents);
  const list = document.querySelector("#letters");
  list.replaceChildren();
  for (const letter of home.letters) {
    const item = document.createElement("li");
    item.className = "letter simple";
    const who = document.createElement("div");
    const title = document.createElement("h3");
    title.textContent = letter.writer_pseudonym || letter.writer;
    const when = document.createElement("p");
    when.className = "when";
    when.textContent = api.formatDate(letter.matched_at || letter.matchedAt);
    const transit = document.createElement("p");
    transit.className = "transit";
    transit.textContent = letter.status || "Not mailed yet";
    who.append(title, when, transit);
    const actions = document.createElement("div");
    actions.className = "actions";
    const matchId = letter.match_id || letter.id;
    for (const action of letter.actions || []) {
      const button = document.createElement("button");
      button.className = "btn btn-secondary";
      button.type = "button";
      button.textContent = LABELS[action] || action;
      button.addEventListener("click", async () => {
        try {
          if (action === "mark_received") await api.markReceived(matchId);
          else await api.markMailed(matchId);
          await render();
        } catch (error) {
          showStatus(status, error);
        }
      });
      actions.append(button);
    }
    const report = document.createElement("button");
    report.className = "text-link";
    report.type = "button";
    report.textContent = "Report";
    report.addEventListener("click", async () => {
      await api.report(matchId);
      await render();
    });
    actions.append(report);
    item.append(who, actions);
    list.append(item);
  }
  fillMatches(home.letters);
}

document.querySelector("#payouts")?.addEventListener("click", async (event) => {
  event.preventDefault();
  const result = await api.payoutLink();
  status.textContent = result.url
    ? "Opening Stripe."
    : "Payout setup opens Stripe’s hosted page once a test key is in place. Nothing was sent.";
  if (result.url) window.location.assign(result.url);
});

document.querySelector("#mail-alerts").addEventListener("change", async (event) => {
  try {
    await api.saveSettings(event.target.checked);
    status.textContent = event.target.checked
      ? "Mailed-letter notes will go to your verified email."
      : "Mailed-letter notes stay on this page.";
  } catch (error) {
    event.target.checked = !event.target.checked;
    showStatus(status, error);
  }
});

async function keep(kind) {
  const matchId = preserveMatch.value;
  const body = document.querySelector("#preserve-body").value;
  try {
    if (kind === "private") await api.savePrivate(matchId, body);
    else await api.offerArchive(matchId, body);
    status.textContent = kind === "private"
      ? "Private copy saved. It is not how the letter is sent."
      : "Offered to the archive. It stays pending until someone curates it.";
    document.querySelector("#preserve-body").value = "";
  } catch (error) {
    showStatus(status, error);
  }
}

document.querySelector("#save-private").addEventListener("click", () => keep("private"));
document.querySelector("#offer-archive").addEventListener("click", () => keep("archive"));

async function loadSettings() {
  const settings = await api.settings();
  document.querySelector("#mail-alerts").checked = settings.mail_alerts !== false;
}

render();
loadSettings();
