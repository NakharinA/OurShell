// Pure parsing for the Hosts search box's "Connect" button: decides whether
// the typed text names an existing saved host, or looks like
// `[user@]host[:port]` and should open the New Host modal prefilled. No RPC
// calls here - the caller (HostsView) does the actual connecting / opening.

export type QuickConnectResult =
  | { type: "empty" }
  | { type: "connect"; hostName: string }
  | { type: "new"; username: string | null; hostname: string; port: number | null }
  | { type: "none"; query: string };

/** `user@host:port`, `user@host`, `host:port`, or a plain host/IP. */
const ADDRESS_RE = /^(?:([^\s@]+)@)?([^\s@:]+)(?::(\d{1,6}))?$/;

/**
 * Decides what the quick-connect search box should do with `query`, given
 * the list of existing host names (any match wins over the address parse,
 * since a saved host named e.g. "web" should just connect rather than
 * re-opening the new-host form).
 */
export function parseQuickConnect(query: string, hostNames: string[]): QuickConnectResult {
  const trimmed = query.trim();
  if (!trimmed) return { type: "empty" };

  const existing = hostNames.find((h) => h.toLowerCase() === trimmed.toLowerCase());
  if (existing) return { type: "connect", hostName: existing };

  const m = ADDRESS_RE.exec(trimmed);
  if (m) {
    const [, user, host, portStr] = m;
    // A bare word with no '@', ':' or '.' (e.g. "foo") isn't a real address;
    // treat it as "no match" rather than offering to create "foo".
    const looksLikeAddress = trimmed.includes("@") || trimmed.includes(":") || trimmed.includes(".");
    if (looksLikeAddress) {
      const port = portStr ? Number(portStr) : null;
      return {
        type: "new",
        username: user ?? null,
        hostname: host,
        port: port && port > 0 && port <= 65535 ? port : null,
      };
    }
  }

  return { type: "none", query: trimmed };
}
