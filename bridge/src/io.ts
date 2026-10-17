// Protocol I/O over stdin/stdout. stdout carries size-delimited BridgeToDaemon frames only;
// logs go to stderr (see Logger.LogToStderr in main.ts).

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { sizeDelimitedDecodeStream, sizeDelimitedEncode } from "@bufbuild/protobuf/wire";
import { type Cause, Context, Effect, Fiber, Layer, Queue, Stdio, Stream } from "effect";
import { ProtocolError } from "./errors.ts";
import {
  BridgeToDaemonSchema,
  type DaemonToBridge,
  DaemonToBridgeSchema,
} from "./gen/blue/bridge/v1/bridge_pb.ts";

export type BridgeEvent = MessageInitShape<typeof BridgeToDaemonSchema>["event"];

export class BridgeIO extends Context.Service<
  BridgeIO,
  {
    /** Queues a frame for stdout. Frames are written in order, and all of them are flushed
     * before the layer shuts down. */
    send(event: BridgeEvent): Effect.Effect<void>;
    readonly commands: Stream.Stream<DaemonToBridge, ProtocolError>;
  }
>()("claude-bridge/BridgeIO") {
  static readonly layer = Layer.effect(
    BridgeIO,
    Effect.gen(function* () {
      const stdio = yield* Stdio.Stdio;

      const outgoing = yield* Queue.unbounded<Uint8Array, Cause.Done>();
      const writer = yield* Stream.fromQueue(outgoing).pipe(
        Stream.run(stdio.stdout()),
        Effect.tapError((error) => Effect.logError("writing to stdout failed", error)),
        Effect.forkScoped,
      );
      yield* Effect.addFinalizer(() =>
        Queue.end(outgoing).pipe(Effect.andThen(Fiber.await(writer))),
      );

      const send = (event: BridgeEvent) =>
        Queue.offer(
          outgoing,
          sizeDelimitedEncode(BridgeToDaemonSchema, create(BridgeToDaemonSchema, { event })),
        ).pipe(Effect.asVoid);

      const commands = Stream.fromAsyncIterable(
        sizeDelimitedDecodeStream(DaemonToBridgeSchema, Stream.toAsyncIterable(stdio.stdin)),
        (cause) => new ProtocolError({ cause }),
      );

      return BridgeIO.of({ send, commands });
    }),
  );
}
