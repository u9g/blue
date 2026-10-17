import { expect, test } from "bun:test";
import { create } from "@bufbuild/protobuf";
import { sizeDelimitedDecodeStream, sizeDelimitedEncode } from "@bufbuild/protobuf/wire";
import { tmpdir } from "node:os";
import {
  DaemonToBridgeSchema,
  BridgeToDaemonSchema,
  type BridgeToDaemon,
} from "../src/gen/blue/bridge/v1/bridge_pb.ts";

const path = (relative: string) =>
  new URL(relative, import.meta.url).pathname.replace(/^\/(\w:)/, "$1");
const MAIN = path("../src/main.ts");
const RECORDING = path("../../crates/daemon/tests/fixtures/turns.jsonl");

type Command = Parameters<typeof create<typeof DaemonToBridgeSchema>>[1];

function spawnBridge(env: Record<string, string> = {}) {
  const proc = Bun.spawn(["bun", MAIN], {
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
    env: { ...process.env, ...env },
  });
  const frames = sizeDelimitedDecodeStream(BridgeToDaemonSchema, proc.stdout)[Symbol.asyncIterator]();
  const next = async (): Promise<BridgeToDaemon["event"]> => {
    const { value, done } = await frames.next();
    if (done) throw new Error("bridge closed stdout");
    return value.event;
  };
  const sendInput = (command: Command) => {
    proc.stdin.write(sizeDelimitedEncode(DaemonToBridgeSchema, create(DaemonToBridgeSchema, command)));
    proc.stdin.flush();
  };
  const start = { sessionId: "s1", claudePath: "claude", cwd: tmpdir() };
  return { proc, next, sendInput, start };
}

test("handshake: Ready first, then Exited after Close", async () => {
  const { proc, next, sendInput } = spawnBridge();

  const ready = await next();
  expect(ready.case).toBe("ready");
  if (ready.case === "ready") {
    expect(ready.value.protocolVersion).toBe(1);
    expect(ready.value.sdkVersion).toBe("0.3.302");
  }

  sendInput({ command: { case: "close", value: {} } });
  expect(await next()).toMatchObject({ case: "exited", value: { error: "" } });
  expect(await proc.exited).toBe(0);
});

test("Prompt before Start is an Error and the bridge keeps running", async () => {
  const { proc, next, sendInput } = spawnBridge();
  expect((await next()).case).toBe("ready");

  sendInput({ command: { case: "prompt", value: { text: "hi" } } });
  expect(await next()).toMatchObject({ case: "error", value: { message: "Prompt before Start" } });

  proc.stdin.end();
  expect(await next()).toMatchObject({ case: "exited", value: { error: "" } });
  expect(await proc.exited).toBe(0);
});

test("Start without claude_path stops the bridge with the reason", async () => {
  const { proc, next, sendInput, start } = spawnBridge();
  expect((await next()).case).toBe("ready");

  sendInput({ command: { case: "start", value: { ...start, claudePath: "" } } });
  expect(await next()).toMatchObject({
    case: "exited",
    value: { error: "Start.claude_path is required" },
  });
  expect(await proc.exited).toBe(1);
});

test("replay mode plays back one recorded turn per Prompt", async () => {
  const { proc, next, sendInput, start } = spawnBridge({ BLUE_BRIDGE_REPLAY: RECORDING });
  expect((await next()).case).toBe("ready");
  sendInput({ command: { case: "start", value: start } });

  sendInput({ command: { case: "prompt", value: { text: "first" } } });
  const types: string[] = [];
  for (;;) {
    const event = await next();
    if (event.case !== "sdkMessage") throw new Error(`unexpected ${event.case}`);
    const message = JSON.parse(event.value.json);
    types.push(message.type);
    if (message.type === "result") break;
  }
  expect(types[0]).toBe("system");
  expect(types.at(-1)).toBe("result");

  sendInput({ command: { case: "close", value: {} } });
  expect(await next()).toMatchObject({ case: "exited", value: { error: "" } });
  expect(await proc.exited).toBe(0);
});
