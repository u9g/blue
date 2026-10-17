// Replay mode (BLUE_BRIDGE_REPLAY=<file>): instead of running Claude, each Prompt plays back the
// next recorded turn — every SDK message up to and including its `result`. Recordings are the
// JSON-lines files the daemon's live tests write. Used to test the daemon end to end without
// Claude.

import { readFileSync } from "node:fs";
import { send } from "./io.ts";
import type { BridgeSession } from "./session.ts";

export class ReplaySession implements BridgeSession {
  private readonly turns: string[][] = [];
  private playing: Promise<void> = Promise.resolve();

  constructor(path: string) {
    let turn: string[] = [];
    for (const line of readFileSync(path, "utf8").split("\n")) {
      if (!line.trim()) continue;
      turn.push(line);
      if (JSON.parse(line).type === "result") {
        this.turns.push(turn);
        turn = [];
      }
    }
  }

  prompt(_text: string): void {
    const turn = this.turns.shift();
    this.playing = this.playing.then(async () => {
      if (!turn) {
        await send({ case: "error", value: { message: "replay has no more recorded turns" } });
        return;
      }
      for (const json of turn) await send({ case: "sdkMessage", value: { json } });
    });
  }

  async close(): Promise<void> {
    await this.playing;
  }
}
