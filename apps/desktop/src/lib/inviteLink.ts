// Invite links from a server's /admin page look like
// `https://sync.example.com/invite#code=ABCD...`. Pasting one into the
// Server URL field fills in both the server and the invite code.

export interface InviteLink {
  server: string;
  code: string;
}

export function parseInviteLink(text: string): InviteLink | null {
  let url: URL;
  try {
    url = new URL(text.trim());
  } catch {
    return null;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  if (!url.pathname.replace(/\/+$/, "").endsWith("/invite")) return null;
  const code = new URLSearchParams(url.hash.slice(1)).get("code")?.trim();
  if (!code) return null;
  // The server lives wherever /invite is mounted (normally the root).
  const base = url.pathname.replace(/\/+$/, "").slice(0, -"/invite".length);
  return { server: `${url.origin}${base}`, code };
}
