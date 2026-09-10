"use client";
import { useEffect, useRef, useState } from "react";
import type { QueueStatus } from "@/lib/wasm";
import { useTheme } from "@/components/theme-provider";
import { chartTheme } from "@/lib/chart-theme";

const number = new Intl.NumberFormat("en-US", { maximumFractionDigits: 18 });
const arrival = (ns: string | null) => {
  if (!ns) return "Unknown arrival";
  const n = BigInt(ns);
  return `${new Date(Number(n / 1000000n)).toISOString().slice(11, 19)}.${(n % 1000000000n).toString().padStart(9, "0")}`;
};

export function LevelQueue({
  status,
  decimals,
  following,
  onFollow,
  onClose,
}: {
  status: QueueStatus;
  decimals: number;
  following: boolean;
  onFollow(): void;
  onClose(): void;
}) {
  const appearance = useTheme();
  const canvas = useRef<HTMLCanvasElement>(null);
  const [hover, setHover] = useState<string | null>(null);
  const hovered = status.orders.find((o) => o.id === hover);
  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const theme = chartTheme(element);
    const draw = () => {
      const rect = element.getBoundingClientRect();
      const dpr = Math.min(devicePixelRatio || 1, 2);
      element.width = Math.max(1, Math.round(rect.width * dpr));
      element.height = Math.round(48 * dpr);
      const ctx = element.getContext("2d")!;
      ctx.scale(dpr, dpr);
      ctx.fillStyle = theme.canvas;
      ctx.fillRect(0, 0, rect.width, 48);
      ctx.font = `10px ${theme.font}`;
      const width = rect.width;
      let left = 0;
      for (const order of status.orders) {
        const length = (order.quantity / status.totalQuantity) * width;
        // Stable shades identify orders across snapshots. Width is quantity.
        const hash = [...order.id].reduce(
          (h, c) => (h * 31 + c.charCodeAt(0)) | 0,
          0,
        );
        const bid = status.side === "buy";
        const gradient = ctx.createLinearGradient(0, 4, 0, 34);
        const low = bid ? theme.bidLow : theme.askLow;
        const high = bid ? theme.bidHigh : theme.askHigh;
        gradient.addColorStop(0, hash & 1 ? low : high);
        gradient.addColorStop(0.15 + ((hash >>> 0) % 18) / 25, low);
        gradient.addColorStop(1, hash & 1 ? high : low);
        ctx.fillStyle = order.mine ? theme.simulation : gradient;
        ctx.fillRect(left, 4, length, 30);
        if (length > 2) {
          ctx.strokeStyle = order.id === hover ? theme.text : theme.panel;
          ctx.strokeRect(left + 0.5, 4.5, length - 1, 29);
        }
        const label = `${order.mine ? "YOU · " : ""}${number.format(order.quantity)}`;
        if (ctx.measureText(label).width + 10 < length) {
          ctx.fillStyle = order.mine ? theme.simulationInk : theme.queueInk;
          ctx.fillText(label, left + 5, 23);
        }
        // A marker keeps a small resting order locatable without widening it.
        if (order.mine) {
          ctx.fillStyle = theme.simulation;
          ctx.beginPath();
          ctx.moveTo(left + length / 2, 34);
          ctx.lineTo(left + length / 2 - 4, 41);
          ctx.lineTo(left + length / 2 + 4, 41);
          ctx.fill();
        }
        left += length;
      }
      if (!status.orders.length) {
        ctx.fillStyle = theme.muted;
        ctx.fillText("No resting orders in this price range", 8, 23);
      }
    };
    draw();
    const observer = new ResizeObserver(draw);
    observer.observe(element);
    return () => observer.disconnect();
  }, [status, hover, appearance]);
  return (
    <section className="level-queue" aria-label="Level FIFO queue">
      <div className="queue-heading">
        <strong>LEVEL QUEUE</strong>
        <span>
          {status.side.toUpperCase()} ·{" "}
          <span className="queue-value">{status.lower.toFixed(decimals)}</span>
          {status.upper !== status.lower && (
            <>
              –
              <span className="queue-value">
                {status.upper.toFixed(decimals)}
              </span>
            </>
          )}{" "}
          ·{" "}
          <span className="queue-value">
            {number.format(status.orders.length)}
          </span>{" "}
          orders ·{" "}
          <span className="queue-value">
            {number.format(status.totalQuantity)}
          </span>{" "}
          quantity
        </span>
        {status.ordersAhead !== null && (
          <span className="queue-ahead">
            Ahead of you:{" "}
            <span className="queue-value">
              {number.format(status.ordersAhead)}
            </span>{" "}
            orders /{" "}
            <span className="queue-value">
              {number.format(status.quantityAhead!)}
            </span>
          </span>
        )}
        {following && (
          <button className="outline" onClick={onFollow}>
            Follow my order
          </button>
        )}
        <button
          className="outline"
          onClick={onClose}
          aria-label="Close level queue"
        >
          ×
        </button>
      </div>
      <canvas
        ref={canvas}
        role="img"
        aria-label={`${status.side} FIFO queue: ${status.orders.length} orders; ${status.ordersAhead === null ? "no simulated resting order" : `${status.ordersAhead} orders ahead of your order`}`}
        onPointerLeave={() => setHover(null)}
        onPointerMove={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          const target =
            ((event.clientX - rect.left) / rect.width) * status.totalQuantity;
          let quantity = 0;
          setHover(
            status.orders.find((o) => {
              quantity += o.quantity;
              return quantity >= target;
            })?.id ?? null,
          );
        }}
      />
      <div className="queue-caption">
        <span>
          {hovered ? (
            <>
              {hovered.mine ? "YOUR ORDER · " : ""}
              <span className="queue-value">{hovered.id}</span> ·{" "}
              <span className="queue-value">
                {number.format(hovered.quantity)}
              </span>{" "}
              @{" "}
              <span className="queue-value">
                {hovered.price.toFixed(decimals)}
              </span>{" "}
              · arrived{" "}
              <span className={hovered.timestampNs ? "queue-value" : undefined}>
                {arrival(hovered.timestampNs)}
              </span>
            </>
          ) : (
            "Front → back · width = quantity · highlight = your order · hover for ID and arrival time"
          )}
        </span>
        <span>
          {status.lower === status.upper
            ? "FIFO"
            : "Best price first · native FIFO within each level"}
        </span>
      </div>
    </section>
  );
}
