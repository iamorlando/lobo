"""Console help works in both installed packages and development checkouts."""

# ruff: noqa: D103, S101

from contextlib import nullcontext
from importlib.metadata import PackageNotFoundError
from unittest.mock import Mock

import pytest

from lobo import cli


def missing_version(name: str) -> str:
    raise PackageNotFoundError(name)


@pytest.mark.parametrize("args", [[], ["--help"]])
def test_help_without_distribution_metadata(
    args: list[str], monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setattr(cli, "version", missing_version)
    if args:
        with pytest.raises(SystemExit) as exit_info:
            cli.main(args)
        assert exit_info.value.code == 0
    else:
        assert cli.main(args) == 0
    output = capsys.readouterr()
    assert "serve" in output.out
    assert "web" in output.out
    assert "demo" in output.out
    assert output.err == ""


@pytest.mark.parametrize("no_open", [False, True])
def test_demo_hosts_the_existing_app_locally(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    *,
    no_open: bool,
) -> None:
    host = Mock(return_value=nullcontext("http://127.0.0.1:12345"))
    browser = Mock(return_value=True)
    stopped = Mock()
    monkeypatch.setattr("lobo.demo.demo_context", host)
    monkeypatch.setattr(cli, "open_web", browser)
    monkeypatch.setattr(cli.threading, "Event", lambda: stopped)
    assert (
        cli.main(["demo", "--port", "12345", *(["--no-open"] if no_open else [])]) == 0
    )
    host.assert_called_once_with(port=12345)
    stopped.wait.assert_called_once_with()
    if no_open:
        browser.assert_not_called()
    else:
        browser.assert_called_once_with("http://127.0.0.1:12345")
    assert "http://127.0.0.1:12345" in capsys.readouterr().out


@pytest.mark.parametrize("installed", [True, False])
def test_version_reports_metadata_or_an_explicit_unknown(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    *,
    installed: bool,
) -> None:
    monkeypatch.setattr(
        cli, "version", (lambda _: "1.2.3") if installed else missing_version
    )
    with pytest.raises(SystemExit) as exit_info:
        cli.main(["--version"])
    assert exit_info.value.code == 0
    output = capsys.readouterr()
    expected = "1.2.3" if installed else "unknown (package metadata unavailable)"
    assert output.out.strip() == expected
    assert output.err == ""
