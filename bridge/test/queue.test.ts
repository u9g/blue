import { expect, test } from "bun:test";
import { AsyncQueue } from "../src/queue.ts";

test("delivers items pushed before and after the reader waits, then ends", async () => {
  const queue = new AsyncQueue<number>();
  queue.push(1);
  const reader = (async () => {
    const seen: number[] = [];
    for await (const item of queue) seen.push(item);
    return seen;
  })();
  await Promise.resolve();
  queue.push(2);
  queue.push(3);
  queue.end();
  expect(await reader).toEqual([1, 2, 3]);
});

test("push after end throws", () => {
  const queue = new AsyncQueue<number>();
  queue.end();
  expect(() => queue.push(1)).toThrow();
});
