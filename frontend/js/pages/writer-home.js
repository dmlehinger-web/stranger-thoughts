import { client } from "../api.js";
import { mountTip, showStatus } from "../ui.js";

const api = client();
const list = document.querySelector("#letters");
const name = document.querySelector("#stranger-name");
const status = document.querySelector("[data-status]");
const preserveMatch = document.querySelector("#preserve-match");

const LABELS = {
  mark_mailed: "I mailed my letter",
  mark_received: "Mark letter received",
  mark_reply_mailed: "Mark reply mailed",
};

function letterRow(letter) {
  const item = document.createElement("li");
  item.className = "letter";
  const who = document.createElement("div");
  const title = document.createElement("h3");
  title.textContent = letter.reader_pseudonym;
  const when = document.createElement("p");
  when.className = "when";
  when.textContent = api.formatDate(letter.matched_at);
  const transit = document.createElement("p");
  transit.className = "transit";
  transit.textContent = letter.status || "Not mailed yet";
  who.append(title, when, transit);
  const actions = document.createElement("div");
  actions.className = "actions";
  for (const action of letter.actions || []) {
    const button = document.createElement("button");
    button.className = "btn btn-secondary";
    button.type = "button";
    button.textContent = LABELS[action] || action;
    button.addEventListener("click", async () => {
      try {
        if (action === "mark_received") await api.markReceived(letter.match_id);
        else await api.markMailed(letter.match_id);
        await render();
      } catch (error) {
        showStatus(status, error);
      }
    });
    actions.append(button);
  }
  const tip = document.createElement("button");
  tip.className = "tip-btn";
  tip.type = "button";
  tip.textContent = "Tip";
  tip.dataset.tip = "reader";
  tip.dataset.match = letter.match_id;
  tip.dataset.name = letter.reader_pseudonym;
  actions.append(tip);
  item.append(who, actions);
  return item;
}

function fillMatches(letters) {
  preserveMatch.replaceChildren();
  for (const letter of letters) {
    const option = document.createElement("option");
    option.value = letter.match_id;
    option.textContent = letter.reader_pseudonym;
    preserveMatch.append(option);
  }
}

async function render() {
  const home = await api.writerHome();
  name.textContent = home.current ? home.current.reader_pseudonym : "No one yet";
  list.replaceChildren(...home.letters.map(letterRow));
  fillMatches(home.letters);
  mountTip(document.body);
}

async function loadSettings() {
  const settings = await api.settings();
  document.querySelector("#mail-alerts").checked = settings.mail_alerts !== false;
}

mountTip(document.body);

document.querySelector("#rotate").addEventListener("click", async () => {
  try {
    await api.rotate();
    await render();
  } catch (error) {
    showStatus(status, error);
  }
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

render();
loadSettings();
