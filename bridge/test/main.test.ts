import { expect, test } from "bun:test";
import { create, type MessageInitShape } from "@bufbuild/protobuf";
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

type Command = MessageInitShape<typeof DaemonToBridgeSchema>["command"];

/** Spawns the bridge and waits for its Ready frame. */
async function spawnBridge(env: Record<string, string> = {}) {
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
    proc.stdin.write(sizeDelimitedEncode(DaemonToBridgeSchema, create(DaemonToBridgeSchema, { command })));
    proc.stdin.flush();
  };
  /** Expects Exited with `error`, then the process exiting with `code`. */
  const expectExit = async (error = "", code = 0) => {
    expect(await next()).toMatchObject({ case: "exited", value: { error } });
    expect(await proc.exited).toBe(code);
  };
  const ready = await next();
  if (ready.case !== "ready") throw new Error(`expected ready first, got ${ready.case}`);
  const start = { sessionId: "s1", claudePath: "claude", cwd: tmpdir() };
  return { proc, ready: ready.value, next, sendInput, expectExit, start };
}

test("handshake: Ready first, then Exited after Close", async () => {
  const { ready, sendInput, expectExit } = await spawnBridge();
  expect(ready.protocolVersion).toBe(1);
  expect(ready.sdkVersion).toBe("0.3.302");

  sendInput({ case: "close", value: {} });
  await expectExit();
});

test("Prompt before Start is an Error and the bridge keeps running", async () => {
  const { proc, next, sendInput, expectExit } = await spawnBridge();
  sendInput({ case: "prompt", value: { text: "hi" } });
  expect(await next()).toMatchObject({ case: "error", value: { message: "Prompt before Start" } });

  proc.stdin.end();
  await expectExit();
});

test("Start without claude_path stops the bridge with the reason", async () => {
  const { sendInput, expectExit, start } = await spawnBridge();
  sendInput({ case: "start", value: { ...start, claudePath: "" } });
  await expectExit("Start.claude_path is required", 1);
});

test("replay mode plays back one recorded turn per Prompt", async () => {
  const { next, sendInput, expectExit, start } = await spawnBridge({ BLUE_BRIDGE_REPLAY: RECORDING });
  sendInput({ case: "start", value: start });

  sendInput({ case: "prompt", value: { text: "first" } });
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

  sendInput({ case: "close", value: {} });
  await expectExit();
});
