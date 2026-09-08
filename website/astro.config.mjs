import { defineConfig } from "astro/config";

// Two live deploy targets, at two different path depths: GitHub Pages
// serves this as a *project* page (sumitkumar-17.github.io/sandkiln/, a
// subpath), Vercel serves it at its own domain root
// (sandkiln.vercel.app/). One static build can't have two different
// asset base paths baked in — ASTRO_BASE picks which one this build is
// for, defaulting to root ("/") so a plain `npm run build`/`astro dev`
// works correctly for Vercel and local dev without ceremony; the Pages
// workflow (`.github/workflows/deploy-pages.yml`) sets
// ASTRO_BASE=/sandkiln before its build instead.
const base = process.env.ASTRO_BASE || "/";

export default defineConfig({
  base,
  trailingSlash: "always",
});
