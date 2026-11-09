import { expect, test } from "vitest";
import { isLoopback } from "./openin";

test("loopback urls", () => {
  for (const u of ["ws://127.0.0.1:7878/ws", "ws://localhost:1/ws", "ws://[::1]:1/ws"]) expect(isLoopback(u)).toBe(true);
  for (const u of ["wss://example.com/ws", "ws://192.168.1.4:1/ws", "ws://127.0.0.1.evil.com/ws", "nonsense"]) expect(isLoopback(u)).toBe(false);
});
