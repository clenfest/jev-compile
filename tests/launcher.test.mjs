import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

test(
  "npm launcher builds once, preserves arguments, stdout, and failure status",
  { skip: process.platform === "win32" },
  () => {
    const root = mkdtempSync(join(tmpdir(), "jev-launcher-"));
    try {
      const tools = join(root, "tools");
      const cache = join(root, "cache");
      const receipt = join(root, "receipt.json");
      mkdirSync(tools);
      writeFileSync(
        join(tools, "cargo"),
        `#!${process.execPath}
const fs = require("node:fs");
const path = require("node:path");
const args = process.argv.slice(2);
fs.writeFileSync(process.env.RECEIPT, JSON.stringify(args));
const bin = path.join(args[args.indexOf("--root") + 1], "bin");
fs.mkdirSync(bin, { recursive: true });
fs.writeFileSync(path.join(bin, "jev-compile"), ${JSON.stringify(`#!${process.execPath}\nconsole.log(JSON.stringify(process.argv.slice(2))); process.exit(7);\n`)}, { mode: 0o755 });
console.log("compiler output must go to stderr");
`,
        { mode: 0o755 },
      );
      const env = {
        ...process.env,
        PATH: tools,
        XDG_CACHE_HOME: cache,
        RECEIPT: receipt,
      };
      const args = ["--context", "a file with spaces.rs"];
      const first = spawnSync(
        process.execPath,
        [resolve("bin/jev-compile.mjs"), ...args],
        { env, encoding: "utf8" },
      );
      assert.equal(first.status, 7, first.stderr);
      assert.deepEqual(JSON.parse(first.stdout), args);
      assert.match(first.stderr, /compiler output must go to stderr/);
      const installArgs = JSON.parse(readFileSync(receipt, "utf8"));
      assert.equal(installArgs[0], "install");
      assert.ok(installArgs.includes("--locked"));
      rmSync(join(tools, "cargo"));
      const second = spawnSync(
        process.execPath,
        [resolve("bin/jev-compile.mjs"), "--version"],
        { env, encoding: "utf8" },
      );
      assert.equal(second.status, 7, second.stderr);
      assert.deepEqual(JSON.parse(second.stdout), ["--version"]);
      assert.equal(second.stderr, "");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  },
);
