// One Claude session: a single streaming-input query that stays alive across turns. Every SDK
// message is forwarded to the daemon verbatim; the daemon decides what it means.

import type { Query, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import {
  type Cause,
  Deferred,
  Effect,
  Exit,
  Fiber,
  type FileSystem,
  Option,
  type Path,
  Queue,
  Ref,
  Scope,
  Stream,
} from "effect";
import { type BridgeError, SdkError } from "./errors.ts";
import type { Start } from "./gen/blue/bridge/v1/bridge_pb.ts";
import { BridgeIO } from "./io.ts";
import { scoutProjectDirectory } from "./scout.ts";
import { openQuery, userMessage } from "./sdk.ts";

/** What main.ts drives: a live session, or a replay session in replay mode. */
export interface BridgeSession {
  /** Queues a turn; returns immediately (scouting can take a while). */
  prompt(text: string): Effect.Effect<void>;
  readonly interrupt: Effect.Effect<void, BridgeError>;
  /** Lets the current turn finish (scouting included), then ends the session. */
  readonly close: Effect.Effect<void, BridgeError>;
}

/**
 * Starts a live session. Its queries live in the current scope, and anything that stops it from
 * continuing fails `fatal`, which ends the bridge.
 */
export const makeSession = Effect.fn("makeSession")(function* (
  start: Start,
  fatal: Deferred.Deferred<never, BridgeError>,
): Effect.fn.Return<
  BridgeSession,
  never,
  BridgeIO | FileSystem.FileSystem | Path.Path | Scope.Scope
> {
  const io = yield* BridgeIO;
  const scope = yield* Scope.Scope;
  const input = yield* Queue.unbounded<SDKUserMessage, Cause.Done>();
  const prompts = yield* Queue.unbounded<string, Cause.Done>();
  const sdkQuery = yield* Ref.make(Option.none<Query>());
  const forwarding = yield* Ref.make(Option.none<Fiber.Fiber<void, SdkError>>());
  const scouting = yield* Ref.make(Option.none<Fiber.Fiber<Option.Option<string>>>());
  const interruptAfterStart = yield* Ref.make(false);
  const failFatally = <A, E extends BridgeError, R>(effect: Effect.Effect<A, E, R>) =>
    Effect.tapError(effect, (error) => Deferred.fail(fatal, error));

  const sdkInterrupt = (active: Query) =>
    Effect.tryPromise({
      try: () => active.interrupt(),
      catch: (cause) => new SdkError({ cause }),
    }).pipe(Effect.asVoid);

  const chooseDirectory = Effect.fnUntraced(function* (firstPrompt: string) {
    const { cwd, claudePath, resumeClaudeSessionId } = start;
    if (resumeClaudeSessionId !== undefined) return cwd;

    const fiber = yield* Effect.forkChild(
      scoutProjectDirectory({ request: firstPrompt, startDir: cwd, claudePath }),
    );
    yield* Ref.set(scouting, Option.some(fiber));
    const exit = yield* Fiber.await(fiber);
    yield* Ref.set(scouting, Option.none());

    if (Exit.isSuccess(exit) && Option.isSome(exit.value)) {
      yield* Effect.logInfo(`project directory: ${exit.value.value}`);
      return exit.value.value;
    }
    if (Exit.isSuccess(exit)) {
      yield* io.send({
        case: "error",
        value: { message: `Couldn't pick a project directory; working in ${cwd}.` },
      });
    }
    return cwd;
  });

  const startQuery = Effect.fnUntraced(function* (cwd: string) {
    const { claudePath, model, resumeClaudeSessionId } = start;
    const active = yield* openQuery(Stream.toAsyncIterable(Stream.fromQueue(input)), {
      claudePath,
      cwd,
      model,
      resume: resumeClaudeSessionId,
      // Behave like Claude Code itself: its system prompt, and all settings sources (the
      // default), which includes the project's CLAUDE.md.
      systemPrompt: { type: "preset", preset: "claude_code" },
    }).pipe(Scope.provide(scope));
    yield* Ref.set(sdkQuery, Option.some(active));

    const forward = yield* Stream.fromAsyncIterable(active, (cause) => new SdkError({ cause })).pipe(
      Stream.runForEach((message) =>
        io.send({ case: "sdkMessage", value: { json: JSON.stringify(message) } }),
      ),
      failFatally,
      Effect.forkIn(scope),
    );
    yield* Ref.set(forwarding, Option.some(forward));
    return active;
  });

  const handlePrompt = Effect.fnUntraced(function* (text: string) {
    const existing = yield* Ref.get(sdkQuery);
    const active = Option.isSome(existing)
      ? existing.value
      : yield* startQuery(yield* chooseDirectory(text));
    yield* Queue.offer(input, userMessage(text));
    if (yield* Ref.getAndSet(interruptAfterStart, false)) yield* sdkInterrupt(active);
  });

  const worker = yield* Stream.fromQueue(prompts).pipe(
    Stream.runForEach(handlePrompt),
    failFatally,
    Effect.forkIn(scope),
  );

  return {
    prompt: (text) => Queue.offer(prompts, text).pipe(Effect.asVoid),

    interrupt: Effect.gen(function* () {
      const scoutingFiber = yield* Ref.get(scouting);
      if (Option.isSome(scoutingFiber)) {
        // Stop scouting, start the session in the starting directory, and interrupt that turn
        // right away so the daemon still sees the turn end with an SDK result.
        yield* Ref.set(interruptAfterStart, true);
        yield* Fiber.interrupt(scoutingFiber.value);
        return;
      }
      const active = yield* Ref.get(sdkQuery);
      if (Option.isSome(active)) yield* sdkInterrupt(active.value);
    }),

    close: Effect.gen(function* () {
      yield* Queue.end(prompts);
      yield* Fiber.join(worker);
      yield* Queue.end(input);
      const forward = yield* Ref.get(forwarding);
      if (Option.isSome(forward)) yield* Fiber.join(forward.value);
    }),
  };
});
