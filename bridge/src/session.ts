// One Claude session: a single streaming-input query that stays alive across turns. Every SDK
// message is forwarded to the daemon verbatim; the daemon decides what it means.

import { query, type Query, type SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { Start } from "./gen/blue/bridge/v1/bridge_pb.ts";
import { log, send } from "./io.ts";
import { AsyncQueue } from "./queue.ts";
import { scoutProjectDirectory } from "./scout.ts";
import { baseOptions, userMessage } from "./sdk.ts";

/** What main.ts drives: a live Session, or a ReplaySession in replay mode. */
export interface BridgeSession {
  prompt(text: string): void;
  interrupt(): Promise<void>;
  close(): Promise<void>;
}

export class Session implements BridgeSession {
  private readonly input = new AsyncQueue<SDKUserMessage>();
  private sdkQuery: Query | undefined;
  private forwarding: Promise<void> = Promise.resolve();
  // Prompts are handled one after another; the first may include scouting.
  private prompts: Promise<void> = Promise.resolve();
  private scouting: AbortController | undefined;
  private interruptAfterStart = false;

  constructor(
    private readonly start: Start,
    // Called when the session can't continue; the bridge reports it and exits.
    private readonly fatal: (err: unknown) => void,
  ) {}

  prompt(text: string): void {
    this.prompts = this.prompts
      .then(async () => {
        if (!this.sdkQuery) this.startQuery(await this.chooseDirectory(text));
        this.input.push(userMessage(text));
        if (this.interruptAfterStart) {
          this.interruptAfterStart = false;
          await this.sdkQuery!.interrupt();
        }
      })
      .catch(this.fatal);
  }

  async interrupt(): Promise<void> {
    if (this.scouting) {
      // Stop scouting, start the session in the starting directory, and interrupt that turn
      // right away so the daemon still sees the turn end with an SDK result.
      this.interruptAfterStart = true;
      this.scouting.abort();
    } else if (this.sdkQuery) {
      await this.sdkQuery.interrupt();
    }
  }

  /** Lets the current turn finish (scouting included), then ends the input stream and waits
   * for the query to finish. */
  async close(): Promise<void> {
    await this.prompts;
    this.input.end();
    await this.forwarding;
  }

  private async chooseDirectory(firstPrompt: string): Promise<string> {
    const { cwd, claudePath, resumeClaudeSessionId } = this.start;
    if (resumeClaudeSessionId !== undefined) return cwd;

    this.scouting = new AbortController();
    try {
      const chosen = await scoutProjectDirectory({
        request: firstPrompt,
        startDir: cwd,
        claudePath,
        signal: this.scouting.signal,
      });
      if (chosen) {
        log(`project directory: ${chosen}`);
        return chosen;
      }
      if (!this.scouting.signal.aborted) {
        await send({
          case: "error",
          value: { message: `Couldn't pick a project directory; working in ${cwd}.` },
        });
      }
      return cwd;
    } finally {
      this.scouting = undefined;
    }
  }

  private startQuery(cwd: string): void {
    const { claudePath, model, resumeClaudeSessionId } = this.start;
    this.sdkQuery = query({
      prompt: this.input,
      options: {
        ...baseOptions(claudePath),
        cwd,
        model,
        resume: resumeClaudeSessionId,
        // Behave like Claude Code itself: its system prompt, and all settings sources
        // (the default), which includes the project's CLAUDE.md.
        systemPrompt: { type: "preset", preset: "claude_code" },
      },
    });
    const sdkQuery = this.sdkQuery;
    this.forwarding = (async () => {
      for await (const message of sdkQuery) {
        await send({ case: "sdkMessage", value: { json: JSON.stringify(message) } });
      }
    })().catch(this.fatal);
  }
}
