import { describe, expect, it } from "vitest";
import { isDiscordId } from "./validate";

describe("isDiscordId", () => {
  it("17～20 位纯数字", () => {
    expect(isDiscordId("900000000000000000")).toBe(true);
    expect(isDiscordId("1234567890123456")).toBe(false);
    expect(isDiscordId("12345678901234567890a")).toBe(false);
  });
});
