// claude-bridge: a stdio adapter between blue-daemon and the Claude Agent SDK.
//
// Reads size-delimited DaemonToBridge frames from stdin and writes size-delimited BridgeToDaemon
// frames to stdout. stdout carries protocol frames only; all logging goes to stderr.

import { isAbsolute } from "node:path";
import pkg from "../package.json";
import type { Start } from "./gen/blue/bridge/v1/bridge_pb.ts";
import { commands, log, send, sendError } from "./io.ts";
import { ReplaySession } from "./replay.ts";
import { Session, type BridgeSession } from "./session.ts";

// Must match blue_protocol::PROTOCOL_VERSION in Rust.
const PROTOCOL_VERSION = 1;

async function exit(error: unknown): Promise<never> {
  const message = error === undefined ? "" : error instanceof Error ? error.message : String(error);
  if (message) log(`exiting: ${message}`);
  await send({ case: "exited", value: { error: message } });
  process.exit(message ? 1 : 0);
}

function invalidStart(start: Start): string | undefined {
  if (!start.claudePath) return "Start.claude_path is required";
  if (!isAbsolute(start.cwd)) return "Start.cwd must be an absolute path";
  return undefined;
}

async function main(): Promise<never> {
  await send({
    case: "ready",
    value: {
      protocolVersion: PROTOCOL_VERSION,
      bridgeVersion: pkg.version,
      sdkVersion: pkg.dependencies["@anthropic-ai/claude-agent-sdk"],
      runtimeVersion: Bun.version,
    },
  });

  let session: BridgeSession | undefined;
  const replay = process.env.BLUE_BRIDGE_REPLAY;

  for await (const input of commands()) {
    const command = input.command;
    switch (command.case) {
      case "start": {
        if (session) {
          await sendError("Start was already received");
          break;
        }
        const problem = invalidStart(command.value);
        if (problem) return exit(problem);
        session = replay ? new ReplaySession(replay) : new Session(command.value, exit);
        break;
      }
      case "prompt":
        if (session) session.prompt(command.value.text);
        else await sendError("Prompt before Start");
        break;
      case "close":
        await session?.close();
        return exit(undefined);
      case undefined:
        break;
    }
  }

  // stdin closed without Close: shut down the same way.
  await session?.close();
  return exit(undefined);
}

await main().catch(exit);
