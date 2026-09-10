"use client";
import { useState } from "react";
import type { AdapterInfo, SimulationStatus } from "@/lib/wasm";

const time = (ms: number) => new Date(ms).toISOString().slice(11, 23);
const number = new Intl.NumberFormat("en-US", { maximumFractionDigits: 18 });

export function SimulationPanel({
  level,
  mode,
  ready,
  bid,
  ask,
  decimals,
  quantityDecimals,
  note,
  status,
  ended,
  disabled = false,
  onOpen,
  onRun,
  onReturn,
}: {
  level: AdapterInfo["level"];
  mode: AdapterInfo["mode"];
  ready: boolean;
  bid: number;
  ask: number;
  decimals: number;
  quantityDecimals: number;
  note?: string;
  status: SimulationStatus | null;
  ended: boolean;
  disabled?: boolean;
  onOpen(): void;
  onRun(
    kind: "limit" | "market",
    side: string,
    price: number,
    quantity: number,
  ): void;
  onReturn(): void;
}) {
  const [open, setOpen] = useState(false);
  const [side, setSide] = useState("buy");
  const [kind, setKind] = useState<"limit" | "market">("limit");
  const orderKind = level === "l2" ? "market" : kind;
  const [price, setPrice] = useState("");
  const [quantity, setQuantity] = useState("");
  const [error, setError] = useState("");
  return (
    <section
      inert={disabled}
      className={`simulation-panel ${status?.alternateTimeline ? "simulation-active" : ""}`}
      aria-label="Order simulation"
    >
      {note && <p className="session-note">{note}</p>}
      <div className="simulation-heading">
        <strong>
          {status
            ? status.alternateTimeline
              ? "SIMULATED TIMELINE"
              : "MARKET PREVIEW"
            : "ORDER SIMULATION"}
        </strong>
        <span className="feed-capabilities">
          {level.toUpperCase()} · {mode.toUpperCase()}
        </span>
        {status ? (
          <>
            <span>
              {status.side.toUpperCase()} {number.format(status.requested)} ·{" "}
              {status.kind.toUpperCase()}
              {status.price !== null &&
                ` @ ${status.price.toFixed(decimals)}`}{" "}
              · {time(status.startedMs)}
            </span>
            <span role="status">
              {status.stoppedReason ??
                (status.complete
                  ? "Filled"
                  : !status.alternateTimeline
                    ? "Preview complete · unfilled remainder"
                    : ended
                      ? "Replay ended · unfilled remainder"
                      : "Resting")}
              {status.alternateTimeline &&
                status.complete &&
                " · timeline stopped"}
            </span>
            <button
              className="outline"
              onClick={() => {
                onReturn();
                setOpen(false);
              }}
            >
              {status.alternateTimeline
                ? "Return to main timeline"
                : "Close preview"}
            </button>
          </>
        ) : (
          <>
            <span>
              {level === "l2"
                ? "Market fills from current aggregate liquidity."
                : "Market preview or a limit order in a separate timeline."}
            </span>
            <button
              className="outline"
              disabled={!ready}
              onClick={() => {
                if (!open) {
                  onOpen();
                  setPrice(
                    (side === "buy" ? ask || bid : bid || ask).toFixed(
                      decimals,
                    ),
                  );
                  if (!quantity) setQuantity(quantityDecimals ? "1" : "1000");
                }
                setOpen(!open);
                setError("");
              }}
            >
              {open ? "Close" : "Simulate order"}
            </button>
          </>
        )}
      </div>
      {open && !status && (
        <form
          className="simulation-form"
          onSubmit={(event) => {
            event.preventDefault();
            try {
              onRun(orderKind, side, Number(price), Number(quantity));
              setError("");
            } catch (cause) {
              setError(String(cause));
            }
          }}
        >
          <label>
            Order
            <select
              aria-label="Simulation order type"
              value={orderKind}
              onChange={(event) =>
                setKind(event.target.value as "limit" | "market")
              }
            >
              <option value="market">Market</option>
              {level === "l3" && <option value="limit">Limit</option>}
            </select>
          </label>
          <label>
            Side
            <select
              aria-label="Simulation side"
              value={side}
              onChange={(event) => {
                setSide(event.target.value);
                setPrice(
                  (event.target.value === "buy"
                    ? ask || bid
                    : bid || ask
                  ).toFixed(decimals),
                );
              }}
            >
              <option value="buy">Buy</option>
              <option value="sell">Sell</option>
            </select>
          </label>
          {orderKind === "limit" && (
            <label>
              Limit price
              <input
                aria-label="Simulation limit price"
                type="number"
                required
                min={10 ** -decimals}
                step={10 ** -decimals}
                value={price}
                onChange={(event) => setPrice(event.target.value)}
              />
            </label>
          )}
          <label>
            Quantity
            <input
              aria-label="Simulation quantity"
              type="number"
              required
              min={10 ** -quantityDecimals}
              step={10 ** -quantityDecimals}
              value={quantity}
              onChange={(event) => setQuantity(event.target.value)}
            />
          </label>
          <button className="play-button" type="submit">
            {orderKind === "market"
              ? "Preview market order"
              : "Start simulation"}
          </button>
          <span>
            {orderKind === "market"
              ? "Reports current fills and unfilled quantity. The book stays unchanged."
              : mode === "replay"
                ? "FIFO at the limit; worse-price executions fill your resting order at its limit."
                : "Native matching in a separate live book until your order fills."}
          </span>
          {error && <span role="alert">{error}</span>}
        </form>
      )}
      {status && (
        <div className="simulation-results">
          <div className="simulation-summary">
            <span>
              SIMULATED FILLED <b>{number.format(status.filled)}</b>
            </span>
            <span>
              {status.alternateTimeline ? "REMAINING" : "UNFILLED"}{" "}
              <b>{number.format(status.remaining)}</b>
            </span>
            <span>
              AVG PRICE{" "}
              <b>
                {status.averagePrice === null
                  ? "—"
                  : status.averagePrice.toFixed(decimals)}
              </b>
            </span>
            <span>
              EXECUTIONS <b>{number.format(status.executionCount)}</b>
            </span>
            {status.ignored > 0 && (
              <span>
                {number.format(status.ignored)} missing branch references
                skipped
              </span>
            )}
            <small>
              {status.alternateTimeline
                ? "Simulated executions only. The main timeline continues separately."
                : "Snapshot of available liquidity at submission time. No alternate timeline."}
            </small>
          </div>
        </div>
      )}
    </section>
  );
}

export function SimulationResults({
  status,
  decimals,
}: {
  status: SimulationStatus;
  decimals: number;
}) {
  return (
    <section className="execution-panel" aria-label="Simulation fills">
      <div className="panel-heading">
        <h2>SIMULATED EXECUTIONS</h2>
        <span>
          {number.format(status.executionCount)} fills ·{" "}
          {status.side.toUpperCase()}
        </span>
      </div>
      <div
        className="simulation-tape"
        tabIndex={0}
        role="region"
        aria-label="Simulated executions"
      >
        <table>
          <thead>
            <tr>
              <th>Simulated execution</th>
              <th>Price</th>
              <th>Quantity</th>
              <th>Role</th>
            </tr>
          </thead>
          <tbody>
            {status.executions.map((fill) => (
              <tr key={fill.sequence}>
                <td>{time(fill.timeMs)}</td>
                <td>{fill.price.toFixed(decimals)}</td>
                <td>{number.format(fill.quantity)}</td>
                <td>{fill.maker ? "Maker" : "Taker"}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {!status.executions.length && (
          <p>
            {status.alternateTimeline && !status.complete
              ? "Waiting for simulated fills…"
              : "No available fills."}
          </p>
        )}
        {status.executionCount > 50 && (
          <small>Showing the latest 50 simulated executions.</small>
        )}
      </div>
    </section>
  );
}
