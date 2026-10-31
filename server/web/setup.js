// First-run setup wizard (see server/src/web/setup.rs).
"use strict";

const state = { code: "", step: 0, info: {}, dbOk: false, report: null, polling: false };

function go(step) {
  state.step = step;
  for (const p of $$("[data-panel]")) p.hidden = Number(p.dataset.panel) !== step;
  for (const s of $$("[data-step]")) {
    const n = Number(s.dataset.step);
    s.className = n === step ? "on" : n < step ? "done" : "";
  }
  const first = $(`[data-panel="${step}"] input:not([type=radio]):not([type=checkbox]):not(:disabled)`);
  if (first && !first.closest("[hidden]")) first.focus();
}

for (const b of $$("[data-back]")) b.addEventListener("click", () => go(state.step - 1));

async function init() {
  try {
    state.info = await api("/setup/api/info");
    $("#version").textContent = `v${state.info.version}`;
    if (state.info.database_from_env) {
      $("#db-env").hidden = false;
      $("#db-env-name").textContent = state.info.database;
      $("#db-input").hidden = true;
    }
    if (state.info.public_url) $("#public-url").value = state.info.public_url;
    if (state.info.tls_domain) {
      $("#tls-domain").value = state.info.tls_domain;
      setHttpsMode("tls");
    }
    if (state.info.behind_proxy) setHttpsMode("proxy");
    if (state.info.behind_proxy_from_env) {
      for (const r of $$("input[name=https]")) r.disabled = true;
    }
  } catch (e) {
    // Already set up: the server now serves the admin page instead.
    location.href = "/admin";
    return;
  }
  const fromHash = new URLSearchParams(location.hash.slice(1)).get("code");
  // Kept for this tab only, so the page can reload itself (e.g. once the
  // database is reachable) without asking again.
  let saved = null;
  try {
    saved = sessionStorage.getItem("ourshell-setup-code");
  } catch {
    /* storage blocked */
  }
  const code = fromHash || saved;
  if (code) $("#code").value = code;
  go(0);
  if (code && !fromHash) $("#f-code").requestSubmit();
}

$("#f-code").addEventListener("submit", async (e) => {
  e.preventDefault();
  const msg = $("#code-msg");
  const code = $("#code").value.trim();
  try {
    await busy($("#f-code button[type=submit]"), () => api("/setup/api/verify", { method: "POST", body: { code } }));
    state.code = code;
    try {
      sessionStorage.setItem("ourshell-setup-code", code);
    } catch {
      /* storage blocked */
    }
    note(msg, "", "");
    history.replaceState(null, "", location.pathname);
    go(1);
    await showStartupState();
  } catch (err) {
    note(msg, "err", err.message);
  }
});

// Why the server is in setup mode: an unreachable database, or a database
// with existing data (and a missing or wrong master key).
async function showStartupState() {
  let st;
  try {
    st = await api("/setup/api/status", { method: "POST", body: { code: state.code } });
  } catch {
    return;
  }
  if (st.phase === "unreachable") {
    const where = st.database ? ` at ${st.database}` : "";
    if (st.database_from_env) {
      note(
        $("#db-msg"),
        "err",
        `Can't reach the database${where}: ${st.error}. Fix DATABASE_URL in the server's environment ` +
          "(or start the database). The server keeps retrying every 10 seconds and continues on its own once it connects.",
      );
      waitForDatabase();
    } else {
      note($("#db-msg"), "err", `Can't reach the database${where}: ${st.error}. Fix the URL below, or start the database.`);
    }
    return;
  }
  if (st.phase === "new" || st.phase === "existing_data" || state.info.database_from_env) {
    await testDb();
  }
}

// The database comes from the environment and is down: wait for the server
// to reload itself once it's reachable, then start over from its new state.
async function waitForDatabase() {
  if (state.polling) return;
  state.polling = true;
  for (;;) {
    await new Promise((r) => setTimeout(r, 5000));
    try {
      const h = await api("/v1/health");
      if (h.mode === "ready") {
        location.href = "/admin";
        return;
      }
      const st = await api("/setup/api/status", { method: "POST", body: { code: state.code } });
      if (st.phase !== "unreachable") {
        location.reload();
        return;
      }
    } catch {
      /* reloading */
    }
  }
}

function fmtDate(s) {
  if (!s) return "—";
  return new Date(s).toLocaleString();
}

function renderReport(r) {
  const report = $("#db-report");
  const summary = $("#db-summary");
  report.hidden = false;
  $("#existing").hidden = true;
  state.report = r;
  const i = r.inspection;
  const version = i.server_version.split(",")[0];
  if (i.kind === "existing") {
    note(summary, "warn", `Connected (${version}). This database already has an OurShell server's data.`);
    const facts = $("#existing-facts");
    facts.replaceChildren();
    const add = (k, v) => facts.append(el("dt", {}, k), el("dd", {}, v));
    add("Accounts", i.accounts);
    add("Devices", i.devices);
    add("Synced items", i.items);
    add("Schema version", i.schema_version ?? "—");
    add("First account", fmtDate(i.first_org_created));
    if (i.other_tables.length) add("Other tables", i.other_tables.join(", "));
    $("#existing").hidden = false;
    const keyOk = r.key_status === "ok" || r.key_status === "unverifiable";
    $("#import-key-ok").hidden = !keyOk;
    $("#import-key-field").hidden = keyOk || r.key_from_env;
    const msg = $("#import-msg");
    if (r.key_status === "wrong") {
      note(
        msg,
        "err",
        r.key_from_env
          ? "SERVER_MASTER_KEY in the server's environment doesn't match this database. Set it to the original key and restart the server, or clear the database."
          : "The master key this server has doesn't match this database. Enter the original key.",
      );
    } else if (r.key_status === "missing") {
      note(msg, "info", "This server has no master key for this data yet.");
    } else {
      note(msg, "", "");
    }
    $("#import-go").disabled = r.key_from_env && !keyOk;
    syncExistingChoice();
    $("#db-next").disabled = true;
    return;
  }
  state.dbOk = true;
  $("#db-next").disabled = false;
  if (i.kind === "foreign") {
    const names = i.other_tables.slice(0, 8).join(", ") + (i.other_tables.length > 8 ? ", …" : "");
    note(
      summary,
      "warn",
      `Connected (${version}). This database already has ${i.other_tables.length} table(s) of another application (${names}). ` +
        "OurShell will add its own tables next to them; a database of its own is cleaner.",
    );
  } else {
    note(summary, "ok", `Connected (${version}). The database is empty; OurShell will create its tables.`);
  }
}

function syncExistingChoice() {
  const mode = $("input[name=existing]:checked").value;
  $("#import-box").hidden = mode !== "import";
  $("#clear-box").hidden = mode !== "clear";
}
for (const r of $$("input[name=existing]")) r.addEventListener("change", syncExistingChoice);
$("#clear-confirm").addEventListener("input", (e) => {
  $("#clear-go").disabled = e.currentTarget.value.trim() !== "DELETE";
});

function dbUrl() {
  return $("#db-url").value.trim() || null;
}

async function testDb() {
  const msg = $("#db-msg");
  note(msg, "info", "Connecting…");
  state.dbOk = false;
  $("#db-next").disabled = true;
  $("#db-report").hidden = true;
  try {
    const r = await api("/setup/api/inspect", {
      method: "POST",
      body: { code: state.code, database_url: dbUrl() },
    });
    if (r.ok) {
      note(msg, "", "");
      renderReport(r);
    } else {
      note(msg, "err", `Can't connect: ${r.error}`);
    }
  } catch (err) {
    note(msg, "err", err.message);
  }
}
$("#db-test").addEventListener("click", (e) => busy(e.currentTarget, testDb));
$("#db-url").addEventListener("input", () => {
  state.dbOk = false;
  $("#db-next").disabled = true;
  $("#db-report").hidden = true;
});
$("#f-db").addEventListener("submit", (e) => {
  e.preventDefault();
  if (state.dbOk) go(2);
});

$("#import-go").addEventListener("click", async (e) => {
  const msg = $("#import-msg");
  const body = { code: state.code, database_url: dbUrl() };
  if (!$("#import-key-field").hidden) {
    const key = $("#import-key").value.trim();
    if (!key) {
      note(msg, "err", "Enter the original master key.");
      return;
    }
    body.master_key = key;
  }
  if (!$("#import-pw-box").hidden) {
    const pw = $("#import-pw").value;
    if (pw !== $("#import-pw2").value) {
      note(msg, "err", "The passwords don't match.");
      return;
    }
    body.admin_password = pw;
  }
  note(msg, "info", "Checking the key…");
  let r;
  try {
    r = await busy(e.currentTarget, () => api("/setup/api/import", { method: "POST", body }));
  } catch (err) {
    if (err.code === "admin_password_required") {
      $("#import-pw-box").hidden = false;
      note(msg, "info", "The key matches. Choose an admin password, then import.");
      $("#import-pw").focus();
    } else {
      note(msg, "err", err.message);
    }
    return;
  }
  note(msg, "", "");
  showDone(r, { public_url: null, tls_domain: null }, true);
});

$("#clear-go").addEventListener("click", async (e) => {
  const msg = $("#clear-msg");
  note(msg, "info", "Deleting…");
  try {
    await busy(e.currentTarget, () =>
      api("/setup/api/clear", {
        method: "POST",
        body: { code: state.code, database_url: dbUrl(), confirm: $("#clear-confirm").value.trim() },
      }),
    );
  } catch (err) {
    note(msg, "err", err.message);
    return;
  }
  note(msg, "", "");
  $("#clear-confirm").value = "";
  $("#clear-go").disabled = true;
  $("input[name=existing][value=import]").checked = true;
  await testDb();
  note($("#db-msg"), "ok", "Cleared. Continue to set up a new server.");
});

function httpsMode() {
  return $("input[name=https]:checked").value;
}
function setHttpsMode(mode) {
  $(`input[name=https][value=${mode}]`).checked = true;
  $("#tls-fields").hidden = mode !== "tls";
  $("#proxy-note").hidden = mode !== "proxy";
}
for (const r of $$("input[name=https]")) r.addEventListener("change", () => setHttpsMode(httpsMode()));
$("#f-addr").addEventListener("submit", (e) => {
  e.preventDefault();
  const url = $("#public-url").value.trim();
  if (httpsMode() === "tls" && !$("#tls-domain").value.trim()) {
    note($("#addr-msg"), "err", "Enter the domain for the certificate, or turn HTTPS off.");
    return;
  }
  if (url && !/^https?:\/\//.test(url)) {
    note($("#addr-msg"), "err", "The address must start with http:// or https://");
    return;
  }
  note($("#addr-msg"), "", "");
  go(3);
});

$("#f-signup").addEventListener("submit", (e) => {
  e.preventDefault();
  go(4);
});

$("#f-admin").addEventListener("submit", async (e) => {
  e.preventDefault();
  const msg = $("#admin-msg");
  const pw = $("#pw").value;
  if (pw !== $("#pw2").value) {
    note(msg, "err", "The passwords don't match.");
    return;
  }
  const policy = $("input[name=policy]:checked").value;
  const tlsOn = httpsMode() === "tls";
  const body = {
    code: state.code,
    database_url: dbUrl(),
    public_url: $("#public-url").value.trim() || null,
    tls_domain: tlsOn ? $("#tls-domain").value.trim() : null,
    tls_email: tlsOn ? $("#tls-email").value.trim() || null : null,
    behind_proxy: httpsMode() === "proxy",
    signups_enabled: policy !== "closed",
    invite_only: policy === "invite",
    max_devices_per_user: Number($("#max-devices").value || 0),
    admin_password: pw,
  };
  note(msg, "info", "Saving…");
  let r;
  try {
    r = await busy($("#finish"), () => api("/setup/api/finish", { method: "POST", body }));
  } catch (err) {
    note(msg, "err", err.message);
    return;
  }
  note(msg, "", "");
  showDone(r, body, false);
});

async function showDone(r, body, imported) {
  go(5);
  try {
    sessionStorage.removeItem("ourshell-setup-code");
  } catch {
    /* storage blocked */
  }
  const url =
    body.public_url ||
    (body.tls_domain ? `https://${body.tls_domain}` : `${location.protocol}//${location.host}`);
  $("#server-url").textContent = url;
  if (imported) {
    note(
      $("#import-done"),
      "ok",
      `Imported the existing data: ${r.accounts} account(s) and all settings were kept. Sign in from the app as before.`,
    );
  }
  if (r.env_lines && r.env_lines.length) {
    // The URL typed here is the real one; the server only sends a hint
    // without the password.
    const typed = dbUrl();
    const lines = r.env_lines.map((l) => (typed && l.startsWith("DATABASE_URL=") ? `DATABASE_URL=${typed}` : l));
    $("#persist-box").hidden = false;
    $("#persist-why").textContent = `(${r.persistence}.)`;
    const box = $("#env-lines");
    box.replaceChildren();
    for (const line of lines) {
      const copyBtn = el("button", { type: "button", class: "small" }, "Copy");
      copyBtn.addEventListener("click", () => copy(line, copyBtn));
      box.append(el("div", { class: "envline" }, el("div", { class: "secret" }, line), copyBtn));
    }
    $("#copy-env").addEventListener("click", (e) => copy(lines.join("\n"), e.currentTarget));
  }
  if (r.master_key && !imported) {
    $("#key-box").hidden = false;
    $("#master-key").textContent = r.master_key;
    $("#copy-key").addEventListener("click", (e) => copy(r.master_key, e.currentTarget));
  }
  if (r.config_toml) {
    $("#dl-config").hidden = false;
    $("#dl-config").addEventListener("click", () => download("ourshell-config.toml", r.config_toml));
  }
  const ready = await waitForMode("ready");
  note(
    $("#restart-msg"),
    ready ? "ok" : "warn",
    ready
      ? "The server is running."
      : "The server is taking a while to start. Check `docker logs ourshell-server`.",
  );
}

init();
