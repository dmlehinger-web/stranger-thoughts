import { finishLogin } from "../auth.js";

const params = new URLSearchParams(window.location.search);
const code = params.get("code");
const note = document.querySelector("[data-status]");

if (window.ST_CONFIG.mock) {
  note.textContent = "This preview does not talk to Cognito. Use the writer or reader pages.";
} else if (code) {
  finishLogin(code)
    .then(() => {
      const role = sessionStorage.getItem("st-role");
      window.location.assign(role === "reader" ? "../../read/home/" : "../../write/home/");
    })
    .catch(() => {
      note.textContent = "Sign-in could not be finished. Start again from the home page.";
    });
} else {
  note.textContent = "This page is only used after Cognito sends you back.";
}
