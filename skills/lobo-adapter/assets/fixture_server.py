"""Local synthetic feed for testing the shipped browser runner with a wheel.

Copy this file to a consumer project; it has no checkout-relative imports.
Subscriptions are recorded so browser checks can verify lazy book loading.
"""

import argparse
import json
import time
from pathlib import Path
from threading import Event, Lock, Thread

from lobo import server_context
from lobo.replay.adapters import CustomAdapter, Protocol
from lobo.replay.adapters import expressions as le
from lobo.replay.adapters import models as lm
from websockets.exceptions import ConnectionClosed
from websockets.sync.server import serve


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    lock = Lock()

    def stream(socket):
        try:
            symbol = json.loads(socket.recv(timeout=10))["symbol"]
            with lock, Path("subscriptions.jsonl").open("a") as log:
                log.write(json.dumps(symbol) + "\n")
            while True:
                socket.send(json.dumps({"symbol": symbol, "time": time.time_ns()}))
                time.sleep(0.25)
        except ConnectionClosed:
            pass

    protocol = Protocol(
        lm.Json(
            lm.Message(
                True,
                lm.Book(
                    le.Field("symbol"),
                    lm.Level(side="sell", price=60, quantity=3),
                    lm.Level(side="buy", price=59, quantity=2),
                    snapshot=True,
                    timestamp=le.Field("time"),
                ),
            )
        ),
        connect=(lm.Subscribe(),),
        subscriptions=(lm.Send({"symbol": le.Variable("symbol")}),),
        symbols_per_connection=1,
    )
    with serve(stream, "127.0.0.1", 0) as upstream:
        thread = Thread(target=upstream.serve_forever, daemon=True)
        thread.start()
        try:
            adapter = CustomAdapter(
                protocol,
                lm.Source.websocket(
                    f"ws://127.0.0.1:{upstream.socket.getsockname()[1]}"
                ),
                instruments=[lm.Instrument(s, 2, 0) for s in ("AAA", "BBB", "CCC")],
                symbol="AAA",
                scope=None,
                level=lm.BookLevel.L2,
            )
            with server_context(adapters=[adapter], port=args.port) as server:
                print(server.url, flush=True)
                try:
                    Event().wait()
                except KeyboardInterrupt:
                    pass
        finally:
            upstream.shutdown()
            thread.join(timeout=5)


if __name__ == "__main__":
    main()
