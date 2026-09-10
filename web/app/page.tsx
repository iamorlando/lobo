"use client";
import { useEffect, useState } from "react";
import ReplayLab from "@/components/replay-lab";
import type { ServerConfiguration } from "@/lib/server-context";
export default function Page() {
  const [configuration, setConfiguration] =
    useState<ServerConfiguration | null>();
  const [error, setError] = useState("");
  useEffect(() => {
    const abort = new AbortController();
    void (async () => {
      try {
        const response = await fetch("/api/server-context", {
          signal: abort.signal,
          cache: "no-store",
        });
        if (!response.ok)
          throw new Error("Could not discover the terminal's data source");
        const config = await response.json();
        setConfiguration(
          config.mode === "server" ? (config as ServerConfiguration) : null,
        );
      } catch (cause) {
        if (!abort.signal.aborted) setError(String(cause));
      }
    })();
    return () => abort.abort();
  }, []);
  if (configuration === undefined)
    return (
      <main className="terminal">
        <p role="status">{error || "Connecting to terminal…"}</p>
      </main>
    );
  return <ReplayLab server={configuration ?? undefined} />;
}
