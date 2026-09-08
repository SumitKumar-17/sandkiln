import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// Deployed two ways: as a /docs subpath of the main site (see
// website/astro.config.mjs for why base is env-driven — GitHub Pages
// serves a project subpath, Vercel serves the domain root there), or as
// its own standalone deployment at its own domain root (a separate
// Vercel project building only this directory, aliased to
// sandkiln-docs.vercel.app) — set ASTRO_DOCS_STANDALONE=true for that
// second case so this site's own links resolve at "/" instead of
// expecting the merged /docs prefix.
const base = process.env.ASTRO_DOCS_STANDALONE
  ? "/"
  : `${(process.env.ASTRO_BASE || "/").replace(/\/$/, "")}/docs`;

export default defineConfig({
  base,
  trailingSlash: "always",
  integrations: [
    starlight({
      title: "sandkiln docs",
      description:
        "Docs for sandkiln — a compute primitive for safely running untrusted or AI-generated code in hardware-isolated Firecracker microVMs.",
      social: [
        { icon: "github", label: "GitHub", href: "https://github.com/SumitKumar-17/sandkiln" },
      ],
      editLink: {
        baseUrl: "https://github.com/SumitKumar-17/sandkiln/edit/main/website/docs/",
      },
      customCss: ["./src/styles/custom.css"],
      sidebar: [
        {
          label: "Getting Started",
          items: [
            { label: "Introduction", slug: "getting-started/introduction" },
            { label: "Self-hosting quickstart", slug: "getting-started/self-hosting" },
            { label: "First sandbox: JS/TS", slug: "getting-started/js" },
            { label: "First sandbox: Python", slug: "getting-started/python" },
            { label: "First sandbox: CLI", slug: "getting-started/cli" },
          ],
        },
        {
          label: "Core Concepts",
          items: [
            { label: "Sandbox lifecycle", slug: "concepts/sandbox-lifecycle" },
            { label: "Snapshots, resume, and fork", slug: "concepts/snapshots" },
            { label: "Named sandboxes & persistent stop", slug: "concepts/named-sandboxes" },
            { label: "Drives", slug: "concepts/drives" },
            { label: "Custom & managed images", slug: "concepts/images" },
            { label: "Networking & isolation", slug: "concepts/networking" },
            { label: "Auth", slug: "concepts/auth" },
            { label: "Dev-server preview", slug: "concepts/preview" },
          ],
        },
        {
          label: "Guides",
          items: [
            { label: "Run untrusted AI-generated code safely", slug: "guides/run-untrusted-code" },
            { label: "Persist state across runs by name", slug: "guides/persist-by-name" },
            { label: "Share a drive read-only", slug: "guides/share-a-drive" },
            { label: "Boot from a custom image", slug: "guides/custom-image" },
            { label: "Auto-suspend idle sandboxes", slug: "guides/auto-suspend" },
          ],
        },
        {
          label: "Reference",
          items: [
            { label: "CLI (kiln)", slug: "reference/cli" },
            { label: "Daemon HTTP API", slug: "reference/http-api" },
            { label: "JS/TS SDK", slug: "reference/js-sdk" },
            { label: "Python SDK", slug: "reference/python-sdk" },
          ],
        },
        {
          label: "Architecture",
          items: [
            { label: "Overview", slug: "architecture/overview" },
            { label: "Privilege model", slug: "architecture/privilege-model" },
            { label: "Wire protocol", slug: "architecture/wire-protocol" },
            { label: "Persistence model", slug: "architecture/persistence-model" },
            { label: "Bug hunt: the vsock timeout", slug: "architecture/bug-hunt-vsock-timeout" },
            { label: "Startup latency & the pre-warmed pool", slug: "architecture/startup-latency" },
          ],
        },
      ],
    }),
  ],
});
