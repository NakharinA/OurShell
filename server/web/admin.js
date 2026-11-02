// Admin page (see server/src/web/admin.rs).
"use strict";

const views = {
  overview: loadOverview,
  users: loadUsers,
  invites: loadInvites,
  settings: loadSettings,
  backups: loadBackups,
};

function showApp(signedIn) {
  $("#login").hidden = signedIn;
  $("#app").hidden = !signedIn;
  $("#logout").hidden = !signedIn;
  if (!signedIn) $("#login-pw").focus();
}

function tab(name) {
  for (const b of $$("[data-tab]")) {
    const on = b.dataset.tab === name;
    b.classList.toggle("on", on);
    b.setAttribute("aria-selected", on);
  }
  for (const v of $$("[data-view]")) v.hidden = v.dataset.view !== name;
  views[name]().catch(onError);
}
for (const b of $$("[data-tab]")) b.addEventListener("click", () => tab(b.dataset.tab));

function onError(err) {
  if (err.status === 401) showApp(false);
  else console.error(err);
}

async function loadOverview() {
  const s = await api("/admin/api/status");
  $("#version").textContent = `v${s.version}`;
  $("#s-users").textContent = s.users;
  $("#s-devices").textContent = s.devices;
  $("#s-items").textContent = s.items;
  serverUrl = s.public_url || location.origin;
  $("#o-url").textContent = serverUrl;
  $("#o-noowner").hidden = s.has_owner;
  $("#o-version").textContent = s.version;
  $("#o-db").textContent = `${s.database} (PostgreSQL ${s.database_version})`;
  $("#o-tls").textContent = s.tls_domain
    ? `Let's Encrypt for ${s.tls_domain}${s.tls_status ? ` (${s.tls_status})` : ""}`
    : "Off (plain HTTP)";
}

async function loadUsers() {
  const { users } = await api("/admin/api/users");
  const body = $("#users");
  body.replaceChildren();
  if (!users.length) {
    body.append(el("tr", {}, el("td", { colspan: 4, class: "empty" }, "No accounts yet.")));
    return;
  }
  for (const u of users) {
    const action = u.role === "Owner"
      ? null
      : el(
          "button",
          {
            type: "button",
            class: `small${u.disabled ? "" : " danger"}`,
            onclick: (e) => toggleUser(u, e.currentTarget),
          },
          u.disabled ? "Enable" : "Disable",
        );
    body.append(
      el(
        "tr",
        {},
        el("td", {}, u.email),
        el("td", {}, u.role),
        el("td", {}, el("span", { class: `pill ${u.disabled ? "err" : "ok"}` }, u.disabled ? "Disabled" : "Active")),
        el("td", {}, action),
      ),
    );
  }
}

async function toggleUser(u, button) {
  if (!u.disabled && !confirm(`Disable ${u.email}? They're signed out everywhere.`)) return;
  try {
    await busy(button, () =>
      api(`/admin/api/users/${encodeURIComponent(u.user_id)}/${u.disabled ? "enable" : "disable"}`, {
        method: "POST",
      }),
    );
    await loadUsers();
  } catch (err) {
    note($("#users-msg"), "err", err.message);
  }
}

/** The address clients use: the configured public URL, else this page's origin. */
let serverUrl = "";

async function inviteLink(code) {
  if (!serverUrl) {
    const s = await api("/admin/api/status");
    serverUrl = s.public_url || location.origin;
  }
  return `${serverUrl.replace(/\/+$/, "")}/invite#code=${encodeURIComponent(code)}`;
}

async function copyInviteLink(code, button) {
  copy(await inviteLink(code), button);
}

async function loadInvites() {
  const { invites } = await api("/admin/api/invites");
  const body = $("#invites");
  body.replaceChildren();
  if (!invites.length) {
    body.append(el("tr", {}, el("td", { colspan: 5, class: "empty" }, "No invites yet.")));
    return;
  }
  for (const i of invites) {
    const action = i.used_by
      ? null
      : el("button", { type: "button", class: "small", onclick: (e) => copyInviteLink(i.code, e.currentTarget) }, "Copy link");
    body.append(
      el(
        "tr",
        {},
        el("td", { class: "mono" }, i.code),
        el("td", {}, i.role),
        el("td", {}, new Date(i.created_at).toLocaleString()),
        el("td", {}, i.used_by ? i.used_by : el("span", { class: "pill ok" }, "Unused")),
        el("td", {}, action),
      ),
    );
  }
}

$("#invite-new").addEventListener("click", async (e) => {
  const msg = $("#invite-msg");
  try {
    const r = await busy(e.currentTarget, () =>
      api("/admin/api/invites", { method: "POST", body: { role: $("#invite-role").value } }),
    );
    const link = await inviteLink(r.code);
    msg.className = "note ok";
    msg.hidden = false;
    msg.replaceChildren(
      el("div", {}, "New invite: ", el("b", { class: "mono" }, r.code)),
      el("div", { class: "mono" }, link),
      el("button", { type: "button", class: "small", onclick: (ev) => copy(link, ev.currentTarget) }, "Copy link"),
    );
    copy(link, e.currentTarget);
    await loadInvites();
  } catch (err) {
    note(msg, "err", err.message);
  }
});

let envLocked = [];

async function loadSettings() {
  const { settings: s, env_locked } = await api("/admin/api/settings");
  envLocked = env_locked;
  const form = $("#settings");
  for (const input of $$("input", form)) {
    const v = s[input.name];
    if (input.type === "checkbox") input.checked = !!v;
    else if (input.name !== "oidc_client_secret") input.value = v ?? "";
  }
  form.elements.oidc_client_secret.value = "";
  $("#oidc-secret-hint").textContent = s.oidc_secret_set ? "Saved. Leave empty to keep it." : "";
  for (const wrap of $$("[data-key]", form)) {
    const key = wrap.dataset.key;
    const locked = env_locked.includes(key) || (key.startsWith("tls_") && env_locked.some((k) => k.startsWith("tls_")));
    for (const input of $$("input", wrap)) input.disabled = locked;
    let tag = $(".locked", wrap);
    if (locked && !tag) {
      tag = el("span", { class: "locked" }, " · set by environment");
      (wrap.querySelector("span") || wrap).append(tag);
    }
  }
  syncProxyFields();
}

/** Behind a proxy, the proxy owns HTTPS: the built-in certificate fields are off. */
function syncProxyFields() {
  const f = $("#settings").elements;
  const proxy = f.behind_proxy.checked;
  for (const name of ["tls_domain", "tls_email"]) {
    const locked = envLocked.some((k) => k.startsWith("tls_"));
    f[name].disabled = proxy || locked;
  }
  f.trusted_proxies.disabled = !proxy || envLocked.includes("trusted_proxies");
}
$("#settings").elements.behind_proxy.addEventListener("change", syncProxyFields);

$("#settings").addEventListener("submit", async (e) => {
  e.preventDefault();
  const f = e.currentTarget.elements;
  const num = (name) => Number(f[name].value || 0);
  const body = {
    public_url: f.public_url.value.trim() || null,
    signups_enabled: f.signups_enabled.checked,
    invite_only: f.invite_only.checked,
    max_devices_per_user: num("max_devices_per_user"),
    rate_limit_per_min: num("rate_limit_per_min"),
    session_ttl_hours: num("session_ttl_hours"),
    tls_domain: f.tls_domain.value.trim() || null,
    tls_email: f.tls_email.value.trim() || null,
    behind_proxy: f.behind_proxy.checked,
    trusted_proxies: f.trusted_proxies.value.trim() || null,
    oidc_issuer: f.oidc_issuer.value.trim() || null,
    oidc_client_id: f.oidc_client_id.value.trim() || null,
    oidc_client_secret: f.oidc_client_secret.value || null,
    oidc_redirect_url: f.oidc_redirect_url.value.trim() || null,
  };
  const msg = $("#settings-msg");
  try {
    await busy(e.submitter, () => api("/admin/api/settings", { method: "PUT", body }));
    note(msg, "info", "Saved. Reloading the server…");
    const ok = await waitForMode("ready");
    note(msg, ok ? "ok" : "warn", ok ? "Saved and applied." : "Saved, but the server hasn't come back yet. Check its log.");
    await loadSettings();
  } catch (err) {
    note(msg, "err", err.message);
  }
});

$("#password").addEventListener("submit", async (e) => {
  e.preventDefault();
  const f = e.currentTarget.elements;
  const msg = $("#password-msg");
  try {
    await busy(e.submitter, () =>
      api("/admin/api/password", { method: "PUT", body: { current: f.current.value, new: f.new.value } }),
    );
    note(msg, "ok", "Password changed. Sign in again.");
    setTimeout(() => showApp(false), 1500);
  } catch (err) {
    note(msg, "err", err.message);
  }
});

// ---- Backups ----

function fmtSize(n) {
  if (n === null || n === undefined) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

const fmtTime = (t) => (t ? new Date(t).toLocaleString() : "");

function showBackupStatus(st) {
  $("#b-enabled").textContent = st.running ? "On (a backup is running now)" : st.enabled ? "On" : "Off";
  $("#b-last").textContent = st.last_success_at
    ? `${fmtTime(st.last_success_at)} · ${fmtSize(st.last_size)} · ${st.last_key}`
    : "Never";
  const result = { uploaded: "uploaded", unchanged: "unchanged — skipped", failed: "failed" }[st.last_result] || "";
  $("#b-check").textContent = st.last_check_at ? `${fmtTime(st.last_check_at)}${result ? ` (${result})` : ""}` : "Never";
  $("#b-next").textContent = st.next_check ? fmtTime(st.next_check) : st.enabled ? "" : "Not scheduled";
  $("#b-error").textContent = st.last_error && st.last_result === "failed"
    ? `${fmtTime(st.last_error_at)}: ${st.last_error}`
    : "None";
}

async function loadBackups() {
  const { settings: s, env_locked, status, min_passphrase_len } = await api("/admin/api/backup");
  const form = $("#backup");
  for (const input of $$("input", form)) {
    const v = s[input.name];
    if (input.type === "checkbox") input.checked = !!v;
    else if (input.type !== "password") input.value = v ?? "";
    else input.value = "";
  }
  form.elements.passphrase.minLength = min_passphrase_len;
  $("#backup-secret-hint").textContent = s.secret_access_key_set ? "Saved. Leave empty to keep it." : "";
  $("#backup-pass-hint").textContent = s.passphrase_set
    ? "Saved. Leave empty to keep it."
    : `At least ${min_passphrase_len} characters.`;
  for (const wrap of $$("[data-key]", form)) {
    const locked = env_locked.includes(wrap.dataset.key);
    for (const input of $$("input", wrap)) input.disabled = locked;
    if (locked && !$(".locked", wrap)) {
      (wrap.querySelector("span") || wrap).append(el("span", { class: "locked" }, " · set by environment"));
    }
  }
  showBackupStatus(status);
  await loadBackupList();
}

async function loadBackupList() {
  const body = $("#backup-list");
  const msg = $("#backup-list-msg");
  body.replaceChildren();
  note(msg, "", "");
  let res;
  try {
    res = await api("/admin/api/backup/list");
  } catch (err) {
    if (err.code === "incomplete_backup_settings") {
      body.append(el("tr", {}, el("td", { colspan: 3, class: "empty" }, "Set up the storage above first.")));
    } else {
      note(msg, "err", err.message);
    }
    return;
  }
  if (!res.backups.length) {
    body.append(el("tr", {}, el("td", { colspan: 3, class: "empty" }, "No backups yet.")));
    return;
  }
  for (const b of res.backups) {
    body.append(
      el(
        "tr",
        {},
        el("td", { class: "mono" }, b.key),
        el("td", {}, fmtSize(b.size)),
        el("td", {}, fmtTime(b.last_modified)),
      ),
    );
  }
}

function backupForm() {
  const f = $("#backup").elements;
  const num = (name) => Number(f[name].value || 0);
  return {
    enabled: f.enabled.checked,
    endpoint: f.endpoint.value.trim() || null,
    region: f.region.value.trim() || null,
    bucket: f.bucket.value.trim() || null,
    prefix: f.prefix.value.trim() || null,
    access_key_id: f.access_key_id.value.trim() || null,
    secret_access_key: f.secret_access_key.value || null,
    path_style: f.path_style.checked,
    passphrase: f.passphrase.value || null,
    interval_hours: num("interval_hours"),
    keep: num("keep"),
  };
}

$("#backup-test").addEventListener("click", async (e) => {
  const msg = $("#backup-msg");
  try {
    const r = await busy(e.currentTarget, () => api("/admin/api/backup/test", { method: "POST", body: backupForm() }));
    note(msg, "ok", r.message);
  } catch (err) {
    note(msg, "err", err.message);
  }
});

$("#backup").addEventListener("submit", async (e) => {
  e.preventDefault();
  const msg = $("#backup-msg");
  try {
    await busy(e.submitter, () => api("/admin/api/backup", { method: "PUT", body: backupForm() }));
    note(msg, "info", "Saved. Reloading the server…");
    const ok = await waitForMode("ready");
    note(msg, ok ? "ok" : "warn", ok ? "Saved. The first check runs in about 2 minutes." : "Saved, but the server hasn't come back yet. Check its log.");
    await loadBackups();
  } catch (err) {
    note(msg, "err", err.message);
  }
});

$("#backup-run").addEventListener("click", async (e) => {
  const msg = $("#backup-run-msg");
  note(msg, "info", "Backing up…");
  try {
    const r = await busy(e.currentTarget, () => api("/admin/api/backup/run", { method: "POST" }));
    const o = r.outcome;
    note(
      msg,
      "ok",
      o.result === "uploaded"
        ? `Uploaded ${o.key} (${fmtSize(o.size)})${o.deleted.length ? `; removed ${o.deleted.length} old backup(s)` : ""}.`
        : "Nothing changed since the last backup.",
    );
    showBackupStatus(r.status);
    await loadBackupList();
  } catch (err) {
    note(msg, "err", err.message);
    api("/admin/api/backup").then((r) => showBackupStatus(r.status), () => {});
  }
});

$("#login").addEventListener("submit", async (e) => {
  e.preventDefault();
  const msg = $("#login-msg");
  try {
    await busy(e.submitter, () =>
      api("/admin/api/login", { method: "POST", body: { password: $("#login-pw").value } }),
    );
    $("#login-pw").value = "";
    note(msg, "", "");
    showApp(true);
    tab("overview");
  } catch (err) {
    note(msg, "err", err.message);
  }
});

$("#logout").addEventListener("click", async () => {
  try {
    await api("/admin/api/logout", { method: "POST" });
  } finally {
    showApp(false);
  }
});

(async () => {
  try {
    await loadOverview();
    showApp(true);
  } catch (err) {
    if (err.status === 401) showApp(false);
    else note($("#login-msg"), "err", err.message), showApp(false);
  }
})();
