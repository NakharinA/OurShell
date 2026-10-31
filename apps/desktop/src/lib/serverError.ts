// The sync server's error code (e.g. "totp_required", "invite_required"),
// which the agent passes along in the JSON-RPC error's `data`
// (`sync_err` in crates/cli/src/agent/sync_ops.rs).

export function serverErrorCode(err: unknown): string | null {
  if (!err || typeof err !== "object" || !("data" in err)) return null;
  const data = (err as { data?: unknown }).data;
  if (!data || typeof data !== "object") return null;
  const code = (data as { server_error?: unknown }).server_error;
  return typeof code === "string" ? code : null;
}
