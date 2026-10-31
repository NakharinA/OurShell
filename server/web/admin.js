// Admin page (see server/src/web/admin.rs).
"use strict";

const views = {
  overview: loadOverview,
  users: loadUsers,
  invites: loadInvites,
  settings: loadSettings,
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
