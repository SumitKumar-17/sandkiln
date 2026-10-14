import { defineConfig } from "astro/config";

// GitHub Pages serves this as a *project* page (a subpath,
// sumitkumar-17.github.io/sandkiln/); every other target serves it at a domain
// root. One static build can't bake in two asset base paths, so ASTRO_BASE
// picks which one this build is for, defaulting to root so a plain
// `npm run dev`/`npm run build` needs no ceremony. The Pages workflow
// (.github/workflows/deploy-pages.yml) is the only place that sets it.
const base = process.env.ASTRO_BASE || "/";

// Only used to build absolute URLs (sitemap, canonical tags). Defaults to the
// Pages deployment because that is the one this repo controls and verifies;
// a mirror sets ASTRO_SITE to its own origin.
const site = process.env.ASTRO_SITE || "https://sumitkumar-17.github.io";

export default defineConfig({
  base,
  site,
  trailingSlash: "always",
});
