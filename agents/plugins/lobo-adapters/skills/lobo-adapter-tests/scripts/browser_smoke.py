"""Inspect the terminal bundled in an installed lobo wheel using Playwright.

Starts a consumer's server.py, or connects to an existing URL. Requires the
Python Playwright package and its Chromium browser (setup --testing).
"""

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import time
from pathlib import Path
from queue import Empty, Queue
from threading import Thread
from urllib.parse import urlsplit


class LocalServer:
    def __init__(self, script, arguments, timeout, output):
        self.script = Path(script).resolve()
        self.arguments = arguments
        self.timeout = timeout
        self.output = output
        self.process = None
        self.reader = None

    def start(self):
        lines = Queue()
        environment = dict(os.environ, PYTHONUNBUFFERED="1")
        environment.pop("PYTHONPATH", None)
        self.process = subprocess.Popen(
            [sys.executable, "-u", str(self.script), "--port", "0", *self.arguments],
            cwd=self.script.parent,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=os.name != "nt",
        )

        def collect():
            with (self.output / "server.log").open("w", encoding="utf-8") as log:
                for line in self.process.stdout:
                    log.write(line)
                    log.flush()
                    lines.put(line)

        self.reader = Thread(target=collect, daemon=True)
        self.reader.start()
        deadline = time.monotonic() + self.timeout
        while time.monotonic() < deadline:
            try:
                line = lines.get(timeout=0.2)
            except Empty:
                if self.process.poll() is not None:
                    raise RuntimeError(
                        "Server exited before printing its URL; see server.log"
                    )
                continue
            match = re.search(
                r"https?://(?:127\.0\.0\.1|localhost|\[::1\]):\d+\S*", line
            )
            if match:
                return match.group(0).rstrip(".,)")
        raise RuntimeError(
            "Server did not print a loopback URL before timeout; see server.log"
        )

    def stop(self):
        if self.process is not None:
            if self.process.poll() is None:
                if os.name == "nt":
                    self.process.terminate()
                else:
                    os.killpg(self.process.pid, signal.SIGINT)
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    if os.name == "nt":
                        self.process.kill()
                    else:
                        os.killpg(self.process.pid, signal.SIGKILL)
                    self.process.wait(timeout=5)
            if self.reader is not None:
                self.reader.join(timeout=2)
            self.process.stdout.close()


def inspect_terminal(args, url, report):
    from playwright.sync_api import expect, sync_playwright

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(
            headless=not args.headed,
            args=["--enable-unsafe-webgpu", "--ignore-gpu-blocklist"],
        )
        page = browser.new_page(viewport={"width": 1440, "height": 1000})
        page.set_default_timeout(args.timeout * 1000)
        report["page_errors"] = []
        report["console_errors"] = []
        page.on("pageerror", lambda error: report["page_errors"].append(str(error)))
        page.on(
            "console",
            lambda message: (
                report["console_errors"].append(message.text)
                if message.type == "error"
                else None
            ),
        )
        try:
            response = page.goto(url, wait_until="domcontentloaded")
            if response is None or not response.ok:
                raise RuntimeError("Terminal URL did not return a successful response")
            report["webgpu"] = page.evaluate("""async () => {
                if (!navigator.gpu) return false;
                return !!(await navigator.gpu.requestAdapter());
            }""")
            if not report["webgpu"]:
                raise RuntimeError(
                    "Chromium has no WebGPU adapter; use a capable browser environment"
                )
            ticker = page.locator("#ticker")
            expect(ticker).to_be_enabled()
            page.wait_for_function(
                """minimum => {
                const text = document.querySelector('#ticker-help')?.textContent || '';
                const count = Number(text.split(' symbols')[0].replace(/[^0-9]/g, ''));
                return count >= minimum;
            }""",
                arg=args.min_symbols,
            )
            report["directory"] = page.locator("#ticker-help").inner_text()
            report["initial_symbol"] = ticker.input_value()
            expect(page.locator(".gpu-canvas")).to_be_visible()
            expect(page.locator(".quote-panel")).to_be_visible()
            if args.switch_symbol or args.symbol:
                ticker.fill(args.symbol or "")
                options = page.locator('#ticker-options [role="option"] strong')
                expect(options.first).to_be_visible()
                available = options.all_text_contents()
                selected = args.symbol or next(
                    (s for s in available if s != report["initial_symbol"]), None
                )
                if selected is None or selected == report["initial_symbol"]:
                    raise RuntimeError(
                        "Ticker switching requires a second discovered symbol"
                    )
                if selected not in available:
                    raise RuntimeError(
                        f"Requested symbol {selected!r} was not offered by autocomplete"
                    )
                option = page.locator('#ticker-options [role="option"]').filter(
                    has=page.locator(
                        "strong", has_text=re.compile("^" + re.escape(selected) + "$")
                    )
                )
                with page.expect_response(
                    lambda r: (
                        r.request.method == "POST"
                        and "/subscriptions" in r.url
                        and r.request.post_data_json.get("selected") == selected
                    )
                ) as selection:
                    option.click()
                if not selection.value.ok:
                    raise RuntimeError(
                        f"Hosted adapter rejected selection: HTTP {selection.value.status}"
                    )
                expect(ticker).to_have_value(selected)
                expect(ticker).to_be_enabled()
                expect(page.locator("#ticker-options")).to_have_count(0)
                report["selected_symbol"] = selected
            report["quotes"] = page.locator(".quote-panel").inner_text()
            if args.require_quotes:
                for side in ("bid", "ask"):
                    expect(page.locator(f".{side}-quote strong")).to_have_text(
                        re.compile(r"[0-9]")
                    )
                report["quotes"] = page.locator(".quote-panel").inner_text()
            if report["page_errors"]:
                raise RuntimeError("Terminal raised JavaScript errors; see report.json")
        finally:
            try:
                page.screenshot(
                    path=str(Path(args.out) / "terminal.png"), full_page=True
                )
            finally:
                browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument(
        "--server", help="Consumer server.py accepting --port 0 and printing server.url"
    )
    target.add_argument(
        "--url", help="Existing loopback server; the runner will not stop it"
    )
    parser.add_argument(
        "--server-arg",
        action="append",
        default=[],
        help="Extra argument passed to server.py (repeatable; use = for flags)",
    )
    parser.add_argument("--switch-symbol", action="store_true")
    parser.add_argument("--symbol", help="Actual discovered symbol to select")
    parser.add_argument("--min-symbols", type=int, default=1)
    parser.add_argument(
        "--require-quotes",
        action="store_true",
        help="Require numeric best bid and ask for a fixture or known non-empty market",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=60,
        help="Startup and per-browser-step timeout in seconds",
    )
    parser.add_argument("--headed", action="store_true")
    parser.add_argument("--out", default=".lobo-test-artifacts")
    args = parser.parse_args()
    if args.timeout <= 0 or args.min_symbols < 1:
        parser.error("timeout and min-symbols must be positive")
    if args.url and (
        urlsplit(args.url).hostname not in {"127.0.0.1", "localhost", "::1"}
        or urlsplit(args.url).scheme not in {"http", "https"}
    ):
        parser.error("Use a loopback HTTP(S) server URL")
    output = Path(args.out).resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {"passed": False, "python": sys.executable}
    server = (
        LocalServer(args.server, args.server_arg, args.timeout, output)
        if args.server
        else None
    )
    try:
        url = server.start() if server else args.url
        report["url"] = url
        inspect_terminal(args, url, report)
        report["passed"] = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        if server:
            server.stop()
        (output / "report.json").write_text(
            json.dumps(report, indent=2) + "\n", encoding="utf-8"
        )
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
