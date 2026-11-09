import { describe, expect, it } from "vitest";
import { normalizeServerUrl } from "./server";

describe("normalizeServerUrl", () => {
  it.each([
    ["serenity", "ws://serenity:7747/ws"],
    ["  100.73.45.61 ", "ws://100.73.45.61:7747/ws"],
    ["serenity.tail0000.ts.net:9000", "ws://serenity.tail0000.ts.net:9000/ws"],
    ["[fd7a:115c::1]", "ws://[fd7a:115c::1]:7747/ws"],
    ["[fd7a:115c::1]:8080", "ws://[fd7a:115c::1]:8080/ws"],
    ["ws://box:7747/ws", "ws://box:7747/ws"],
    ["ws://box/", "ws://box:7747/ws"],
    ["wss://box.example:443/atelier", "wss://box.example/atelier"],
    ["http://box:7747", "ws://box:7747/ws"],
  ])("%s → %s", (input, url) => {
    expect(normalizeServerUrl(input)).toBe(url);
  });

  it.each(["", "   ", "fd7a::1", "user@box", "box:port", "ftp://box", "box:99999"])("rejects %j", (input) => {
    expect(normalizeServerUrl(input)).toBeNull();
  });
});
