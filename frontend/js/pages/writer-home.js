import { client } from "../api.js";
import { mountTip, showStatus } from "../ui.js";

const api = client();
const list = document.querySelector("#letters");
const name = document.querySelector("#stranger-name");
const address = document.querySelector("#address");

function letterRow(letter) {
  const item = document.createElement("li");
  item.className = "letter";
  item.innerHTML = `
    <div>
      <h3></h3>
      <p class="when"></p>
    </div>
    <label class="sent"><input type="checkbox"> Sent</label>
    <button class="tip-btn" type="button" data-tip="reader">Tip</button>`;
  item.querySelector("h3").textContent = letter.reader_pseudonym;
  item.querySelector(".when").textContent = api.formatDate(letter.matched_at);
  const box = item.querySelector("input");
  box.checked = letter.sent;
  box.addEventListener("change", async () => {
    try {
      await api.setSent(letter.match_id, box.checked);
    } catch (error) {
      box.checked = !box.checked;
      showStatus(document.querySelector("[data-status]"), error);
    }
  });
  const tip = item.querySelector("[data-tip]");
  tip.dataset.match = letter.match_id;
  tip.dataset.name = letter.reader_pseudonym;
  return item;
}

function paintAddress(payload) {
  address.replaceChildren();
  if (!payload) {
    address.hidden = true;
    return;
  }
  address.hidden = false;
  const lines = [
    payload.reader_pseudonym,
    payload.address.line1,
    `${payload.address.city}, ${payload.address.region} ${payload.address.postal_code}`,
    payload.address.country,
  ];
  lines.forEach((line, index) => {
    const row = document.createElement("p");
    if (index === 0) row.className = "who";
    row.textContent = line;
    address.append(row);
  });
  const note = document.createElement("p");
  note.className = "note";
  note.textContent = "Shown only to you, for this match.";
  address.append(note);
}

async function render() {
  const home = await api.writerHome();
  name.textContent = home.current ? home.current.reader_pseudonym : "No one yet";
  list.replaceChildren(...home.letters.map(letterRow));
  try {
    paintAddress(home.current ? await api.currentAddress() : null);
  } catch (error) {
    address.hidden = true;
    showStatus(document.querySelector("[data-status]"), error);
  }
}

mountTip(document.body);

document.querySelector("#rotate").addEventListener("click", async () => {
  try {
    await api.rotate();
    await render();
  } catch (error) {
    showStatus(document.querySelector("[data-status]"), error);
  }
});

render();
