// Writes plugins/anthology/sounds/attention.wav: a two-note chime made from sine waves, so the
// sound has no third-party source. Mono, 16-bit, 22.05 kHz, about 0.36 s (under 16 KB).
//
//   node scripts/gen-attention-wav.mjs plugins/anthology/sounds/attention.wav
//
// Run it in a container (the repo asks for no host installs):
//   docker run --rm -v "$PWD":/w -w /w node:22-bookworm-slim node scripts/gen-attention-wav.mjs plugins/anthology/sounds/attention.wav
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'

const out = process.argv[2]
if (!out) {
  console.error('usage: node scripts/gen-attention-wav.mjs <output.wav>')
  process.exit(2)
}

const RATE = 22050
const PEAK = 0.55 // of full scale; a notification should not startle
// Each note: frequency in Hz, length in seconds, decay rate (higher dies faster).
const NOTES = [
  { hz: 880, seconds: 0.14, decay: 14 },
  { hz: 1174.66, seconds: 0.22, decay: 11 },
]
const ATTACK = 0.006 // seconds; a ramp in, so the first sample does not click

const samples = []
for (const { hz, seconds, decay } of NOTES) {
  const n = Math.round(seconds * RATE)
  for (let i = 0; i < n; i++) {
    const t = i / RATE
    const envelope = Math.min(1, t / ATTACK) * Math.exp(-decay * t)
    // The fundamental plus a quiet octave, which rounds the tone.
    const wave = Math.sin(2 * Math.PI * hz * t) + 0.25 * Math.sin(2 * Math.PI * 2 * hz * t)
    samples.push((PEAK / 1.25) * envelope * wave)
  }
}
// Fade the last 10 ms to silence so the clip ends without a click.
const tail = Math.round(0.01 * RATE)
for (let i = 0; i < tail; i++) {
  samples[samples.length - 1 - i] *= i / tail
}

const data = Buffer.alloc(samples.length * 2)
samples.forEach((s, i) => data.writeInt16LE(Math.round(Math.max(-1, Math.min(1, s)) * 32767), i * 2))

const header = Buffer.alloc(44)
header.write('RIFF', 0)
header.writeUInt32LE(36 + data.length, 4)
header.write('WAVE', 8)
header.write('fmt ', 12)
header.writeUInt32LE(16, 16) // fmt chunk size
header.writeUInt16LE(1, 20) // PCM
header.writeUInt16LE(1, 22) // mono
header.writeUInt32LE(RATE, 24)
header.writeUInt32LE(RATE * 2, 28) // byte rate
header.writeUInt16LE(2, 32) // block align
header.writeUInt16LE(16, 34) // bits per sample
header.write('data', 36)
header.writeUInt32LE(data.length, 40)

mkdirSync(dirname(out), { recursive: true })
writeFileSync(out, Buffer.concat([header, data]))
console.log(`${out}: ${header.length + data.length} bytes, ${(samples.length / RATE).toFixed(2)} s`)
