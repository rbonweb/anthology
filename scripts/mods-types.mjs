// Dev only (ci/jobs/mods.sh tsc): write Claude Code's hooks declarations for a given native
// binary to a file, for a local tsc run over the anthology mods. The binary keeps them as an
// embedded zstd-compressed file; this finds the frame whose text starts with the declarations'
// own first line and writes it under the header line Claude Code itself writes. The output is
// gitignored: it is Claude Code's file and is never committed.
//
//   node scripts/mods-types.mjs <native claude binary> <version> <out.d.ts>
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { zstdDecompressSync } from 'node:zlib'

const [binary, version, out] = process.argv.slice(2)
if (!binary || !version || !out) {
  console.error('usage: node scripts/mods-types.mjs <binary> <version> <out.d.ts>')
  process.exit(2)
}

const FIRST_LINE = "// Claude Code function hooks: the plugin API's TypeScript declarations."
const MAGIC = Buffer.from([0x28, 0xb5, 0x2f, 0xfd])
// Decompression stops at the end of the first frame; 8 MiB of input covers a frame of the
// declarations' size many times over.
const WINDOW = 8 << 20

const buf = readFileSync(binary)
let text
for (let at = buf.indexOf(MAGIC); at !== -1 && text === undefined; at = buf.indexOf(MAGIC, at + 1)) {
  let raw
  try {
    raw = zstdDecompressSync(buf.subarray(at, Math.min(buf.length, at + WINDOW)))
  } catch {
    continue
  }
  if (raw.subarray(0, FIRST_LINE.length).toString('utf8') === FIRST_LINE) text = raw.toString('utf8')
}
if (text === undefined) {
  console.error(`mods-types: no declarations found in ${binary}`)
  process.exit(1)
}
mkdirSync(dirname(out), { recursive: true })
writeFileSync(out, `// Written by Claude Code ${version}.\n${text}`)
console.log(`mods-types: wrote ${out} (${text.length} characters)`)
