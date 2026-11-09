import { expect, test } from "vitest";
import { patchLines } from "./patch";

test("keeps hunks, drops headers", () => {
  const p = "diff --git a/a b/a\nindex 1..2\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n\\ No newline at end of file\n";
  expect(patchLines(p)).toEqual([
    { op: " ", text: "@@ -1,2 +1,2 @@" },
    { op: " ", text: "one" },
    { op: "-", text: "two" },
    { op: "+", text: "TWO" },
  ]);
});

test("empty and binary patches give no rows", () => {
  expect(patchLines("")).toEqual([]);
  expect(patchLines("diff --git a/x b/x\nBinary files a/x and b/x differ\n")).toEqual([]);
});
