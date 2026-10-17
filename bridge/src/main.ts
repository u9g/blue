// claude-bridge: a stdio adapter between blue-daemon and the Claude Agent SDK.
//
// Reads size-delimited BridgeInput frames from stdin and writes size-delimited BridgeOutput
// frames to stdout. stdout carries protocol frames only; all logging goes to stderr.

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { sizeDelimitedDecodeStream, sizeDelimitedEncode } from "@bufbuild/protobuf/wire";
import pkg from "../package.json";
import { BridgeInputSchema, BridgeOutputSchema } from "./gen/blue/bridge/v1/bridge_pb.ts";

// Must match blue_protocol::PROTOCOL_VERSION in Rust.
const PROTOCOL_VERSION = 1;

type OutputEvent = MessageInitShape<typeof BridgeOutputSchema>["event"];

function send(event: OutputEvent): Promise<void> {
  const frame = sizeDelimitedEncode(BridgeOutputSchema, create(BridgeOutputSchema, { event }));
  return new Promise((resolve, reject) =>
    process.stdout.write(frame, (err) => (err ? reject(err) : resolve())),
  );
}

async function main(): Promise<number> {
  await send({
    case: "ready",
    value: {
      protocolVersion: PROTOCOL_VERSION,
      bridgeVersion: pkg.version,
      sdkVersion: pkg.dependencies["@anthropic-ai/claude-agent-sdk"],
      runtimeVersion: Bun.version,
    },
  });

  for await (const input of sizeDelimitedDecodeStream(BridgeInputSchema, Bun.stdin.stream())) {
    switch (input.command.case) {
      case "close":
        await send({ case: "exited", value: {} });
        return 0;
      case undefined:
        continue;
      default:
        // Session handling (Start/Prompt/Interrupt) lands in phase 1.
        console.error(`claude-bridge: ${input.command.case} is not implemented yet`);
        await send({
          case: "error",
          value: { message: `${input.command.case} is not implemented yet` },
        });
    }
  }

  // stdin closed without Close: treat as a clean shutdown request.
  await send({ case: "exited", value: {} });
  return 0;
}

process.exit(await main());
