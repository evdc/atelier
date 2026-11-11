import { describe, expect, it } from "vitest";
import { diffCounts, diffLines, exitCode, highlightLine, sendSummary, spawnedId, tail, toolStat } from "./tools";

const show = (a: string, b: string) => diffLines(a, b).map((l) => l.op + l.text);

describe("diffLines", () => {
  it("insert", () => expect(show("a\nc", "a\nb\nc")).toEqual([" a", "+b", " c"]));
  it("delete", () => expect(show("a\nb\nc", "a\nc")).toEqual([" a", "-b", " c"]));
  it("replace", () => expect(show("a\nb\nc", "a\nB\nc")).toEqual([" a", "-b", "+B", " c"]));
  it("replace_all counts the strings as given", () => {
    expect(diffCounts(diffLines("foo", "bar"))).toEqual({ add: 1, del: 1 });
  });
  it("empty old string is a pure insert", () => expect(show("", "x\ny")).toEqual(["+x", "+y"]));
  it("interleaved changes", () => expect(show("a\nb\nc\nd", "a\nX\nc\nY")).toEqual([" a", "-b", "+X", " c", "-d", "+Y"]));
  it("huge middles fall back without a table", () => {
    const a = Array.from({ length: 800 }, (_, i) => `a${i}`).join("\n");
    const b = Array.from({ length: 800 }, (_, i) => `b${i}`).join("\n");
    expect(diffCounts(diffLines(a, b))).toEqual({ add: 800, del: 800 });
  });
});

describe("toolStat", () => {
  it("edit", () =>
    expect(toolStat("Edit", { old_string: "a", new_string: "a\nb\nc" }, undefined)).toBe("+2 −0"));
  it("edit replace_all", () =>
    expect(toolStat("Edit", { old_string: "a", new_string: "b", replace_all: true }, undefined)).toBe("+1 −1 all"));
  it("write", () => expect(toolStat("Write", { content: "a\nb\n" }, undefined)).toBe("2 lines"));
  it("read range", () => expect(toolStat("Read", { offset: 10, limit: 5 }, undefined)).toBe("10–14"));
  it("read whole", () => expect(toolStat("Read", {}, undefined)).toBe(""));
  it("grep", () => expect(toolStat("Grep", {}, { text: "a.ts\nb.ts\n", error: false })).toBe("2 matches"));
  it("glob none", () => expect(toolStat("Glob", {}, { text: "No files found", error: false })).toBe("0 files"));
  it("bash error", () => expect(toolStat("Bash", {}, { text: "Exit code 2\nboom", error: true })).toBe("exit 2"));
  it("bash ok", () => expect(toolStat("Bash", {}, { text: "hi", error: false })).toBe(""));
  it("todos", () =>
    expect(toolStat("TodoWrite", { todos: [{ content: "a", status: "completed" }, { content: "b", status: "pending" }] }, undefined)).toBe("1/2"));
});

it("exitCode", () => expect(exitCode("Exit code 127\nx")).toBe(127));
it("tail", () => {
  expect(tail("1\n2\n3", 2)).toEqual({ text: "2\n3", dropped: 1 });
  expect(tail("1\n2", 2).dropped).toBe(0);
});
it("highlightLine", () => {
  const t = highlightLine('const x = foo("hi"); // note');
  expect(t.filter((k) => k.kind).map((k) => [k.kind, k.text])).toEqual([
    ["kw", "const"],
    ["fn", "foo"],
    ["str", '"hi"'],
    ["com", "// note"],
  ]);
  expect(t.map((k) => k.text).join("")).toBe('const x = foo("hi"); // note');
});

describe("spawnedId", () => {
  const ok = { text: "started 01a1-b2 'Write it' in worktree atelier/x", error: false };
  it("reads the child's id from a spawn result", () => expect(spawnedId("mcp__atelier__spawn", ok)).toBe("01a1-b2"));
  it("ignores other tools, pending calls and refusals", () => {
    expect(spawnedId("Bash", ok)).toBeNull();
    expect(spawnedId("mcp__atelier__spawn", undefined)).toBeNull();
    expect(spawnedId("mcp__atelier__spawn", { text: "spawn refused: …", error: true })).toBeNull();
    expect(spawnedId("mcp__atelier__spawn", { text: "spawn refused: …", error: false })).toBeNull();
  });
});

describe("sendSummary", () => {
  it("names the target as typed and the text's first line", () => {
    expect(sendSummary("mcp__atelier__send", { to: "7f3a", text: "\n  Check the WAL\nmore" })).toEqual({
      to: "7f3a",
      line: "Check the WAL",
    });
  });
  it("is only for send", () => {
    expect(sendSummary("mcp__atelier__read", { to: "7f3a", text: "x" })).toBeNull();
    expect(sendSummary("mcp__atelier__send", {})).toBeNull();
  });
});
