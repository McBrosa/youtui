#!/usr/bin/env node

let pending = Buffer.alloc(0)

function writeFrame(value) {
  const payload = Buffer.from(JSON.stringify(value))
  const prefix = Buffer.alloc(4)
  prefix.writeUInt32BE(payload.length)
  process.stdout.write(Buffer.concat([prefix, payload]))
}

writeFrame({
  version: 1,
  event: "ready",
  cache_dir: process.env.YOUTUI_FAKE_CACHE,
})

process.stdin.on("data", (chunk) => {
  pending = Buffer.concat([pending, chunk])

  while (pending.length >= 4) {
    const size = pending.readUInt32BE(0)
    if (pending.length < size + 4) break

    const frame = pending.subarray(4, size + 4)
    pending = pending.subarray(size + 4)
    const request = JSON.parse(frame.toString("utf8"))
    const result = request.command === "ping" ? {version: 1} : {}
    writeFrame({version: 1, id: request.id, ok: true, result})
  }
})
