// Compiles the bridge into a standalone executable at target/bridge/.
// `bun build --compile` won't create a missing --outfile directory on Windows, and some Bun
// versions throw EEXIST from a recursive mkdir of an existing directory, so check first.
import { existsSync, mkdirSync } from "node:fs";

const outDir = "../target/bridge";
if (!existsSync(outDir)) mkdirSync(outDir, { recursive: true });

const build = Bun.spawnSync(
  ["bun", "build", "--compile", "--minify", "src/main.ts", "--outfile", `${outDir}/claude-bridge`],
  { stdio: ["inherit", "inherit", "inherit"] },
);
process.exit(build.exitCode ?? 1);
