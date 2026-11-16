// Hikaya hub web viewer. Plain JavaScript, no build step, no dependencies.
//
// This browser pairs with the hub like any other device: the passphrase is
// sent once to register it, and the returned token is kept in localStorage.
// Dictation text is only ever inserted with textContent, never as HTML.
"use strict";

const STRINGS = {
  en: {
    pairTitle: "Open your archive",
    pairHint:
      "Pair this browser with your hub once. The passphrase is not stored.",
    deviceName: "Name for this browser",
    passphrase: "Hub passphrase",
    pair: "Pair",
    lockedTitle: "The hub is locked",
    lockedHint: "It restarted. Enter its passphrase to unlock it.",
    unlock: "Unlock",
    uninitialized:
      "This hub has no archive yet. Connect the Hikaya app to it first; that creates the archive.",
    search: "Search your dictations",
    more: "Load more",
    forget: "Forget this browser",
    empty: "No dictations yet.",
    noMatch: "Nothing matches.",
    unlocked: "Unlocked",
    locked: "Locked",
    unreachable: "Hub unreachable",
    devicePrefix: "Paired as",
    revoked: "This browser is no longer allowed on the hub. Pair it again.",
    defaultName: "Browser",
    dictations: "Dictations",
    meetings: "Meetings",
    searchMeetings: "Search your meetings",
    noMeetings: "No meetings yet.",
    back: "← All meetings",
    transcript: "Transcript",
    you: "You",
    others: "Others",
    lines: "lines",
  },
  fr: {
    pairTitle: "Ouvrir vos archives",
    pairHint:
      "Associez ce navigateur à votre hub une seule fois. La phrase secrète n'est pas conservée.",
    deviceName: "Nom de ce navigateur",
    passphrase: "Phrase secrète du hub",
    pair: "Associer",
    lockedTitle: "Le hub est verrouillé",
    lockedHint:
      "Il a redémarré. Saisissez sa phrase secrète pour le déverrouiller.",
    unlock: "Déverrouiller",
    uninitialized:
      "Ce hub n'a pas encore d'archives. Connectez-y d'abord l'application Hikaya ; c'est elle qui les crée.",
    search: "Rechercher dans vos dictées",
    more: "Afficher plus",
    forget: "Oublier ce navigateur",
    empty: "Aucune dictée pour l'instant.",
    noMatch: "Aucun résultat.",
    unlocked: "Déverrouillé",
    locked: "Verrouillé",
    unreachable: "Hub injoignable",
    devicePrefix: "Associé en tant que",
    revoked:
      "Ce navigateur n'est plus autorisé sur le hub. Associez-le de nouveau.",
    defaultName: "Navigateur",
    dictations: "Dictées",
    meetings: "Réunions",
    searchMeetings: "Rechercher dans vos réunions",
    noMeetings: "Aucune réunion pour l'instant.",
    back: "← Toutes les réunions",
    transcript: "Transcription",
    you: "Vous",
    others: "Autres",
    lines: "lignes",
  },
};

const lang =
  navigator.language && navigator.language.toLowerCase().startsWith("fr")
    ? "fr"
    : "en";
const t = (key) => STRINGS[lang][key] ?? STRINGS.en[key] ?? key;
const $ = (id) => document.getElementById(id);
const TOKEN_KEY = "hikaya.token";
const PAGE = 30;

const store = {
  get: () => {
    try {
      return localStorage.getItem(TOKEN_KEY);
    } catch {
      return null;
    }
  },
  set: (token) => {
    try {
      localStorage.setItem(TOKEN_KEY, token);
    } catch {
      /* private mode: pairing lasts for this page only */
    }
  },
  clear: () => {
    try {
      localStorage.removeItem(TOKEN_KEY);
    } catch {
      /* nothing stored */
    }
  },
};
let memoryToken = store.get();
let oldest = null; // created_at of the last shown dictation, for paging
let query = "";
let tab = "dictations"; // or "meetings"

class HubError extends Error {
  constructor(status, code, message) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

async function api(path, { method = "GET", body, auth = true } = {}) {
  const headers = {};
  if (body !== undefined) headers["content-type"] = "application/json";
  if (auth && memoryToken) headers.authorization = `Bearer ${memoryToken}`;
  let response;
  try {
    response = await fetch(path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      cache: "no-store",
    });
  } catch {
    throw new HubError(0, "unreachable", t("unreachable"));
  }
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    throw new HubError(
      response.status,
      data?.error ?? "error",
      data?.message ?? response.statusText,
    );
  }
  return data;
}

function show(...ids) {
  for (const id of ["pair", "unlock", "uninitialized", "archive"])
    $(id).hidden = !ids.includes(id);
}

function setState(text, tone) {
  $("state").textContent = text;
  $("state").className = `state ${tone ?? ""}`;
}

function showError(error) {
  $("error").textContent = error ? error.message : "";
  $("error").hidden = !error;
}

/** Appends `text` to `parent`, wrapping the searched words in <mark>. */
function highlighted(parent, text) {
  const words = query
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean)
    .map((w) => w.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase());
  if (!words.length) {
    parent.textContent = text;
    return;
  }
  for (const piece of text.split(/([\p{L}\p{N}]+)/u)) {
    const plain = piece.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
    if (piece && words.some((w) => plain.startsWith(w))) {
      const mark = document.createElement("mark");
      mark.textContent = piece;
      parent.append(mark);
    } else {
      parent.append(document.createTextNode(piece));
    }
  }
}

function render(dictations, append) {
  const list = $("results");
  if (!append) list.replaceChildren();
  const format = new Intl.DateTimeFormat(lang, {
    dateStyle: "medium",
    timeStyle: "short",
  });
  for (const d of dictations) {
    const item = document.createElement("li");
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.textContent = `${format.format(new Date(d.created_at))} · ${d.device}${d.language ? ` · ${d.language}` : ""}`;
    const text = document.createElement("div");
    text.className = "text";
    highlighted(text, d.text);
    item.append(meta, text);
    list.append(item);
  }
  if (dictations.length) oldest = dictations[dictations.length - 1].created_at;
  $("more").hidden = dictations.length < PAGE;
  const nothing = !append && dictations.length === 0;
  $("empty").hidden = !nothing;
  $("empty").textContent = query ? t("noMatch") : t("empty");
}

const clock = (ms) => {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = String(Math.floor((total % 3600) / 60)).padStart(2, "0");
  const sec = String(total % 60).padStart(2, "0");
  return h ? `${h}:${m}:${sec}` : `${m}:${sec}`;
};

function renderMeetings(meetings, append) {
  const list = $("results");
  if (!append) list.replaceChildren();
  const format = new Intl.DateTimeFormat(lang, {
    dateStyle: "medium",
    timeStyle: "short",
  });
  for (const m of meetings) {
    const item = document.createElement("li");
    item.className = "clickable";
    item.tabIndex = 0;
    item.setAttribute("role", "button");
    const title = document.createElement("div");
    title.className = "title";
    highlighted(title, m.title);
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.textContent = `${format.format(new Date(m.started_at))} · ${clock(m.duration_ms)} · ${m.device} · ${m.line_count} ${t("lines")}`;
    item.append(title, meta);
    const open = () => void openMeeting(m.id);
    item.addEventListener("click", open);
    item.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") open();
    });
    list.append(item);
  }
  if (meetings.length) oldest = meetings[meetings.length - 1].started_at;
  $("more").hidden = meetings.length < PAGE;
  const nothing = !append && meetings.length === 0;
  $("empty").hidden = !nothing;
  $("empty").textContent = query ? t("noMatch") : t("noMeetings");
}

/** Shows Markdown from the summary as safe elements: headings, bullets, text. */
function renderSummary(parent, markdown) {
  parent.replaceChildren();
  let list = null;
  for (const raw of markdown.split("\n")) {
    const line = raw.trim();
    if (!line) {
      list = null;
      continue;
    }
    const heading = line.match(/^#{1,6}\s+(.*)$/);
    const bullet = line.match(/^[-*]\s+(?:\[[ xX]\]\s+)?(.*)$/);
    if (heading) {
      list = null;
      const h = document.createElement("h4");
      h.textContent = heading[1];
      parent.append(h);
    } else if (bullet) {
      if (!list) {
        list = document.createElement("ul");
        parent.append(list);
      }
      const li = document.createElement("li");
      li.textContent = bullet[1];
      list.append(li);
    } else {
      list = null;
      const p = document.createElement("p");
      p.textContent = line;
      parent.append(p);
    }
  }
}

async function openMeeting(id) {
  try {
    const m = await api(`/api/meetings/${encodeURIComponent(id)}`);
    const format = new Intl.DateTimeFormat(lang, {
      dateStyle: "full",
      timeStyle: "short",
    });
    $("meeting-title").textContent = m.title;
    $("meeting-meta").textContent =
      `${format.format(new Date(m.started_at))} · ${clock(m.duration_ms)} · ${m.device}`;
    $("meeting-summary").hidden = !m.summary;
    if (m.summary) renderSummary($("meeting-summary"), m.summary);
    const lines = $("meeting-lines");
    lines.replaceChildren();
    for (const l of m.lines) {
      const li = document.createElement("li");
      const time = document.createElement("span");
      time.className = "time";
      time.textContent = clock(l.start_ms);
      const who = document.createElement("span");
      who.className = `speaker ${l.speaker === "you" ? "you" : ""}`;
      who.textContent = `${t(l.speaker === "you" ? "you" : "others")}:`;
      li.append(time, who, document.createTextNode(l.text));
      lines.append(li);
    }
    showMeeting(true);
    showError(null);
  } catch (error) {
    await handle(error);
  }
}

function showMeeting(on) {
  $("meeting").hidden = !on;
  for (const id of ["results", "search", "more", "empty"]) {
    if (on) $(id).hidden = true;
  }
  if (!on) $("search").hidden = false;
  window.scrollTo(0, 0);
}

function selectTab(name) {
  tab = name;
  for (const which of ["dictations", "meetings"]) {
    $(`tab-${which}`).setAttribute("aria-selected", String(which === name));
  }
  $("q").placeholder = t(name === "meetings" ? "searchMeetings" : "search");
  $("q").setAttribute("aria-label", $("q").placeholder);
  showMeeting(false);
  oldest = null;
  void load(false);
}

async function load(append = false) {
  const params = new URLSearchParams({ limit: String(PAGE) });
  if (query) params.set("q", query);
  if (append && oldest !== null) params.set("before", String(oldest));
  try {
    if (tab === "meetings") {
      renderMeetings(await api(`/api/meetings?${params}`), append);
    } else {
      render(await api(`/api/dictations?${params}`), append);
    }
    $("results").hidden = false;
    showError(null);
  } catch (error) {
    await handle(error);
  }
}

async function handle(error) {
  if (error.code === "locked") return refresh();
  if (error.status === 401) {
    memoryToken = null;
    store.clear();
    show("pair");
    return showError(new Error(t("revoked")));
  }
  showError(error);
}

async function refresh() {
  let status;
  try {
    status = await api("/api/status", { auth: false });
  } catch (error) {
    setState(t("unreachable"), "warn");
    return showError(error);
  }
  showError(null);
  if (status.state === "uninitialized") {
    setState("", "");
    return show("uninitialized");
  }
  if (status.state === "locked") {
    setState(t("locked"), "warn");
    return show("unlock");
  }
  setState(t("unlocked"), "ok");
  if (!memoryToken) return show("pair");
  try {
    const me = await api("/api/me");
    $("device").textContent = `${t("devicePrefix")} ${me.name}`;
    show("archive");
    await load(false);
  } catch (error) {
    await handle(error);
  }
}

function busy(form, on) {
  for (const el of form.elements) el.disabled = on;
}

$("pair").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, true);
  try {
    const result = await api("/api/devices", {
      method: "POST",
      auth: false,
      body: { name: $("pair-name").value, passphrase: $("pair-pass").value },
    });
    memoryToken = result.token;
    store.set(result.token);
    await refresh();
  } catch (error) {
    showError(error);
  } finally {
    $("pair-pass").value = "";
    busy(form, false);
  }
});

$("unlock").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, true);
  try {
    await api("/api/unlock", {
      method: "POST",
      auth: false,
      body: { passphrase: $("unlock-pass").value },
    });
    await refresh();
  } catch (error) {
    showError(error);
  } finally {
    $("unlock-pass").value = "";
    busy(form, false);
  }
});

let searchTimer;
$("q").addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => {
    query = $("q").value.trim();
    oldest = null;
    void load(false);
  }, 250);
});
$("search").addEventListener("submit", (event) => event.preventDefault());
$("more").addEventListener("click", () => void load(true));
$("tab-dictations").addEventListener("click", () => selectTab("dictations"));
$("tab-meetings").addEventListener("click", () => selectTab("meetings"));
$("back").addEventListener("click", () => {
  showMeeting(false);
  void load(false);
});
$("forget").addEventListener("click", () => {
  memoryToken = null;
  store.clear();
  $("results").replaceChildren();
  void refresh();
});

document.documentElement.lang = lang;
for (const el of document.querySelectorAll("[data-t]"))
  el.textContent = t(el.dataset.t);
$("q").placeholder = t("search");
$("q").setAttribute("aria-label", t("search"));
$("pair-name").value =
  `${t("defaultName")} ${/iPhone|iPad/.test(navigator.userAgent) ? "iPhone" : ""}`.trim();
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") void refresh();
});
void refresh();
