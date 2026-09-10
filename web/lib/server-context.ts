import type { AdapterInfo } from "./wasm";

export interface ServerConfiguration {
  mode: "server";
  name: string;
  metrics: number;
  adapters?: (AdapterInfo & {
    observer?: boolean;
    subscriptionsEndpoint?: string;
    books: ServerConfiguration["books"];
  })[];
  books: {
    symbol: string;
    price_decimals: number;
    quantity_decimals: number;
  }[];
}

/** Add subscriptions through the same adapter contract used by Python select(). */
export async function subscribeHostedAdapter(
  endpoint: string,
  selected: string,
  symbols: string[] = [],
  signal?: AbortSignal,
): Promise<void> {
  const response = await fetch(endpoint, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ selected, symbols }),
    signal,
  });
  if (!response.ok) {
    const message = await response.json().catch(() => null);
    throw new Error(
      message?.error ?? `Subscription failed (${response.status})`,
    );
  }
}
