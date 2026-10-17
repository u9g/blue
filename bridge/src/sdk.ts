// Pieces shared by the scouting query and the real session.

import type { Options, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import pkg from "../package.json";
import { log } from "./io.ts";

export function userMessage(text: string): SDKUserMessage {
  return {
    type: "user",
    message: { role: "user", content: text },
    parent_tool_use_id: null,
  };
}

// Options every query gets: the machine's own `claude` (never the SDK's bundled copy), no
// permission prompts, and Claude Code's stderr forwarded to ours.
export function baseOptions(claudePath: string): Options {
  return {
    pathToClaudeCodeExecutable: claudePath,
    permissionMode: "bypassPermissions",
    allowDangerouslySkipPermissions: true,
    env: { ...process.env, CLAUDE_AGENT_SDK_CLIENT_APP: `blue-bridge/${pkg.version}` },
    stderr: (data) => log(`claude: ${data.trimEnd()}`),
  };
}
