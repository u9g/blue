// Protocol I/O over stdin/stdout. stdout carries size-delimited BridgeToDaemon frames only;
// everything human-readable goes to stderr via log().

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { sizeDelimitedDecodeStream, sizeDelimitedEncode } from "@bufbuild/protobuf/wire";
import {
  BridgeToDaemonSchema,
  DaemonToBridgeSchema,
  type DaemonToBridge,
} from "./gen/blue/bridge/v1/bridge_pb.ts";

export type BridgeEvent = MessageInitShape<typeof BridgeToDaemonSchema>["event"];

export function send(event: BridgeEvent): Promise<void> {
  const frame = sizeDelimitedEncode(BridgeToDaemonSchema, create(BridgeToDaemonSchema, { event }));
  return new Promise((resolve, reject) =>
    process.stdout.write(frame, (err) => (err ? reject(err) : resolve())),
  );
}

/** A non-fatal problem for the daemon to log. */
export function sendError(message: string): Promise<void> {
  return send({ case: "error", value: { message } });
}

export function commands(): AsyncIterable<DaemonToBridge> {
  return sizeDelimitedDecodeStream(DaemonToBridgeSchema, Bun.stdin.stream());
}

export function log(message: string): void {
  console.error(`claude-bridge: ${message}`);
}
