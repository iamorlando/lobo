"""Launch a packaged order-book server or open the terminal of a chosen server."""

import argparse
import signal
import threading
import webbrowser
from importlib.metadata import PackageNotFoundError, version
from urllib.parse import urlsplit

from . import Book, server_context


def server_url(value: str) -> str:
    """Accept an explicit HTTP(S) server URL without shell interpretation."""
    parsed = urlsplit(value)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise argparse.ArgumentTypeError("use an http:// or https:// server URL")
    if parsed.username or parsed.password:
        raise argparse.ArgumentTypeError("server URLs must not contain credentials")
    return value


def open_web(url: str) -> bool:
    """Open this server's own terminal in the default system browser."""
    return webbrowser.open(server_url(url), new=2)


def main(argv: list[str] | None = None) -> int:
    """Run the console command installed with the Python wheel."""
    parser = argparse.ArgumentParser(
        prog="lobo", description="Start a local order-book server and web terminal."
    )
    try:
        package_version = version("pylobo")
    except PackageNotFoundError:
        package_version = "unknown (package metadata unavailable)"
    parser.add_argument("--version", action="version", version=package_version)
    commands = parser.add_subparsers(dest="command")
    serve = commands.add_parser(
        "serve", help="start the order API and bundled terminal"
    )
    serve.add_argument("--host", default="127.0.0.1")
    serve.add_argument("--port", type=int, default=8000)
    serve.add_argument("--book", action="append", default=[], metavar="SYMBOL")
    serve.add_argument("--price-decimals", type=int, default=0)
    serve.add_argument("--quantity-decimals", type=int, default=0)
    serve.add_argument(
        "--open", action="store_true", help="open the terminal in a browser"
    )
    web = commands.add_parser("web", help="open the terminal of an existing server")
    web.add_argument("--server", type=server_url, required=True)
    demo = commands.add_parser("demo", help="run the existing demo app locally")
    demo.add_argument("--port", type=int, default=0)
    demo.add_argument(
        "--no-open", action="store_true", help="print the URL without opening a browser"
    )
    args = parser.parse_args(argv)
    if args.command is None:
        parser.print_help()
        return 0
    if args.command == "web":
        print(args.server, flush=True)
        return 0 if open_web(args.server) else 1

    stopped = threading.Event()
    previous = {}
    for signum in (signal.SIGINT, signal.SIGTERM):
        previous[signum] = signal.signal(signum, lambda *_: stopped.set())
    try:
        if args.command == "demo":
            from .demo import demo_context

            with demo_context(port=args.port) as url:
                print(url, flush=True)
                print("Ctrl-C stops the local app.", flush=True)
                if not args.no_open:
                    open_web(url)
                stopped.wait()
            return 0
        with server_context(
            host=args.host,
            port=args.port,
            price_decimals=args.price_decimals,
            quantity_decimals=args.quantity_decimals,
        ) as server:
            for symbol in args.book or ["BOOK1"]:
                Book(symbol)
            print(server.url, flush=True)
            if args.open:
                open_web(server.url)
            stopped.wait()
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
    return 0
