// Public invite page: /invite#code=XXXX. The code stays in the URL
// fragment, so it never reaches the server or a proxy's access log.
"use strict";

(() => {
  const code = (new URLSearchParams(location.hash.slice(1)).get("code") || "").trim();
  const server = location.origin;
  if (!code) {
    $("#no-code").hidden = false;
    return;
  }
  $("#invite").hidden = false;
  $("#server").textContent = server;
  $("#code").textContent = code;
  const link = `${server}/invite#code=${encodeURIComponent(code)}`;
  $("#copy-server").addEventListener("click", (e) => copy(server, e.currentTarget));
  $("#copy-code").addEventListener("click", (e) => copy(code, e.currentTarget));
  $("#copy-link").addEventListener("click", (e) => copy(link, e.currentTarget));
})();
