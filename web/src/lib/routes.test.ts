import { describe, expect, it } from "vitest";
import { href, parse } from "./routes";

describe("session routes", () => {
  it("round-trips a position in the session (`?at=<seq>`)", () => {
    const r = { name: "sessions", project: "p1", session: "s1", at: 42 } as const;
    expect(href(r)).toBe("#/p/p1/s/s1?at=42");
    expect(parse("#/p/p1/s/s1?at=42")).toEqual(r);
  });
  it("ignores a missing or bad position", () => {
    expect(parse("#/p/p1/s/s1")).toEqual({ name: "sessions", project: "p1", session: "s1" });
    expect(parse("#/p/p1/s/s1?at=x")).toEqual({ name: "sessions", project: "p1", session: "s1" });
  });
});

describe("tickets route", () => {
  it("round-trips", () => {
    expect(href({ name: "tickets", project: "p 1" })).toBe("#/p/p%201/tickets");
    expect(parse("#/p/p%201/tickets")).toEqual({ name: "tickets", project: "p 1" });
  });
});
