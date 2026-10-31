// First-run setup wizard (see server/src/web/setup.rs).
"use strict";

const state = { code: "", step: 0, info: {}, dbOk: false };

function go(step) {
  state.step = step;
  for (const p of $$("[data-panel]")) p.hidden = Number(p.dataset.panel) !== step;
  for (const s of $$("[data-step]")) {
    const n = Number(s.dataset.step);
    s.className = n === step ? "on" : n < step ? "done" : "";
  }
  const first = $(`[data-panel="${step}"] input:not([type=radio]):not([type=checkbox]):not(:disabled)`);
  if (first) first.focus();
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
      $("#tls-on").checked = true;
      $("#tls-domain").value = state.info.tls_domain;
      $("#tls-fields").hidden = false;
    }
  } catch (e) {
    // Already set up: the server now serves the admin page instead.
    location.href = "/admin";
    return;
  }
  const fromHash = new URLSearchParams(location.hash.slice(1)).get("code");
  if (fromHash) $("#code").value = fromHash;
  go(0);
}

$("#f-code").addEventListener("submit", async (e) => {
  e.preventDefault();
  const msg = $("#code-msg");
  const code = $("#code").value.trim();
  try {
    await busy(e.submitter, () => api("/setup/api/verify", { method: "POST", body: { code } }));
    state.code = code;
    note(msg, "", "");
    history.replaceState(null, "", location.pathname);
    go(1);
  } catch (err) {
    note(msg, "err", err.message);
  }
});

async function testDb() {
  const msg = $("#db-msg");
  note(msg, "info", "Connecting…");
  state.dbOk = false;
  $("#db-next").disabled = true;
  try {
    const r = await api("/setup/api/test-db", {
      method: "POST",
      body: { code: state.code, database_url: $("#db-url").value.trim() || null },
    });
    if (r.ok) {
      state.dbOk = true;
      $("#db-next").disabled = false;
      note(msg, "ok", `Connected: ${r.version.split(",")[0]}`);
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
});
$("#f-db").addEventListener("submit", (e) => {
  e.preventDefault();
  if (state.dbOk) go(2);
});

$("#tls-on").addEventListener("change", (e) => {
  $("#tls-fields").hidden = !e.currentTarget.checked;
});
$("#f-addr").addEventListener("submit", (e) => {
  e.preventDefault();
  const url = $("#public-url").value.trim();
  if ($("#tls-on").checked && !$("#tls-domain").value.trim()) {
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
  const tlsOn = $("#tls-on").checked;
  const body = {
    code: state.code,
    database_url: $("#db-url").value.trim() || null,
    public_url: $("#public-url").value.trim() || null,
    tls_domain: tlsOn ? $("#tls-domain").value.trim() : null,
    tls_email: tlsOn ? $("#tls-email").value.trim() || null : null,
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
  showDone(r, body);
});

async function showDone(r, body) {
  go(5);
  const url =
    body.public_url ||
    (body.tls_domain ? `https://${body.tls_domain}` : `${location.protocol}//${location.host}`);
  $("#server-url").textContent = url;
  if (r.master_key) {
    $("#key-box").hidden = false;
    $("#master-key").textContent = r.master_key;
    $("#copy-key").addEventListener("click", (e) => copy(r.master_key, e.currentTarget));
  }
  $("#dl-config").addEventListener("click", () => download("ourshell-config.toml", r.config_toml));
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
