import { defineConfig } from "astro/config";
import mdx from "@astrojs/mdx";
import starlight from "@astrojs/starlight";

// GitHub Pages serves this as a *project* page (a subpath,
// sumitkumar-17.github.io/sandkiln/); every other target serves it at a domain
// root. One static build can't bake in two asset base paths, so ASTRO_BASE
// picks which one this build is for, defaulting to root so a plain
// `npm run dev`/`npm run build` needs no ceremony. The Pages workflow
// (.github/workflows/deploy-pages.yml) is the only place that sets it.
const base = process.env.ASTRO_BASE || "/";

// Only used to build absolute URLs (sitemap, canonical tags, the llms.txt
// index). Defaults to the Pages deployment because that is the one this repo
// controls and verifies; a mirror sets ASTRO_SITE to its own origin.
const site = process.env.ASTRO_SITE || "https://sumitkumar-17.github.io";

const docs = (slug) => `docs/${slug}`;

export default defineConfig({
  base,
  site,
  trailingSlash: "always",
  // Starlight registers astro-expressive-code, which has to be set up before
  // mdx() so code blocks inside .mdx pages get the same highlighting as the
  // ones in .md — the build fails loudly if this order is reversed.
  integrations: [
    starlight({
      title: "sandkiln",
      description:
        "Docs for sandkiln — a compute primitive for safely running untrusted or AI-generated code in hardware-isolated Firecracker microVMs.",
      social: [
        { icon: "github", label: "GitHub", href: "https://github.com/SumitKumar-17/sandkiln" },
      ],
      editLink: {
        baseUrl: "https://github.com/SumitKumar-17/sandkiln/edit/main/website/",
      },
      customCss: ["./src/styles/tokens.css", "./src/styles/starlight.css"],
      components: {
        // The docs and the marketing pages are one site; the docs header
        // carries the same top-level nav so moving between them is not a
        // one-way trip out of the sidebar.
        SiteTitle: "./src/components/DocsSiteTitle.astro",
      },
      sidebar: [
        {
          label: "Getting Started",
          items: [
            { label: "Introduction", slug: docs("getting-started/introduction") },
            { label: "Self-hosting quickstart", slug: docs("getting-started/self-hosting") },
            { label: "First sandbox: JS/TS", slug: docs("getting-started/js") },
            { label: "First sandbox: Python", slug: docs("getting-started/python") },
            { label: "First sandbox: CLI", slug: docs("getting-started/cli") },
          ],
        },
        {
          label: "Core Concepts",
          items: [
            { label: "Sandbox lifecycle", slug: docs("concepts/sandbox-lifecycle") },
            { label: "Snapshots, resume, and fork", slug: docs("concepts/snapshots") },
            { label: "Named sandboxes & persistent stop", slug: docs("concepts/named-sandboxes") },
            { label: "Drives", slug: docs("concepts/drives") },
            { label: "Remote storage mounts", slug: docs("concepts/remote-storage") },
            { label: "Custom & managed images", slug: docs("concepts/images") },
            { label: "Networking & isolation", slug: docs("concepts/networking") },
            { label: "Auth", slug: docs("concepts/auth") },
            { label: "Dev-server preview", slug: docs("concepts/preview") },
          ],
        },
        {
          label: "Guides",
          items: [
            { label: "Run untrusted AI-generated code safely", slug: docs("guides/run-untrusted-code") },
            { label: "Persist state across runs by name", slug: docs("guides/persist-by-name") },
            { label: "Share a drive read-only", slug: docs("guides/share-a-drive") },
            { label: "Boot from a custom image", slug: docs("guides/custom-image") },
            { label: "Auto-suspend idle sandboxes", slug: docs("guides/auto-suspend") },
          ],
        },
        {
          label: "Internals",
          items: [
            { label: "SQLite: the history store", slug: docs("internals/sqlite-history-store") },
            { label: "Snapshot, resume, and fork: the mechanism", slug: docs("internals/snapshot-resume-fork") },
            { label: "Drives: the mechanism", slug: docs("internals/drives") },
            { label: "Images: the mechanism", slug: docs("internals/images") },
            { label: "vsock wire protocol", slug: docs("internals/vsock-wire-protocol") },
            { label: "Jailer & privilege model", slug: docs("internals/jailer-privilege-model") },
            { label: "iptables & egress policy", slug: docs("internals/egress-iptables") },
            { label: "TAP devices & bridge networking", slug: docs("internals/tap-bridge-networking") },
            { label: "FUSE and rclone", slug: docs("internals/fuse-rclone-mounts") },
            { label: "Token-bucket rate limiting", slug: docs("internals/rate-limiting") },
            { label: "MMDS (guest metadata)", slug: docs("internals/mmds") },
            { label: "PTY and forkpty", slug: docs("internals/pty") },
            { label: "exec-stream sessions", slug: docs("internals/exec-stream") },
            { label: "Pre-warmed pools", slug: docs("internals/pre-warmed-pools") },
          ],
        },
        {
          label: "Reference",
          items: [
            { label: "CLI (kiln)", slug: docs("reference/cli") },
            { label: "Daemon HTTP API", slug: docs("reference/http-api") },
            { label: "JS/TS SDK", slug: docs("reference/js-sdk") },
            { label: "Python SDK", slug: docs("reference/python-sdk") },
          ],
        },
        {
          label: "Architecture",
          items: [
            { label: "Overview", slug: docs("architecture/overview") },
            { label: "Startup latency & the pre-warmed pool", slug: docs("architecture/startup-latency") },
            { label: "Bug hunt: the vsock timeout", slug: docs("architecture/bug-hunt-vsock-timeout") },
            { label: "Engineering notebook", slug: docs("architecture/engineering-notebook") },
          ],
        },
      ],
    }),
    mdx(),
  ],
});
