#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const { version } = JSON.parse(
  readFileSync(join(root, "package.json"), "utf8"),
);
const cache = join(
  process.env.XDG_CACHE_HOME || join(homedir(), ".cache"),
  "jev-compile",
  `${version}-${process.platform}-${process.arch}`,
);
const executable = join(
  cache,
  "bin",
  process.platform === "win32" ? "jev-compile.exe" : "jev-compile",
);

function finish(result) {
  if (result.error) {
    console.error(`jev-compile: ${result.error.message}`);
    process.exit(1);
  }
  if (result.signal) {
    process.kill(process.pid, result.signal);
    process.exit(1);
  }
  process.exit(result.status ?? 1);
}

if (!existsSync(executable)) {
  console.error(
    "jev-compile: building the bundled Rust CLI (first run; Cargo and Rust 1.89+ required).",
  );
  const result = spawnSync(
    "cargo",
    ["install", "--path", root, "--locked", "--root", cache],
    { stdio: ["inherit", 2, 2] },
  );
  if (result.error || result.signal || result.status !== 0) finish(result);
}

finish(spawnSync(executable, process.argv.slice(2), { stdio: "inherit" }));
