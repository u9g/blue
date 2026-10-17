// Scouting: before a new session starts, a short, cheap query works out which directory the
// request is about and reports it through the set_project_directory tool. The real session then
// starts there, so Claude Code loads that project's CLAUDE.md, settings and git context.
// The scouting conversation itself is thrown away (not persisted, not forwarded).

import { statSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { createSdkMcpServer, query, tool } from "@anthropic-ai/claude-agent-sdk";
import { z } from "zod";
import { log } from "./io.ts";
import { baseOptions, userMessage } from "./sdk.ts";

const SCOUT_MODEL = "haiku";
const SCOUT_MAX_TURNS = 12;

function systemPrompt(startDir: string): string {
  return `You choose the working directory for a coding agent on this machine. You are given a \
request the user made to that agent. Do not carry out the request.

Find the directory the request is about, usually a git repository, by exploring the filesystem \
with Glob, Grep and Read. Start from ${startDir} and look in a few likely places with narrow \
patterns; don't scan the whole disk. When the request names a project, find the directory with \
that name.

Then call set_project_directory exactly once with the directory's absolute path. If the request \
isn't about any particular directory, call it with ${startDir}.`;
}

// Expands `~`, resolves relative paths against the start directory, and returns the path only
// if it is an existing directory.
function existingDirectory(path: string, startDir: string): string | undefined {
  const expanded = path === "~" || path.startsWith("~/") ? join(homedir(), path.slice(1)) : path;
  const absolute = isAbsolute(expanded) ? expanded : resolve(startDir, expanded);
  try {
    return statSync(absolute).isDirectory() ? absolute : undefined;
  } catch {
    return undefined;
  }
}

/** Returns the chosen directory, or undefined if scouting ended without choosing one. */
export async function scoutProjectDirectory(options: {
  request: string;
  startDir: string;
  claudePath: string;
  signal: AbortSignal;
}): Promise<string | undefined> {
  const { request, startDir, claudePath, signal } = options;
  let chosen: string | undefined;

  const blue = createSdkMcpServer({
    name: "blue",
    version: "1.0.0",
    tools: [
      tool(
        "set_project_directory",
        "Set the directory the coding agent will work in. Call exactly once.",
        { path: z.string().describe("Absolute path of an existing directory") },
        async ({ path }) => {
          const directory = existingDirectory(path, startDir);
          if (!directory) {
            return {
              content: [{ type: "text", text: `${path} is not an existing directory.` }],
              isError: true,
            };
          }
          chosen = directory;
          return { content: [{ type: "text", text: `Set to ${directory}. You are done.` }] };
        },
      ),
    ],
  });

  const abortController = new AbortController();
  signal.addEventListener("abort", () => abortController.abort(), { once: true });

  // Custom SDK tools need streaming input, hence the one-message generator.
  async function* input() {
    yield userMessage(`The user's request:\n\n${request}`);
  }

  const scouting = query({
    prompt: input(),
    options: {
      ...baseOptions(claudePath),
      cwd: startDir,
      model: SCOUT_MODEL,
      maxTurns: SCOUT_MAX_TURNS,
      systemPrompt: systemPrompt(startDir),
      tools: ["Glob", "Grep", "Read"],
      mcpServers: { blue },
      settingSources: [],
      persistSession: false,
      abortController,
    },
  });

  try {
    for await (const message of scouting) {
      if (message.type === "assistant") {
        for (const block of message.message.content) {
          if (block.type === "tool_use") log(`scouting: ${block.name} ${JSON.stringify(block.input)}`);
        }
      }
      if (chosen || message.type === "result") break;
    }
  } catch (err) {
    if (!signal.aborted) log(`scouting failed: ${err}`);
  } finally {
    scouting.close();
  }
  return chosen;
}
