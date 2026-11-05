import { Connector, Shoukaku } from "shoukaku";

const USER_ID = "123456789012345678";
const GUILD_ID = "123456789012345678";

class SmokeConnector extends Connector {
  constructor() {
    super({});
  }

  getId() {
    return USER_ID;
  }

  sendPacket() {}

  listen(nodes) {
    this.ready(nodes);
  }
}

async function main() {
  const [baseUrl, password] = process.argv.slice(2);
  if (!baseUrl || !password) {
    throw new Error("usage: shoukaku-smoke.mjs BASE_URL PASSWORD");
  }

  const shoukaku = new Shoukaku(new SmokeConnector(), [], {
    resume: true,
    resumeTimeout: 60,
    reconnectTries: 1,
    reconnectInterval: 0.1,
    restTimeout: 10,
  });

  const nodeName = "ravalink-shoukaku-ci";
  let resolveReady;
  let rejectReady;
  const ready = new Promise((resolve, reject) => {
    resolveReady = resolve;
    rejectReady = reject;
  });

  shoukaku.once("ready", (name, lavalinkResume, libraryResume) => {
    resolveReady({ name, lavalinkResume, libraryResume });
  });
  shoukaku.once("error", (name, error) => rejectReady(error));

  shoukaku.addNode({
      name: nodeName,
      url: baseUrl.replace(/^https?:\/\//, ""),
      auth: password,
      secure: false,
    });

    let readyTimer;
    const timeout = new Promise((_, reject) => {
      readyTimer = setTimeout(
        () => reject(new Error("Shoukaku ready timed out")),
        10_000,
      );
    });
    const readyInfo = await Promise.race([ready, timeout]);
    clearTimeout(readyTimer);
    if (readyInfo.name !== nodeName) {
      throw new Error(`Unexpected Shoukaku node ready event: ${readyInfo.name}`);
    }

    const node = shoukaku.nodes.get(nodeName);
    if (!node?.sessionId) {
      throw new Error("Shoukaku did not receive a Lavalink Ready session");
    }

    const info = await node.rest.getLavalinkInfo();
    if (!info?.filters.includes("timescale")) {
      throw new Error("Timescale missing from Shoukaku Lavalink info");
    }

    const stats = await node.rest.stats();
    if (!stats || typeof stats.players !== "number" || !stats.memory || !stats.cpu) {
      throw new Error("Shoukaku could not deserialize Lavalink stats");
    }

    const loaded = await node.rest.resolve("ravalink-smoke:unsupported");
    if (loaded?.loadType !== "empty") {
      throw new Error(`Unexpected Shoukaku load result: ${JSON.stringify(loaded)}`);
    }

    const player = await node.rest.updatePlayer({
      guildId: GUILD_ID,
      noReplace: false,
      playerOptions: { volume: 75 },
    });
    if (!player || player.guildId !== GUILD_ID || player.volume !== 75) {
      throw new Error("Shoukaku player update response did not match Lavalink v4");
    }
    if (
      player.voice.token !== "" ||
      player.voice.endpoint !== "" ||
      player.voice.sessionId !== "" ||
      player.voice.channelId !== null
    ) {
      throw new Error("Shoukaku received an invalid default voice state");
    }

    const players = await node.rest.getPlayers();
    if (!players || players.length !== 1 || players[0].guildId !== GUILD_ID) {
      throw new Error("Shoukaku player listing failed");
    }

    await node.rest.destroyPlayer(GUILD_ID);
    const afterDelete = await node.rest.getPlayers();
    if (!afterDelete || afterDelete.length !== 0) {
      throw new Error("Shoukaku player deletion failed");
    }

  console.log("Shoukaku 4.3.0 compatibility smoke passed");
}

main()
  .then(() => process.exit(0))
  .catch((error) => {
    console.error(error);
    process.exit(1);
  });
