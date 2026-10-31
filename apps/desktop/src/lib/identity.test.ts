import { describe, expect, it } from "vitest";
import { REDACTED, passwordToSave, planIdentityUpdate } from "./identity";

describe("passwordToSave", () => {
  it("replaces a saved password with a newly typed one", () => {
    expect(passwordToSave("new-secret", true)).toBe("new-secret");
  });
  it("keeps a saved password the user didn't touch, without ever knowing it", () => {
    expect(passwordToSave("", true)).toBe(REDACTED);
  });
  it("saves no password when none was saved or the saved one was removed", () => {
    expect(passwordToSave("", false)).toBeNull();
  });
  it("keeping a saved password still keeps the identity", () => {
    expect(
      planIdentityUpdate({
        hostName: "web",
        username: "root",
        password: passwordToSave("", true),
        keyId: null,
        existingIdentityId: "id-1",
      }),
    ).toMatchObject({ action: "put", id: "id-1", identity: { password: REDACTED } });
  });
});

describe("planIdentityUpdate", () => {
  it("does nothing when there are no credentials and nothing was linked", () => {
    expect(
      planIdentityUpdate({
        hostName: "web",
        username: null,
        password: null,
        keyId: null,
        existingIdentityId: null,
      }),
    ).toEqual({ action: "none" });
  });

  it("clears and deletes the old identity when credentials are removed", () => {
    expect(
      planIdentityUpdate({
        hostName: "web",
        username: "root",
        password: null,
        keyId: null,
        existingIdentityId: "id-1",
      }),
    ).toEqual({ action: "clear", deleteId: "id-1" });
  });

  it("creates a new identity named after the host when a password is set", () => {
    expect(
      planIdentityUpdate({
        hostName: "prod-web",
        username: "root",
        password: "secret",
        keyId: null,
        existingIdentityId: null,
      }),
    ).toEqual({
      action: "put",
      id: null,
      identity: { name: "prod-web", username: "root", password: "secret", key_id: null },
    });
  });

  it("reuses the existing identity id when editing a host", () => {
    const plan = planIdentityUpdate({
      hostName: "prod-web",
      username: "root",
      password: null,
      keyId: "key-9",
      existingIdentityId: "id-5",
    });
    expect(plan).toEqual({
      action: "put",
      id: "id-5",
      identity: { name: "prod-web", username: "root", password: null, key_id: "key-9" },
    });
  });

  it("treats blank strings as empty", () => {
    expect(
      planIdentityUpdate({
        hostName: "web",
        username: "  ",
        password: "   ",
        keyId: "  ",
        existingIdentityId: null,
      }),
    ).toEqual({ action: "none" });
  });
});
