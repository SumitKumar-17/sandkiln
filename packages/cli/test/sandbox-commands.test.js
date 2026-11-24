import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

// The `sandbox` group is split across several modules and assembled by
// registerSandboxCommands. This reads the built CLI's real help output so a
// future split cannot silently drop a subcommand, an option or an argument,
// or reorder the listing. Each row is [usage line, option flags].
const cli = fileURLToPath(new URL("../dist/index.js", import.meta.url));

function help(...args) {
  const result = spawnSync(process.execPath, [cli, ...args, "--help"], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout;
}

function subcommandNames() {
  const lines = help("sandbox").split("\n");
  const start = lines.findIndex((line) => line.startsWith("Commands:"));
  return lines
    .slice(start + 1)
    .filter((line) => line.trim() !== "")
    .map((line) => line.trim().split(/\s+/)[0])
    .filter((name) => name !== "help");
}

function describe(name) {
  const text = help("sandbox", name);
  const usage = /^Usage: kiln sandbox (.*)$/m.exec(text)[1];
  const lines = text.split("\n");
  const start = lines.findIndex((line) => line.startsWith("Options:"));
  const flags = [];
  for (const line of lines.slice(start + 1)) {
    if (line.trim() === "") break;
    const match = /^ {2}(-.*?)(?: {2}|$)/.exec(line);
    if (match && match[1] !== "-h, --help") flags.push(match[1]);
  }
  return [usage, flags];
}

const EXPECTED = [
  ["create [options]", ["--name <name>", "--tag <key=value>", "--vcpu <count>", "--mem <mib>", "--image <id>", "--rate-bandwidth <bytes-per-sec>", "--rate-ops <ops-per-sec>", "--drive <id[:ro]>", "--env <key=value>", "--egress-mode <allow_all|deny_all>", "--allow-cidr <cidr>", "--deny-cidr <cidr>"]],
  ["get-or-create [options]", ["--name <name>", "--tag <key=value>", "--vcpu <count>", "--mem <mib>", "--rate-bandwidth <bytes-per-sec>", "--rate-ops <ops-per-sec>", "--drive <id[:ro]>", "--env <key=value>", "--egress-mode <allow_all|deny_all>", "--allow-cidr <cidr>", "--deny-cidr <cidr>"]],
  ["by-name [options] <name>", []],
  ["ls [options]", ["--tag <key=value>"]],
  ["rm [options] <id>", ["--destroy"]],
  ["exec [options] <id> <command> [args...]", ["--env <key=value>", "--timeout <seconds>"]],
  ["read [options] <id> <path>", []],
  ["write [options] <id> <path> <local-file>", []],
  ["chmod [options] <id> <path> <mode>", []],
  ["chown [options] <id> <path> <uid> <gid>", []],
  ["mkdir [options] <id> <path>", ["-p, --parents"]],
  ["rename [options] <id> <from> <to>", []],
  ["cp [options] <id> <from> <to>", []],
  ["upload-dir [options] <id> <local-dir> <remote-dir>", ["--exclude <name...>"]],
  ["download-dir [options] <id> <remote-dir> <local-dir>", []],
  ["symlink [options] <id> <target> <link-path>", []],
  ["readlink [options] <id> <path>", []],
  ["truncate [options] <id> <path> <size>", []],
  ["ls-dir [options] <id> <path>", []],
  ["pty [options] <id>", []],
  ["tunnel [options] <id> <guest-port>", ["--local-port <port>", "--local-host <host>"]],
  ["exec-stream [options] <id> <command> [args...]", ["--env <key=value>"]],
  ["logs [options] <id> [session-id]", []],
  ["preview [options] <id> <port>", ["--path <path>"]],
  ["mount [options] <id> <bucket> <endpoint> <mount-path>", ["--access-key <key>", "--secret-key <key>", "--read-only"]],
  ["mounts [options] <id>", []],
  ["unmount [options] <id> <mount-id>", []],
  ["snapshot [options] <id>", []],
  ["snapshots [options]", ["--source <sandbox-id>"]],
  ["resume [options] <snapshot-id>", []],
  ["fork [options] <snapshot-id>", []],
];

test("kiln sandbox registers exactly the expected subcommands, in order, with their arguments and options", () => {
  const names = subcommandNames();
  assert.deepEqual(
    names,
    EXPECTED.map(([usage]) => usage.split(" ")[0]),
  );
  assert.deepEqual(names.map(describe), EXPECTED);
});
