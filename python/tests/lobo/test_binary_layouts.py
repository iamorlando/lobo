"""Exercise public binary declarations against native replay and generated help."""

# ruff: noqa: S101, D103, FBT001

import gzip
import inspect
from pathlib import Path

import pytest

from lobo.replay.adapters import CustomAdapter, Protocol
from lobo.replay.adapters import expressions as le
from lobo.replay.adapters import models as lm


def uint(offset: int, size: int = 1) -> lm.UInt:
    return lm.UInt(offset, size, byteorder="little")


def protocol(*, inclusive: bool = True) -> Protocol:
    return Protocol(
        lm.Binary(
            length=uint(0, 2),
            length_includes_prefix=inclusive,
            tag=uint(0),
            key=uint(1),
            timestamp=uint(2),
            records={
                1: lm.Record(
                    fields={"version": uint(4)},
                    block_length=uint(3),
                    block_offset=4,
                    groups=[
                        lm.Group(
                            "orders",
                            header_size=4,
                            count=uint(2, 2),
                            block_length=uint(0, 2),
                            fields={
                                "symbol": lm.Text(0, 3),
                                "id": uint(3, 2),
                                "price": uint(5, 2),
                                "quantity": uint(7, 2),
                            },
                        )
                    ],
                    actions=[
                        le.When(
                            le.Field("version").eq(1),
                            le.ForEach(
                                le.Field("orders"),
                                lm.Book(
                                    le.Field("symbol"),
                                    lm.Add(
                                        id=le.Field("id"),
                                        side="buy",
                                        price=le.Field("price"),
                                        quantity=le.Field("quantity"),
                                    ),
                                    snapshot=True,
                                ),
                            ),
                        )
                    ],
                )
            },
        )
    )


def frame(count: int, *, extension: int = 0, inclusive: bool = True) -> bytes:
    body = bytes([1, 0, 1, 1 + extension, 1]) + b"\xff" * extension
    body += (9 + extension).to_bytes(2, "little") + count.to_bytes(2, "little")
    for symbol, identifier, price, quantity in [
        (b"AAA", 513, 1200, 300),
        (b"BBB", 514, 2400, 600),
    ][:count]:
        body += symbol + b"".join(
            n.to_bytes(2, "little") for n in (identifier, price, quantity)
        )
        body += b"\xff" * extension
    return (len(body) + (2 if inclusive else 0)).to_bytes(2, "little") + body


def adapter(source: lm.Source, *, inclusive: bool = True) -> CustomAdapter:
    return CustomAdapter(
        protocol(inclusive=inclusive),
        source,
        instruments=[lm.Instrument(symbol, 0, 0) for symbol in ("AAA", "BBB")],
        symbol="AAA",
        mode=lm.FeedMode.Replay,
    )


@pytest.mark.parametrize("inclusive", [False, True])
@pytest.mark.parametrize("source_kind", ["chunks", "file", "gzip"])
def test_variable_groups_route_entries_and_skip_extensions(
    tmp_path: Path, inclusive: bool, source_kind: str
) -> None:
    data = (
        frame(0, inclusive=inclusive)
        + frame(1, inclusive=inclusive)
        + frame(2, extension=3, inclusive=inclusive)
    )
    if source_kind == "chunks":
        source = lm.Source.packets([data[i : i + 1] for i in range(len(data))])
    else:
        path = tmp_path / "records.bin"
        path.write_bytes(gzip.compress(data) if source_kind == "gzip" else data)
        source = lm.Source.file(path, chunk_size=3)
    with adapter(source, inclusive=inclusive) as feed:
        feed.start()
        feed.wait()
        assert feed.status()["messages"] == 3
        assert feed.status()["bytes"] == len(data)
        assert feed.queue("AAA", "buy", 0, 10000)[0][:3] == (513, 1200, 300)
        assert feed.queue("BBB", "buy", 0, 10000)[0][:3] == (514, 2400, 600)


@pytest.mark.parametrize(
    "data,match",
    [
        (b"\x01\x00", "Invalid record length"),
        (b"\x02\x00", "Invalid record length"),
        (b"\x00", "Truncated record prefix"),
        (frame(2)[:-1], "Truncated record body"),
    ],
)
def test_invalid_frames_are_reported_by_wait(data: bytes, match: str) -> None:
    with adapter(lm.Source.packets([data])) as feed:
        feed.start()
        with pytest.raises(RuntimeError, match=match):
            feed.wait()


@pytest.mark.parametrize(
    "offset,value,match",
    [
        (5, 0, "root block"),
        (7, 8, "block length is too small"),
        (9, 3, "Truncated binary group entries"),
    ],
)
def test_malformed_groups_fail_before_any_book_action(
    offset: int, value: int, match: str
) -> None:
    data = bytearray(frame(2))
    data[offset] = value
    with adapter(lm.Source.packets([bytes(data)])) as feed:
        feed.start()
        with pytest.raises(RuntimeError, match=match):
            feed.wait()
        assert feed.status()["books"] == 0


def test_variable_records_report_bulk_execution_limit(tmp_path: Path) -> None:
    path = tmp_path / "records.bin"
    path.write_bytes(frame(2))
    with (
        adapter(lm.Source.file(path)) as feed,
        pytest.raises(RuntimeError, match="Variable binary records.*start"),
    ):
        feed.run(concurrent=False)


def test_group_directory_is_discovered_from_a_binary_file(tmp_path: Path) -> None:
    # Reuse the public declarations, replacing book actions with group metadata.
    group = lm.Group(
        "orders",
        header_size=4,
        count=uint(2, 2),
        block_length=uint(0, 2),
        fields={"symbol": lm.Text(0, 3), "id": uint(3, 2)},
    )
    record = lm.Record(
        fields={"version": uint(4)},
        block_length=uint(3),
        block_offset=4,
        groups=[group],
        actions=[
            le.ForEach(
                le.Field("orders"),
                lm.Register(
                    symbol=le.Field("symbol"),
                    key=le.Field("id"),
                    price_decimals=2,
                    quantity_decimals=0,
                ),
            )
        ],
    )
    wire = Protocol(
        lm.Binary(
            length=uint(0, 2),
            length_includes_prefix=True,
            tag=uint(0),
            key=uint(1),
            timestamp=uint(2),
            records={1: record},
        )
    )
    path = tmp_path / "directory.bin"
    path.write_bytes(frame(2, extension=3))
    with CustomAdapter(wire, lm.Source.file(path), symbol="AAA") as feed:
        assert feed.tickers() == ["AAA", "BBB"]


def test_binary_help_and_exports_describe_offset_origins() -> None:
    assert "length_includes_prefix" in inspect.signature(lm.Binary).parameters
    assert inspect.signature(lm.Record).parameters["size"].default is None
    assert "relative to each entry" in inspect.getdoc(lm.Group)
    assert "start()" in inspect.getdoc(lm.Binary)
    data = protocol().data["format"]
    assert data["length_includes_prefix"] is True
    assert data["records"]["1"]["groups"][0]["name"] == "orders"


def test_inclusive_fixed_records_work_in_bulk_and_streaming(tmp_path: Path) -> None:
    wire = Protocol(
        lm.Binary(
            length=uint(0, 2),
            length_includes_prefix=True,
            tag=uint(0),
            key=uint(1),
            timestamp=uint(2),
            records={
                1: lm.Record(
                    size=6,
                    fields={"symbol": lm.Text(3, 3)},
                    actions=[
                        lm.Register(
                            symbol=le.Field("symbol"),
                            key=le.Variable("key"),
                            price_decimals=0,
                            quantity_decimals=0,
                        ),
                        lm.Add(id=1, side="buy", price=100, quantity=9),
                    ],
                )
            },
        )
    )
    path = tmp_path / "fixed.bin"
    path.write_bytes(b"\x08\x00\x01\x01\x01AAA")
    with CustomAdapter(
        wire, lm.Source.file(path), symbol="AAA", mode=lm.FeedMode.Replay
    ) as feed:
        assert feed.run(concurrent=False)["aaa"].orders.bids.visible_quantity == 9
    with CustomAdapter(
        wire, lm.Source.file(path), symbol="AAA", mode=lm.FeedMode.Replay
    ) as feed:
        feed.start()
        feed.wait()
        assert feed.levels("AAA")[0]["quantity"] == 9


@pytest.mark.parametrize(
    "overrides",
    [
        {"alignment": 3},
        {"max_count": 0},
        {"count": lm.Text(0, 1)},
        {"block_length": lm.Text(0, 1)},
        {"header_size": 1},
    ],
)
def test_invalid_group_declarations_fail_during_setup(overrides: dict) -> None:
    arguments = {
        "header_size": 4,
        "count": uint(2, 2),
        "block_length": uint(0, 2),
        "fields": {},
    }
    arguments.update(overrides)
    with pytest.raises(ValueError):
        lm.Group("items", **arguments)
