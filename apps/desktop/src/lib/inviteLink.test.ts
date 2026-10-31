import { describe, expect, it } from "vitest";
import { parseInviteLink } from "./inviteLink";

describe("parseInviteLink", () => {
  it("splits an invite link into server and code", () => {
    expect(parseInviteLink(" https://ourshell-server.example.com/invite#code=ABCD2345EFGH ")).toEqual({
      server: "https://ourshell-server.example.com",
      code: "ABCD2345EFGH",
    });
    expect(parseInviteLink("http://192.168.1.10:8443/invite/#code=XYZ")).toEqual({
      server: "http://192.168.1.10:8443",
      code: "XYZ",
    });
  });
  it("ignores plain server URLs and other links", () => {
    expect(parseInviteLink("https://sync.example.com")).toBeNull();
    expect(parseInviteLink("https://sync.example.com/invite")).toBeNull();
    expect(parseInviteLink("https://sync.example.com/admin#code=X")).toBeNull();
    expect(parseInviteLink("not a url")).toBeNull();
    expect(parseInviteLink("ftp://sync.example.com/invite#code=X")).toBeNull();
  });
});
