// A push-based async iterable: the SDK's streaming input reads user messages from it, and the
// bridge pushes one per Prompt.

export class AsyncQueue<T> implements AsyncIterable<T> {
  private items: T[] = [];
  private waiting: ((result: IteratorResult<T>) => void) | undefined;
  private ended = false;

  push(item: T): void {
    if (this.ended) throw new Error("push after end");
    if (!this.wake({ value: item, done: false })) this.items.push(item);
  }

  end(): void {
    this.ended = true;
    this.wake({ value: undefined, done: true });
  }

  /** Hands `result` to a pending `next()`, if there is one. */
  private wake(result: IteratorResult<T>): boolean {
    const resolve = this.waiting;
    this.waiting = undefined;
    resolve?.(result);
    return resolve !== undefined;
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return {
      next: () => {
        const item = this.items.shift();
        if (item !== undefined) return Promise.resolve({ value: item, done: false });
        if (this.ended) return Promise.resolve({ value: undefined, done: true });
        return new Promise((resolve) => {
          this.waiting = resolve;
        });
      },
    };
  }
}
