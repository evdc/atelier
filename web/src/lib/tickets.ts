import { createSignal } from "solid-js";

/** How many tickets the open board has, for the sidebar. The board view owns the watch; null when none is open. */
export const [ticketCount, setTicketCount] = createSignal<number | null>(null);
