const KEY = "st-v2-mock";

const POOL = [
  { id: "reader-cedar", pseudonym: "Cedar", line1: "PO Box 000", city: "Example City", region: "ST", postal: "00000", country: "United States" },
  { id: "reader-harbor", pseudonym: "Harbor", line1: "PO Box 014", city: "Example City", region: "ST", postal: "00000", country: "United States" },
  { id: "reader-moth", pseudonym: "Moth", line1: "PO Box 221", city: "Example City", region: "ST", postal: "00000", country: "United States" },
  { id: "reader-elm", pseudonym: "Elm", line1: "PO Box 018", city: "Example City", region: "ST", postal: "00000", country: "United States" },
  { id: "reader-sable", pseudonym: "Sable", line1: "PO Box 030", city: "Example City", region: "ST", postal: "00000", country: "United States" },
  { id: "reader-quill", pseudonym: "Quill", line1: "PO Box 044", city: "Example City", region: "ST", postal: "00000", country: "United States" },
];

function seed() {
  return {
    writer: { pseudonym: "North Window", demographics: null },
    reader: { pseudonym: "Cedar", payoutsReady: true },
    blocked: [],
    rotatesToday: 1,
    letters: [
      { id: "m-cedar", readerId: "reader-cedar", reader: "Cedar", writer: "North Window", matchedAt: "2026-11-12", sent: false, current: true },
      { id: "m-harbor", readerId: "reader-harbor", reader: "Harbor", writer: "North Window", matchedAt: "2026-11-03", sent: true, current: false },
      { id: "m-moth", readerId: "reader-moth", reader: "Moth", writer: "North Window", matchedAt: "2026-10-21", sent: true, current: false },
    ],
    expected: [
      { id: "m-cedar", writer: "North Window", matchedAt: "2026-11-12" },
      { id: "m-elm", writer: "Elm", matchedAt: "2026-11-08" },
      { id: "m-sable", writer: "Sable", matchedAt: "2026-10-30" },
    ],
    tips: [],
    builderCents: 0,
  };
}

function load() {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) || "null");
    if (saved && Array.isArray(saved.letters)) return saved;
  } catch {
    /* start fresh */
  }
  const fresh = seed();
  localStorage.setItem(KEY, JSON.stringify(fresh));
  return fresh;
}

function save(state) {
  localStorage.setItem(KEY, JSON.stringify(state));
}

function formatDate(iso) {
  const [year, month, day] = String(iso).split("T")[0].split("-").map(Number);
  const names = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  if (!year || !month || !day) return String(iso);
  return `${names[month - 1]} ${day}, ${year}`;
}

function homeBody(state) {
  const current = state.letters.find((letter) => letter.current) || null;
  return {
    pseudonym: state.writer.pseudonym,
    current: current
      ? { match_id: current.id, reader_pseudonym: current.reader, matched_at: current.matchedAt }
      : null,
    letters: state.letters.map((letter) => ({
      match_id: letter.id,
      reader_pseudonym: letter.reader,
      matched_at: letter.matchedAt,
      sent: letter.sent,
    })),
  };
}

export const mockApi = {
  formatDate,
  reset() {
    localStorage.removeItem(KEY);
  },
  async writerHome() {
    return homeBody(load());
  },
  async currentAddress() {
    const state = load();
    const current = state.letters.find((letter) => letter.current);
    if (!current) {
      const error = new Error("not_found");
      error.code = "not_found";
      throw error;
    }
    const reader = POOL.find((item) => item.id === current.readerId);
    return {
      match_id: current.id,
      reader_pseudonym: current.reader,
      address: {
        line1: reader.line1,
        city: reader.city,
        region: reader.region,
        postal_code: reader.postal,
        country: reader.country,
      },
    };
  },
  async saveWriter(pseudonym, demographics) {
    const state = load();
    state.writer.pseudonym = pseudonym;
    state.writer.demographics = demographics;
    save(state);
    return { pseudonym };
  },
  async deleteDemographics() {
    const state = load();
    state.writer.demographics = null;
    save(state);
    return { deleted: true };
  },
  async rotate() {
    const state = load();
    if (state.rotatesToday >= 3) {
      const error = new Error("rate_limited");
      error.code = "rate_limited";
      throw error;
    }
    const current = state.letters.find((letter) => letter.current);
    const eligible = POOL.filter((reader) => reader.id !== current?.readerId && !state.blocked.includes(reader.id));
    if (!eligible.length) {
      const error = new Error("no_reader");
      error.code = "no_reader";
      throw error;
    }
    const next = eligible[Math.floor(Math.random() * eligible.length)];
    for (const letter of state.letters) letter.current = false;
    state.letters.unshift({
      id: `m-${Date.now()}`,
      readerId: next.id,
      reader: next.pseudonym,
      writer: state.writer.pseudonym,
      matchedAt: "2026-11-19",
      sent: false,
      current: true,
    });
    state.rotatesToday += 1;
    save(state);
    return homeBody(state);
  },
  async setSent(matchId, sent) {
    const state = load();
    const letter = state.letters.find((item) => item.id === matchId);
    if (!letter) {
      const error = new Error("not_found");
      error.code = "not_found";
      throw error;
    }
    letter.sent = sent;
    save(state);
    return { sent };
  },
  async saveReader(pseudonym) {
    const state = load();
    state.reader.pseudonym = pseudonym;
    save(state);
    return { pseudonym };
  },
  async readerHome() {
    const state = load();
    const received = state.tips.reduce((sum, tip) => sum + tip.amount_cents, 0);
    return {
      pseudonym: state.reader.pseudonym,
      letters: state.expected,
      tips_received_cents: 1800 + received,
      payouts_ready: state.reader.payoutsReady,
    };
  },
  async tipReader(matchId, amountCents) {
    const state = load();
    const letter = state.letters.find((item) => item.id === matchId);
    if (!letter) {
      const error = new Error("not_found");
      error.code = "not_found";
      throw error;
    }
    state.tips.push({ match_id: matchId, reader: letter.reader, amount_cents: amountCents });
    save(state);
    return { url: null, mock: true, reader: letter.reader, amount_cents: amountCents };
  },
  async tipBuilder(amountCents) {
    const state = load();
    state.builderCents += amountCents;
    save(state);
    return { url: null, mock: true, amount_cents: amountCents };
  },
  async report(matchId) {
    const state = load();
    const letter = state.expected.find((item) => item.id === matchId);
    if (!letter) {
      const error = new Error("not_found");
      error.code = "not_found";
      throw error;
    }
    state.expected = state.expected.filter((item) => item.id !== matchId);
    save(state);
    return { blocked: true };
  },
  async payoutLink() {
    return { url: null, mock: true };
  },
};
