// claude-bridge: a stdio adapter between blue-daemon and the Claude Agent SDK.
//
// Reads size-delimited DaemonToBridge frames from stdin and writes size-delimited BridgeToDaemon
// frames to stdout. stdout carries protocol frames only; all logging goes to stderr.

import { BunRuntime, BunServices } from "@effect/platform-bun";
import {
  Deferred,
  Effect,
  Exit,
  Layer,
  Logger,
  Option,
  Path,
  Ref,
  Runtime,
  Stream,
} from "effect";
import pkg from "../package.json";
import { type BridgeError, describeFailure, InvalidStart } from "./errors.ts";
import type { DaemonToBridge, Start } from "./gen/blue/bridge/v1/bridge_pb.ts";
import { BridgeIO } from "./io.ts";
import { makeReplaySession } from "./replay.ts";
import { type BridgeSession, makeSession } from "./session.ts";

// Must match blue_protocol::PROTOCOL_VERSION in Rust.
const PROTOCOL_VERSION = 1;

const validateStart = Effect.fnUntraced(function* (start: Start) {
  const path = yield* Path.Path;
  if (!start.claudePath) return yield* new InvalidStart({ reason: "Start.claude_path is required" });
  if (!path.isAbsolute(start.cwd)) {
    return yield* new InvalidStart({ reason: "Start.cwd must be an absolute path" });
  }
});

const program = Effect.gen(function* () {
  const io = yield* BridgeIO;
  yield* io.send({
    case: "ready",
    value: {
      protocolVersion: PROTOCOL_VERSION,
      bridgeVersion: pkg.version,
      sdkVersion: pkg.dependencies["@anthropic-ai/claude-agent-sdk"],
      runtimeVersion: Bun.version,
    },
  });

  const fatal = yield* Deferred.make<never, BridgeError>();
  const session = yield* Ref.make(Option.none<BridgeSession>());
  const replay = process.env.BLUE_BRIDGE_REPLAY;
  const error = (message: string) => io.send({ case: "error", value: { message } });

  const handle = Effect.fnUntraced(function* ({ command }: DaemonToBridge) {
    const current = yield* Ref.get(session);
    switch (command.case) {
      case "start": {
        if (Option.isSome(current)) return yield* error("Start was already received");
        yield* validateStart(command.value);
        const started = replay
          ? yield* makeReplaySession(replay)
          : yield* makeSession(command.value, fatal);
        return yield* Ref.set(session, Option.some(started));
      }
      case "prompt":
        if (Option.isNone(current)) return yield* error("Prompt before Start");
        return yield* current.value.prompt(command.value.text);
      case "interrupt":
        if (Option.isSome(current)) yield* current.value.interrupt;
        return;
      case "close":
      case undefined:
        return;
    }
  });

  // Close and stdin EOF end the session the same way: the current turn finishes first.
  const run = io.commands.pipe(
    Stream.takeUntil((input) => input.command.case === "close"),
    Stream.runForEach(handle),
    Effect.andThen(Ref.get(session)),
    Effect.flatMap(Option.match({ onNone: () => Effect.void, onSome: (s) => s.close })),
  );

  const exit = yield* Effect.exit(Effect.raceFirst(run, Deferred.await(fatal)));
  const reason = Exit.isSuccess(exit) ? "" : describeFailure(exit.cause);
  if (reason) yield* Effect.logError(`exiting: ${reason}`);
  yield* io.send({ case: "exited", value: { error: reason } });
  return yield* exit;
});

const layer = BridgeIO.layer.pipe(
  Layer.provideMerge(BunServices.layer),
  Layer.provideMerge(Layer.succeed(Logger.LogToStderr, true)),
);

BunRuntime.runMain(program.pipe(Effect.scoped, Effect.provide(layer)), {
  disableErrorReporting: true,
  // Exit once the daemon's last frame (Exited) is flushed: runMain otherwise waits for the event
  // loop to drain, and stdin stays open until the daemon closes it.
  teardown: (exit) =>
    Runtime.defaultTeardown(exit, (code) => {
      process.stdout.write("", () => process.exit(code));
    }),
});
