import { mockApi } from "./mock.js";

async function live(path, options = {}) {
  const headers = new Headers(options.headers || {});
  headers.set("content-type", "application/json");
  const token = sessionStorage.getItem("st-access-token");
  if (token) headers.set("authorization", `Bearer ${token}`);
  const response = await fetch(`${window.ST_CONFIG.apiBase}${path}`, {
    method: options.method || "GET",
    headers,
    body: options.body ? JSON.stringify(options.body) : undefined,
  });
  const payload = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(payload.error || "invalid_request");
    error.code = payload.error || "invalid_request";
    throw error;
  }
  return payload;
}

const liveApi = {
  formatDate: mockApi.formatDate,
  writerHome: () => live("/writers/home"),
  currentAddress: async () => {
    const home = await live("/writers/home");
    if (!home.current) return null;
    return live(`/matches/${home.current.match_id}/address`);
  },
  saveWriter: (pseudonym) => live("/writers/profile", { method: "POST", body: { pseudonym } }),
  saveDemographics: (body) => live("/writers/demographics", { method: "PUT", body }),
  deleteDemographics: () => live("/writers/demographics", { method: "DELETE" }),
  rotate: () => live("/writers/rotate", { method: "POST", body: {} }),
  setSent: (matchId, sent) => live(`/matches/${matchId}/sent`, { method: "POST", body: { sent } }),
  markMailed: (matchId) => live(`/matches/${matchId}/mailed`, { method: "POST", body: {} }),
  markReceived: (matchId) => live(`/matches/${matchId}/received`, { method: "POST", body: {} }),
  settings: () => live("/account/settings"),
  saveSettings: (mailAlerts) => live("/account/settings", { method: "PUT", body: { mail_alerts: mailAlerts } }),
  savePrivate: (matchId, body) => live("/preservation/private", { method: "POST", body: { match_id: matchId, body } }),
  offerArchive: (matchId, body) => live("/preservation/archive", { method: "POST", body: { match_id: matchId, body } }),
  saveReader: (pseudonym, address) => live("/readers/profile", { method: "POST", body: { pseudonym, address } }),
  readerHome: () => live("/readers/home"),
  tipReader: (matchId, amountCents) => live("/tips/reader", { method: "POST", body: { match_id: matchId, amount_cents: amountCents } }),
  tipBuilder: (amountCents) => live("/tips/builder", { method: "POST", body: { amount_cents: amountCents } }),
  report: (matchId) => live("/readers/reports", { method: "POST", body: { match_id: matchId, reason: "unwanted" } }),
  payoutLink: () => live("/readers/payouts/link", { method: "POST", body: {} }),
};

export function client() {
  return window.ST_CONFIG.mock ? mockApi : liveApi;
}
