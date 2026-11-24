import { readFileSync } from "node:fs";
import { defineConfig } from "tsup";

// Injected at build time so `kiln --version` always matches package.json.
const { version } = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8")) as { version: string };

export default defineConfig([
  {
    entry: ["src/index.ts"],
    format: ["esm"],
    sourcemap: true,
    clean: true,
    target: "node18",
    banner: { js: "#!/usr/bin/env node" },
    define: { __CLI_VERSION__: JSON.stringify(version) },
  },
  {
    // Built as its own entry (no shebang banner) so the pure formatting/
    // parsing logic can be imported and unit-tested without pulling in
    // the CLI's top-level commander wiring, which parses process.argv as
    // a side effect of being imported at all.
    entry: ["src/format.ts", "src/doctor.ts"],
    format: ["esm"],
    sourcemap: true,
    clean: false,
    target: "node18",
  },
]);
