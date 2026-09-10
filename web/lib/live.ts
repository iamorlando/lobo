import type { Session } from "./wasm";

/** Socket transport only. Protocol commands, snapshots and checksums live in Rust. */
function socketTransport(
  endpoint: string,
  session: Pick<
    Session,
    "commands" | "connected" | "disconnected" | "append" | "keepalive"
  >,
  signal: AbortSignal,
) {
  let socket: WebSocket | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;
  let delay = 1000;
  let received = Date.now();
  let state = "Connecting";
  let detail = "";
  function flush() {
    if (socket?.readyState === WebSocket.OPEN) {
      for (const command of session.commands()) socket.send(command);
    }
  }
  function connect() {
    if (stopped) return;
    const url = new URL(endpoint, window.location.href);
    if (url.protocol === "http:") url.protocol = "ws:";
    if (url.protocol === "https:") url.protocol = "wss:";
    socket = new WebSocket(url);
    received = Date.now();
    socket.onopen = () => {
      if (stopped) return;
      try {
        session.connected();
        flush();
        state = "Connected";
        detail = "";
      } catch (error) {
        detail = String(error);
        socket?.close();
      }
    };
    socket.onmessage = (event) => {
      if (stopped) return;
      try {
        if (typeof event.data !== "string")
          throw new Error("Expected a text WebSocket message");
        received = Date.now();
        session.append(new TextEncoder().encode(event.data), false);
        flush();
        delay = 1000;
      } catch (error) {
        detail = error instanceof Error ? error.message : String(error);
        session.disconnected();
        socket?.close();
      }
    };
    socket.onerror = () => {
      if (!detail) detail = "Connection interrupted";
    };
    socket.onclose = () => {
      if (stopped) return;
      session.disconnected();
      state = "Reconnecting";
      retry = setTimeout(connect, delay);
      delay = Math.min(delay * 2, 30000);
    };
  }
  const heartbeat = setInterval(() => {
    if (stopped || socket?.readyState !== WebSocket.OPEN) return;
    if (Date.now() - received > 30000) {
      detail = "Feed timed out";
      socket.close();
      return;
    }
    session.keepalive();
    flush();
  }, 15000);
  const close = () => {
    if (stopped) return;
    stopped = true;
    clearTimeout(retry);
    clearInterval(heartbeat);
    socket?.close();
    signal.removeEventListener("abort", close);
  };
  signal.addEventListener("abort", close, { once: true });
  if (signal.aborted) close();
  else connect();
  function reconnect() {
    if (stopped) return;
    clearTimeout(retry);
    // Detach the old socket so its delayed callbacks cannot affect the new one.
    if (socket) {
      socket.onopen = socket.onmessage = socket.onerror = socket.onclose = null;
      socket.close();
    }
    session.disconnected();
    state = "Reconnecting";
    delay = 1000;
    connect();
  }
  return { flush, close, reconnect, status: () => state, detail: () => detail };
}

interface ConnectionSpec {
  id: number;
  endpoint: string;
  selected: boolean;
}

/** The adapter decides how to partition subscriptions; transports only carry bytes. */
export function connectSocket(session: Session, signal: AbortSignal) {
  const sockets = new Map<number, ReturnType<typeof socketTransport>>();
  let stopped = signal.aborted;
  const specs = () => JSON.parse(session.connections()) as ConnectionSpec[];
  function flush() {
    if (stopped) return;
    for (const spec of specs()) {
      let socket = sockets.get(spec.id);
      if (!socket) {
        const id = spec.id;
        socket = socketTransport(
          spec.endpoint,
          {
            connected: () => session.socket_connected(id),
            disconnected: () => session.socket_disconnected(id),
            commands: () => session.socket_commands(id),
            append: (bytes) => session.socket_receive(id, bytes),
            keepalive: () => session.socket_keepalive(id),
          },
          signal,
        );
        sockets.set(id, socket);
      }
      socket.flush();
    }
  }
  const selected = () => sockets.get(specs().find((s) => s.selected)?.id ?? 0);
  function close() {
    if (stopped) return;
    stopped = true;
    for (const socket of sockets.values()) socket.close();
    sockets.clear();
    signal.removeEventListener("abort", close);
  }
  signal.addEventListener("abort", close, { once: true });
  flush();
  return {
    flush,
    close,
    reconnect() {
      if (!stopped) for (const socket of sockets.values()) socket.reconnect();
    },
    status: () => (stopped ? "Closed" : (selected()?.status() ?? "Connecting")),
    detail: () => (stopped ? "" : (selected()?.detail() ?? "")),
  };
}

/** HTTP protocol data stays opaque until it reaches the native adapter. */
export async function bootstrapFeed(session: Session, signal: AbortSignal) {
  const requests = JSON.parse(session.bootstrap_requests()) as {
    id: string;
    url: string;
  }[];
  for (const request of requests) {
    const response = await fetch(
      `/api/market-data?url=${encodeURIComponent(request.url)}`,
      { signal },
    );
    if (!response.ok) throw new Error(await response.text());
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (signal.aborted) return;
    session.bootstrap(request.id, bytes);
  }
}
