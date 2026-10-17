// Compiles the bridge into a standalone executable at target/bridge/.

import { BunRuntime, BunServices } from "@effect/platform-bun";
import { Effect, FileSystem, Schema } from "effect";
import { ChildProcess, ChildProcessSpawner } from "effect/process";

const OUT_DIR = "../target/bridge";

class BuildFailed extends Schema.TaggedError<BuildFailed>()("BuildFailed", {
  exitCode: Schema.Number,
}) {}

const build = Effect.gen(function* () {
  const fs = yield* FileSystem.FileSystem;
  const spawner = yield* ChildProcessSpawner.ChildProcessSpawner;

  // `bun build --compile` won't create a missing --outfile directory on Windows, and some Bun
  // versions throw EEXIST from a recursive mkdir of an existing directory, so check first.
  if (!(yield* fs.exists(OUT_DIR))) yield* fs.makeDirectory(OUT_DIR, { recursive: true });

  const exitCode = yield* spawner.exitCode(
    ChildProcess.make(
      "bun",
      ["build", "--compile", "--minify", "src/main.ts", "--outfile", `${OUT_DIR}/claude-bridge`],
      { stdout: "inherit", stderr: "inherit" },
    ),
  );
  if (exitCode !== 0) return yield* new BuildFailed({ exitCode });
});

BunRuntime.runMain(build.pipe(Effect.provide(BunServices.layer)));
