// Pieces shared by the scouting query and the real session.

import {
  type Options,
  query,
  type Query,
  type SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";
import { Effect, type Scope } from "effect";
import pkg from "../package.json";

export const userMessage = (text: string): SDKUserMessage => ({
  type: "user",
  message: { role: "user", content: text },
  parent_tool_use_id: null,
});

/**
 * Starts an SDK query that is closed (ending its Claude Code process) when the scope closes.
 *
 * Every query uses the machine's own `claude` (never the SDK's bundled copy), skips permission
 * prompts, and forwards Claude Code's stderr to our logs.
 */
export const openQuery = Effect.fnUntraced(function* (
  prompt: string | AsyncIterable<SDKUserMessage>,
  options: Options & { readonly claudePath: string },
): Effect.fn.Return<Query, never, Scope.Scope> {
  const { claudePath, ...rest } = options;
  const runFork = Effect.runForkWith(yield* Effect.context());
  const log = (data: string) => runFork(Effect.logInfo(`claude: ${data.trimEnd()}`));
  return yield* Effect.acquireRelease(
    Effect.sync(() =>
      query({
        prompt,
        options: {
          pathToClaudeCodeExecutable: claudePath,
          permissionMode: "bypassPermissions",
          allowDangerouslySkipPermissions: true,
          env: { ...process.env, CLAUDE_AGENT_SDK_CLIENT_APP: `blue-bridge/${pkg.version}` },
          stderr: log,
          ...rest,
        },
      }),
    ),
    (sdkQuery) => Effect.sync(() => sdkQuery.close()),
  );
});
