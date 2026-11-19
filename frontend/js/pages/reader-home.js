import { client } from "../api.js";

const api = client();

function money(cents) {
  return `$${(cents / 100).toFixed(0)}`;
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
    title.textContent = letter.writer || letter.writer_pseudonym;
    const when = document.createElement("p");
    when.className = "when";
    when.textContent = api.formatDate(letter.matchedAt || letter.matched_at);
    who.append(title, when);
    const report = document.createElement("button");
    report.className = "text-link";
    report.type = "button";
    report.textContent = "Report";
    report.addEventListener("click", async () => {
      await api.report(letter.id || letter.match_id);
      await render();
    });
    item.append(who, report);
    list.append(item);
  }
}

document.querySelector("#payouts")?.addEventListener("click", async (event) => {
  event.preventDefault();
  const result = await api.payoutLink();
  const note = document.querySelector("[data-status]");
  note.textContent = result.url
    ? "Opening Stripe."
    : "Payout setup opens Stripe’s hosted page once a test key is in place. Nothing was sent.";
  if (result.url) window.location.assign(result.url);
});

render();
