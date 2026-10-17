// Replay mode (BLUE_BRIDGE_REPLAY=<file>): instead of running Claude, each Prompt plays back the
// next recorded turn — every SDK message up to and including its `result`. Recordings are the
// JSON-lines files written by `blue-daemon session --record` and the daemon's live tests. Used
// to test the daemon end to end without Claude.

import { Effect, FileSystem, Ref } from "effect";
import { InvalidStart } from "./errors.ts";
import { BridgeIO } from "./io.ts";
import type { BridgeSession } from "./session.ts";

/** Splits a recording into turns, dropping the daemon's interrupt markers. */
const parseTurns = (recording: string): string[][] => {
  const turns: string[][] = [];
  let turn: string[] = [];
  for (const line of recording.split("\n")) {
    if (!line.trim()) continue;
    const type = JSON.parse(line).type;
    if (type === "blue_interrupt") continue;
    turn.push(line);
    if (type === "result") {
      turns.push(turn);
      turn = [];
    }
  }
  return turns;
};

export const makeReplaySession = Effect.fn("makeReplaySession")(function* (path: string) {
  const io = yield* BridgeIO;
  const fs = yield* FileSystem.FileSystem;
  const recording = yield* fs.readFileString(path).pipe(
    Effect.mapError(
      (error) => new InvalidStart({ reason: `can't read replay recording: ${error.message}` }),
    ),
  );
  const remaining = yield* Ref.make(parseTurns(recording));

  const session: BridgeSession = {
    prompt: () =>
      Effect.gen(function* () {
        const [turn, ...rest] = yield* Ref.get(remaining);
        yield* Ref.set(remaining, rest);
        if (!turn) {
          return yield* io.send({
            case: "error",
            value: { message: "replay has no more recorded turns" },
          });
        }
        yield* Effect.forEach(turn, (json) => io.send({ case: "sdkMessage", value: { json } }), {
          discard: true,
        });
      }),
    interrupt: Effect.void,
    close: Effect.void,
  };
  return session;
});
