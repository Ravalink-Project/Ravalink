#!/usr/bin/env python3
"""Exercise the real Wavelink 3.5.2 client against a local RavaLink process."""

from __future__ import annotations

import asyncio
import sys
from types import SimpleNamespace

import wavelink


GUILD_ID = 123456789012345678
USER_ID = 123456789012345678


class FakeDiscordClient:
    def __init__(self) -> None:
        self.user = SimpleNamespace(id=USER_ID)
        self.ready = asyncio.Event()
        self.events: list[str] = []

    def dispatch(self, event: str, *args: object) -> None:
        self.events.append(event)
        if event == "wavelink_node_ready":
            self.ready.set()


async def run(base_url: str, password: str) -> None:
    client = FakeDiscordClient()
    node = wavelink.Node(
        identifier="ravalink-wavelink-ci",
        uri=base_url,
        password=password,
        client=client,  # type: ignore[arg-type]
        heartbeat=5.0,
        retries=0,
        resume_timeout=60,
    )

    try:
        await node._connect(client=client)  # type: ignore[arg-type]
        await asyncio.wait_for(client.ready.wait(), timeout=10)

        assert node.session_id, "Wavelink did not receive a Lavalink Ready message"

        info = await node._fetch_info()
        assert "timescale" in info["filters"], "Timescale is missing from /v4/info"

        stats = await node._fetch_stats()
        assert "players" in stats and "playingPlayers" in stats

        loaded = await node._fetch_tracks("ravalink-smoke:unsupported")
        assert loaded["loadType"] == "empty", f"unexpected load result: {loaded!r}"

        player = await node._update_player(
            GUILD_ID,
            data={"volume": 75},
            replace=True,
        )
        assert player["guildId"] == str(GUILD_ID)
        assert player["volume"] == 75
        assert player["track"] is None
        assert player["voice"]["token"] == ""
        assert player["voice"]["endpoint"] == ""
        assert player["voice"]["sessionId"] == ""
        assert player["voice"]["channelId"] is None
        assert player["state"]["time"] > 0

        players = await node._fetch_players()
        assert len(players) == 1
        assert players[0]["guildId"] == str(GUILD_ID)

        await node._destroy_player(GUILD_ID)
        assert await node._fetch_players() == []

        print(
            "Wavelink compatibility smoke passed:",
            f"version={wavelink.__version__}",
            f"session={node.session_id}",
        )
    finally:
        await node.close()
        if not node._session.closed:
            await node._session.close()


async def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: wavelink-smoke.py BASE_URL PASSWORD")
    await run(sys.argv[1], sys.argv[2])


if __name__ == "__main__":
    asyncio.run(main())
