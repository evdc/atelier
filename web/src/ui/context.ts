import { createContext, useContext } from "solid-js";
import type { ProjectRow } from "../gen/ProjectRow";
import type { SessionRow } from "../gen/SessionRow";
import type { UsageRow } from "../gen/UsageRow";
import type { Conn } from "../lib/conn";
import type { View } from "../lib/view";

export interface AppCtx {
  conn: Conn;
  projects: View<ProjectRow>;
  /** Every session across projects (for "waiting on you"). */
  sessions: View<SessionRow>;
  /** Subscription limits; empty until an agent has reported a window. */
  usage: View<UsageRow>;
}

export const Ctx = createContext<AppCtx>();

export function useApp(): AppCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error("useApp outside <App>");
  return c;
}
