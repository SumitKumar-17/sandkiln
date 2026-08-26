# images

Kernel and rootfs build scripts for sandbox base images.

Empty for now. Phase 1 adds scripts to fetch/build a minimal guest kernel
and a minimal rootfs to boot the first microVM. Phase 6 adds the managed
base image set and custom image support.

Built artifacts (kernels, rootfs images) are not committed — see
`.gitignore` — they're large binary blobs that get rebuilt or fetched, not
tracked in git history.
