// Pure decision logic for the host modal's "identity behind the scenes"
// feature: the UI never shows the word "Identity", it just has
// Username/Password/Key fields on the host form. On save, we decide whether
// those fields need a backing `Identity` item (created or updated, reusing
// the host's existing linked identity when editing) or whether the link
// should be cleared. The actual `item.put` / `item.delete` RPC calls happen
// in the component; this module only computes *what* to do so it can be
// unit tested without mocking the agent.

import type { Identity } from "./types";

export interface IdentityInput {
  hostName: string;
  username: string | null;
  password: string | null;
  keyId: string | null;
  /** The host's current `identity_id`, if editing an existing host. */
  existingIdentityId: string | null;
}

export type IdentityPlan =
  | { action: "none" }
  | { action: "put"; id: string | null; identity: Identity }
  | { action: "clear"; deleteId: string | null };

/**
 * What the agent sends instead of a stored secret (`redact` in
 * crates/cli/src/agent/items.rs). Saved passwords never reach the UI;
 * sending this back in `item.put` tells the agent to keep the stored value.
 */
export const REDACTED = "********";

/**
 * The password field's value to save: a newly typed password replaces the
 * stored one; an empty field keeps a saved password (as {@link REDACTED})
 * unless the user removed it.
 */
export function passwordToSave(typed: string, saved: boolean): string | null {
  if (typed) return typed;
  return saved ? REDACTED : null;
}

function nonEmpty(s: string | null): string | null {
  const v = s?.trim();
  return v ? v : null;
}

/**
 * Decides how to persist the host form's credential fields as an `Identity`.
 *
 * - No password and no key: the host needs no identity. If one was linked
 *   before, it's cleared (and reported as the id to delete, so no orphaned
 *   identity items pile up invisibly).
 * - Password and/or key set: upsert an identity named after the host,
 *   reusing `existingIdentityId` so editing a host never creates duplicates.
 */
export function planIdentityUpdate(input: IdentityInput): IdentityPlan {
  const password = nonEmpty(input.password);
  const keyId = nonEmpty(input.keyId);

  if (!password && !keyId) {
    return input.existingIdentityId
      ? { action: "clear", deleteId: input.existingIdentityId }
      : { action: "none" };
  }

  return {
    action: "put",
    id: input.existingIdentityId,
    identity: {
      name: input.hostName,
      username: nonEmpty(input.username),
      password,
      key_id: keyId,
    },
  };
}
