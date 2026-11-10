import { describe, expect, it } from "vitest";
import type { SessionRow } from "../gen/SessionRow";
import { prefillNewSession, readersOf, reviewPrompt, shortCommit, takePrefill } from "./review";

const row = (id: string, workspace: object, archived = false) =>
  ({ id, title: id, archived, workspace }) as unknown as SessionRow;

describe("reviewPrompt", () => {
  it("names the base, branch and session, and asks for a read-only review", () => {
    const p = reviewPrompt({ title: "Add login", base: "abc123", branch: "atelier/add-login-1" });
    expect(p).toContain("(base `abc123`, branch `atelier/add-login-1`)");
    expect(p).toContain('made by session "Add login"');
    expect(p).toContain("`git diff abc123`");
    expect(p.endsWith("Don't edit files.")).toBe(true);
  });
});

describe("readersOf", () => {
  it("finds live shared sessions of the owner, not archived ones or other owners", () => {
    const all = [
      row("r1", { mode: "shared", from: "x" }),
      row("r2", { mode: "shared", from: "x" }, true),
      row("r3", { mode: "shared", from: "y" }),
      row("b1", { mode: "worktree", from: "x" }),
      row("x", { mode: "worktree" }),
    ];
    expect(readersOf(all, "x").map((s) => s.id)).toEqual(["r1"]);
  });
});

describe("prefill handoff", () => {
  it("is taken once", () => {
    prefillNewSession({ source: "share", from: "x", title: "Review: x", prompt: "p" });
    expect(takePrefill()?.from).toBe("x");
    expect(takePrefill()).toBeNull();
  });
});

describe("shortCommit", () => {
  it("is seven characters", () => {
    expect(shortCommit("0123456789abcdef")).toBe("0123456");
  });
});
