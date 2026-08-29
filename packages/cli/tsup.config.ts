import { defineConfig } from "tsup";

export default defineConfig([
  {
    entry: ["src/index.ts"],
    format: ["esm"],
    sourcemap: true,
    clean: true,
    target: "node18",
    banner: { js: "#!/usr/bin/env node" },
  },
  {
    // Built as its own entry (no shebang banner) so the pure formatting/
    // parsing logic can be imported and unit-tested without pulling in
    // the CLI's top-level commander wiring, which parses process.argv as
    // a side effect of being imported at all.
    entry: ["src/format.ts"],
    format: ["esm"],
    sourcemap: true,
    clean: false,
    target: "node18",
  },
]);
