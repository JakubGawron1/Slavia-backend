#!/usr/bin/env node
/**
 * Generuje pliki board/templates/* w bootstrapie Slavia-cms z embed/board-templates.json.
 * Uruchom z katalogu Slavia-backend: node scripts/generate-board-bootstrap-templates.mjs
 */
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const root = join(__dirname, '..')
const embedPath = join(root, 'src/embed/board-templates.json')
const outDir = join(root, 'scripts/slavia-cms-board-bootstrap/templates')

const EXT_BY_MIME = {
  'text/html; charset=utf-8': 'html',
  'text/csv; charset=utf-8': 'csv',
  'text/plain; charset=utf-8': 'txt'
}

const map = JSON.parse(readFileSync(embedPath, 'utf8'))
mkdirSync(outDir, { recursive: true })

for (const [id, entry] of Object.entries(map)) {
  const ext = EXT_BY_MIME[entry.mime] ?? 'txt'
  const outPath = join(outDir, `${id}.${ext}`)
  writeFileSync(outPath, entry.content, 'utf8')
  console.log('wrote', outPath)
}

console.log(`Done — ${Object.keys(map).length} templates.`)
