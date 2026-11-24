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
    // Pure helpers as banner-less entries so unit tests import them without
    // starting the stdio server, which index.ts does as a side effect.
    entry: { config: "src/config.ts", truncate: "src/truncate.ts", errors: "src/errors.ts", session: "src/session.ts", "exec-timeout": "src/exec-timeout.ts" },
    format: ["esm"],
    sourcemap: true,
    clean: false,
    target: "node18",
  },
]);
