// Shared helpers for the setup and admin pages. No framework, no build step:
// everything user-provided is inserted with textContent, never innerHTML.
"use strict";

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

/** JSON request. Resolves to the parsed body; rejects with Error(message). */
async function api(path, { method = "GET", body } = {}) {
  const res = await fetch(path, {
    method,
    credentials: "same-origin",
    headers: {
      "Content-Type": "application/json",
      "X-OurShell-Admin": "1",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let data = null;
  try {
    data = await res.json();
  } catch {
    /* empty or non-JSON body */
  }
  if (!res.ok) {
    const err = new Error((data && data.message) || `Request failed (${res.status})`);
    err.status = res.status;
    err.code = data && data.error;
    throw err;
  }
  return data;
}

/** Shows a .note element with a message (kind: ok | err | warn | info). */
function note(el, kind, text) {
  el.className = `note ${kind}`;
  el.textContent = text;
  el.hidden = !text;
}

/** Builds an element: el("td", { class: "mono" }, "text", child, ...). */
function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === false || v === null || v === undefined) continue;
    if (k === "class") node.className = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children) {
    if (c === null || c === undefined) continue;
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

/** Runs `fn` with `button` disabled, restoring it afterwards. */
async function busy(button, fn) {
  button.disabled = true;
  try {
    return await fn();
  } finally {
    button.disabled = false;
  }
}

function download(filename, text) {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
  const a = el("a", { href: url, download: filename });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

async function copy(text, button) {
  try {
    await navigator.clipboard.writeText(text);
    const old = button.textContent;
    button.textContent = "Copied";
    setTimeout(() => (button.textContent = old), 1500);
  } catch {
    /* clipboard needs a secure context; the text is still on screen */
  }
}

/** Polls /v1/health until it reports `mode`, or gives up after ~60 s. */
async function waitForMode(mode) {
  for (let i = 0; i < 60; i++) {
    await new Promise((r) => setTimeout(r, 1000));
    try {
      const h = await api("/v1/health");
      if (h.mode === mode) return true;
    } catch {
      /* restarting */
    }
  }
  return false;
}
