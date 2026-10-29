import { describe, expect, it } from "vitest";
import { get } from "svelte/store";
import { answerConfirm, confirmDialog, confirmRequest } from "./confirm";

describe("confirmDialog", () => {
  it("waits for the user's answer", async () => {
    let settled = false;
    const p = confirmDialog({ title: "Remove x?" }).then((v) => {
      settled = true;
      return v;
    });
    await Promise.resolve();
    expect(settled).toBe(false);
    expect(get(confirmRequest)?.title).toBe("Remove x?");
    answerConfirm(true);
    expect(await p).toBe(true);
    expect(get(confirmRequest)).toBeNull();
  });

  it("resolves false on cancel", async () => {
    const p = confirmDialog({ title: "Remove y?" });
    answerConfirm(false);
    expect(await p).toBe(false);
  });

  it("a newer request cancels an unanswered one", async () => {
    const first = confirmDialog({ title: "a" });
    const second = confirmDialog({ title: "b" });
    expect(await first).toBe(false);
    answerConfirm(true);
    expect(await second).toBe(true);
  });
});
