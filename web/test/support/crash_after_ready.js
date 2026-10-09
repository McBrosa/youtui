#!/usr/bin/env node

const payload = Buffer.from(
  JSON.stringify({
    version: 1,
    event: "ready",
    cache_dir: process.env.YOUTUI_FAKE_CACHE,
  }),
)
const prefix = Buffer.alloc(4)
prefix.writeUInt32BE(payload.length)
process.stdout.write(Buffer.concat([prefix, payload]), () => process.exit(0))
