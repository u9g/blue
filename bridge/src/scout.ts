// Scouting: before a new session starts, a short, cheap query works out which directory the
// request is about and reports it through the set_project_directory tool. The real session then
// starts there, so Claude Code loads that project's CLAUDE.md, settings and git context.
// The scouting conversation itself is thrown away (not persisted, not forwarded).

import { homedir } from "node:os";
import { createSdkMcpServer, tool } from "@anthropic-ai/claude-agent-sdk";
import { Deferred, Effect, FileSystem, Option, Path, Stream } from "effect";
import { z } from "zod";
import { SdkError } from "./errors.ts";
import { openQuery, userMessage } from "./sdk.ts";

const SCOUT_MODEL = "haiku";
const SCOUT_MAX_TURNS = 12;

const systemPrompt = (startDir: string) => `You choose the working directory for a coding agent \
on this machine. You are given a request the user made to that agent. Do not carry out the \
request.

Find the directory the request is about, usually a git repository, by exploring the filesystem \
with Glob, Grep and Read. Start from ${startDir} and look in a few likely places with narrow \
patterns; don't scan the whole disk. When the request names a project, find the directory with \
that name.

Then call set_project_directory exactly once with the directory's absolute path. If the request \
isn't about any particular directory, call it with ${startDir}.`;

/** Returns the chosen directory, or none if scouting ended without choosing one. */
export const scoutProjectDirectory = Effect.fn("scoutProjectDirectory")(
  function* (options: { request: string; startDir: string; claudePath: string }) {
    const { request, startDir, claudePath } = options;
    const fs = yield* FileSystem.FileSystem;
    const path = yield* Path.Path;
    const run = Effect.runPromiseWith(yield* Effect.context());
    const chosen = yield* Deferred.make<string>();

    // Expands `~`, resolves relative paths against the start directory, and keeps the path only
    // if it is an existing directory.
    const existingDirectory = (input: string) => {
      const expanded =
        input === "~" || input.startsWith("~/") ? path.join(homedir(), input.slice(1)) : input;
      const absolute = path.resolve(startDir, expanded);
      return fs.stat(absolute).pipe(
        Effect.map((info) => (info.type === "Directory" ? Option.some(absolute) : Option.none())),
        Effect.orElseSucceed(() => Option.none<string>()),
      );
    };

    const blue = createSdkMcpServer({
      name: "blue",
      version: "1.0.0",
      tools: [
        tool(
          "set_project_directory",
          "Set the directory the coding agent will work in. Call exactly once.",
          { path: z.string().describe("Absolute path of an existing directory") },
          ({ path: input }) =>
            run(
              Effect.gen(function* () {
                const directory = yield* existingDirectory(input);
                if (Option.isNone(directory)) {
                  return {
                    content: [{ type: "text" as const, text: `${input} is not an existing directory.` }],
                    isError: true,
                  };
                }
                yield* Deferred.succeed(chosen, directory.value);
                return {
                  content: [{ type: "text" as const, text: `Set to ${directory.value}. You are done.` }],
                };
              }),
            ),
        ),
      ],
    });

    // Custom SDK tools need streaming input, hence the one-message stream.
    const scouting = yield* openQuery(
      Stream.toAsyncIterable(Stream.succeed(userMessage(`The user's request:\n\n${request}`))),
      {
        claudePath,
        cwd: startDir,
        model: SCOUT_MODEL,
        maxTurns: SCOUT_MAX_TURNS,
        systemPrompt: systemPrompt(startDir),
        tools: ["Glob", "Grep", "Read"],
        mcpServers: { blue },
        settingSources: [],
        persistSession: false,
      },
    );

    const untilResult = Stream.fromAsyncIterable(scouting, (cause) => new SdkError({ cause })).pipe(
      Stream.tap((message) =>
        message.type === "assistant"
          ? Effect.forEach(message.message.content, (block) =>
              block.type === "tool_use"
                ? Effect.logInfo(`scouting: ${block.name} ${JSON.stringify(block.input)}`)
                : Effect.void,
            )
          : Effect.void,
      ),
      Stream.takeUntil((message) => message.type === "result"),
      Stream.runDrain,
      Effect.as(Option.none<string>()),
    );

    // Done as soon as the tool picks a directory; the query is closed when the scope ends.
    return yield* Effect.raceFirst(Deferred.await(chosen).pipe(Effect.map(Option.some)), untilResult);
  },
  Effect.catchTag("SdkError", (error) =>
    Effect.logWarning("scouting failed", error.cause).pipe(Effect.as(Option.none<string>())),
  ),
  Effect.scoped,
);
