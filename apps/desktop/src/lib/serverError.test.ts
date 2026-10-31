import { describe, expect, it } from "vitest";
import { serverErrorCode } from "./serverError";
import { AgentRpcError } from "./types";

describe("serverErrorCode", () => {
  it("reads the sync server's code from the RPC error data", () => {
    const err = new AgentRpcError({
      code: -32602,
      message: "sync server rejected the request (401 totp_required): a TOTP code is required",
      data: { server_status: 401, server_error: "totp_required" },
    });
    expect(serverErrorCode(err)).toBe("totp_required");
  });
  it("is null for other errors", () => {
    expect(serverErrorCode(new Error("boom"))).toBeNull();
    expect(serverErrorCode(new AgentRpcError({ code: -32001, message: "vault is locked" }))).toBeNull();
    expect(serverErrorCode(null)).toBeNull();
  });
});
