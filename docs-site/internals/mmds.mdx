---
title: "Internals: MMDS (guest metadata service)"
description: How a sandbox's own id, name, and tags reach the guest without a network round-trip to the daemon.
---

## What it is

An instance metadata service is a pattern most major cloud providers use: a fixed, well-known address (often `169.254.169.254`, a link-local IP that's never routable off the local machine) serves a small HTTP API from inside the VM itself, answering questions like "what's my own instance ID" or "what tags was I launched with." No credentials are needed to reach it from the guest side (though a token may still be required, see below), and no external network call ever leaves the machine, the answer comes from the hypervisor, not a remote server.

Firecracker implements exactly this pattern as MMDS (MicroVM Metadata Service), and sandkiln uses it as-is.

## Why sandkiln uses it here

Without MMDS, a sandboxed process that wants to know its own sandbox id or the tags it was created with would need the caller to pass that information in some other way, an environment variable set at exec time, or a file written after boot. MMDS instead makes it available from the moment the guest is up, over a channel the guest already has (its own virtual network interface), with no coordination needed between the caller and whatever code ends up running inside.

## Key terms

- **MMDS V2 (token-gated)**: the version sandkiln configures. A caller must first `PUT` a request to a token endpoint, then include that token as a header on every subsequent metadata request. This exists specifically to prevent a class of vulnerability (SSRF, server-side request forgery) where an attacker tricks a *different* process into making a GET request to the metadata endpoint on the victim's behalf; a bare GET is no longer enough by itself.
- **`X-metadata-token-ttl-seconds`**: how long the requested token stays valid, set on the token-request `PUT`.
- **`X-metadata-token`**: the header every subsequent metadata `GET` must carry, set to the token returned by the `PUT` above.

## How it works in sandkiln

MMDS requires a network interface to exist at all (`sandkiln_vmm::vm::VmConfig::metadata` is documented as requiring `network` to also be set) -- a sandbox with no network configured gets no MMDS either. When it is configured, boot issues two requests to Firecracker's own API in sequence: `PUT /mmds/config` (declaring which network interface serves it, and that it's V2) followed by `PUT /mmds` with the actual JSON content, built from the caller's own `id`, `name`, and `tags` at create time.

**The one real gotcha**: a resumed VM's MMDS data store comes back empty from Firecracker's own snapshot/restore, even though every other piece of guest state (memory, open files, running processes) survives intact. `Vm::update_metadata` exists specifically for this: it redoes the *full* `PUT /mmds/config` + `PUT /mmds` sequence rather than a lighter `PATCH /mmds`, because a bare `PATCH` against an empty-after-resume store doesn't recreate it the same way a full `PUT` does. This gets called automatically whenever a caller resumes or forks a named/tagged sandbox, so MMDS content is always correct for whoever is actually holding the sandbox now, not whichever caller originally booted it (see `crate::pool_claim`'s own use of this: a pool's warm-boot placeholder identity is overwritten with the real claimer's `name`/`tags` this same way).

## See it in action

First, the token:

```
$ curl -si -X PUT http://169.254.169.254/latest/api/token \
    -H 'X-metadata-token-ttl-seconds: 21600'

HTTP/1.1 200
Server: Firecracker API
Connection: keep-alive
X-metadata-token-ttl-seconds: 21600
Content-Type: text/plain
Content-Length: 48

lKr9iNwNA2uursz71GJQSAtpRn1zu4iA31Dmrt251M37zM2s
```

That request and everything below it runs *from inside a running sandbox* (via `exec`), not from the host -- MMDS is only reachable from the guest's own network. Then the actual metadata, using that token:

```
$ curl -s -H 'X-metadata-token: lKr9iNwNA2uursz71GJQSAtpRn1zu4iA31Dmrt251M37zM2s' \
    -H 'Accept: application/json' \
    http://169.254.169.254/

{"id":"fe19b607-616e-4038-9d04-e8e7a1c9dc76","name":"mmds-demo","tags":{"purpose":"demo"}}
```

That's a sandbox created with `{"name":"mmds-demo","tags":{"purpose":"demo"}}` in the original `POST /sandboxes` body, read back from inside the guest itself with no host round-trip at all -- both requests above went from the sandboxed process straight to Firecracker's own metadata endpoint.
