import { expect, test } from "bun:test";
import { create } from "@bufbuild/protobuf";
import { sizeDelimitedDecodeStream, sizeDelimitedEncode } from "@bufbuild/protobuf/wire";
import {
  DaemonToBridgeSchema,
  BridgeToDaemonSchema,
  type BridgeToDaemon,
} from "../src/gen/blue/bridge/v1/bridge_pb.ts";

const MAIN = new URL("../src/main.ts", import.meta.url).pathname.replace(/^\/(\w:)/, "$1");

function spawnBridge() {
  const proc = Bun.spawn(["bun", MAIN], { stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  const frames = sizeDelimitedDecodeStream(BridgeToDaemonSchema, proc.stdout)[Symbol.asyncIterator]();
  const next = async (): Promise<BridgeToDaemon["event"]> => {
    const { value, done } = await frames.next();
    if (done) throw new Error("bridge closed stdout");
    return value.event;
  };
  const sendInput = (command: Parameters<typeof create<typeof DaemonToBridgeSchema>>[1]) => {
    proc.stdin.write(sizeDelimitedEncode(DaemonToBridgeSchema, create(DaemonToBridgeSchema, command)));
    proc.stdin.flush();
  };
  return { proc, next, sendInput };
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

test("unimplemented commands get an Error and the bridge keeps running", async () => {
  const { proc, next, sendInput } = spawnBridge();
  expect((await next()).case).toBe("ready");

  sendInput({ command: { case: "interrupt", value: {} } });
  expect((await next()).case).toBe("error");

  proc.stdin.end();
  expect(await next()).toMatchObject({ case: "exited", value: { error: "" } });
  expect(await proc.exited).toBe(0);
});
