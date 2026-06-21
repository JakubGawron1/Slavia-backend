#!/usr/bin/env node
/**
 * Generuje src/embed/openapi.json z router.rs (wszystkie trasy, minimalne opisy).
 * Uruchom po zmianie router.rs: node scripts/generate-openapi.mjs
 */
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const routerPath = resolve(root, 'src/router.rs')
const cargoPath = resolve(root, 'Cargo.toml')
const outPath = resolve(root, 'src/embed/openapi.json')

const router = readFileSync(routerPath, 'utf8')
const version = readFileSync(cargoPath, 'utf8').match(/^version\s*=\s*"([^"]+)"/m)?.[1] ?? '0.0.0'

const HTTP_METHODS = ['get', 'post', 'put', 'patch', 'delete']

/** @type {Map<string, string>} */
const nestByVar = new Map()
for (const m of router.matchAll(/\.nest\("([^"]+)",\s*(\w+)\)/g)) {
  nestByVar.set(m[2], m[1])
}

/** @type {Map<string, { path: string, methods: Set<string> }[]>} */
const routesByVar = new Map()

const blockRe = /let\s+(\w+)\s*=\s*Router::new\(\)([\s\S]*?)(?=\n\s*let\s+\w+\s*=|\n\s*Router::new\(\)\s*$|\n\s*Router::new\(\)\s*\n)/g
for (const block of router.matchAll(blockRe)) {
  const varName = block[1]
  const body = block[2]
  /** @type {{ path: string, methods: Set<string> }[]} */
  const entries = []
  for (const rm of body.matchAll(/\.route\(\s*\n?\s*"([^"]+)"\s*,\s*([\s\S]*?)\)(?=\s*\.(?:route|layer)|\s*;)/g)) {
    const routePath = rm[1]
    const handlerChunk = rm[2]
    const methods = new Set()
    for (const method of HTTP_METHODS) {
      if (new RegExp(`\\b${method}\\(`).test(handlerChunk)) {
        methods.add(method)
      }
    }
    if (methods.size) entries.push({ path: routePath, methods })
  }
  if (entries.length) routesByVar.set(varName, entries)
}

/** @type {Record<string, Record<string, object>>} */
const paths = {}

function addPath(fullPath, method) {
  if (!paths[fullPath]) paths[fullPath] = {}
  const op = {
    summary: `${method.toUpperCase()} ${fullPath}`,
    responses: {
      200: { description: 'OK' },
      401: { description: 'Unauthorized' },
      403: { description: 'Forbidden' }
    }
  }
  if (method === 'post') op.responses[201] = { description: 'Created' }
  if (method === 'delete') op.responses[204] = { description: 'No Content' }
  paths[fullPath][method] = op
}

const PUBLIC_PREFIXES = [
  '/api/auth/login',
  '/api/athletes',
  '/api/athletes/archive',
  '/api/athletes/ranking/sinclair',
  '/api/results/public-board',
  '/api/results/public-board-olympic',
  '/api/posts',
  '/api/announcements',
  '/api/gallery',
  '/api/contact',
  '/api/challenges',
  '/api/club/feed',
  '/api/ai/coach/public',
  '/api/system/ping',
  '/api/health',
  '/api/system/calendar/export',
  '/api/system/mobile-releases/latest',
  '/api/system/openapi.json'
]

function isPublicPath(p) {
  if (p === '/api/auth/login') return true
  return PUBLIC_PREFIXES.some(prefix => p === prefix || p.startsWith(`${prefix}/`))
}

for (const [varName, entries] of routesByVar) {
  const prefix = nestByVar.get(varName) ?? ''
  for (const { path: routePath, methods } of entries) {
    const full = `${prefix}${routePath}`.replace(/\/{2,}/g, '/')
    for (const method of methods) {
      addPath(full, method)
      if (!isPublicPath(full) && method !== 'get' || (method === 'get' && !isPublicPath(full))) {
        paths[full][method].security = [{ BearerAuth: [] }]
      }
    }
  }
}

// Root page + top-level routes (poza blokami `let *_routes`)
paths['/'] = {
  get: {
    summary: 'Backend root HTML',
    responses: { 200: { description: 'OK' } }
  }
}
paths['/api/health'] = {
  get: {
    summary: 'GET /api/health',
    description: 'Alias of GET /api/system/ping — lightweight instance health check.',
    responses: { 200: { description: 'OK' } }
  }
}

const spec = {
  openapi: '3.0.0',
  info: {
    title: 'Slavia API',
    version,
    description: 'API for Slavia Weightlifting Club management system'
  },
  servers: [{ url: '/', description: 'Current host' }],
  paths,
  components: {
    securitySchemes: {
      BearerAuth: {
        type: 'http',
        scheme: 'bearer',
        bearerFormat: 'JWT',
        description: 'JWT from POST /api/auth/login'
      }
    },
    schemas: {
      ApiError: {
        type: 'object',
        properties: {
          error: { type: 'string' },
          message: { type: 'string' }
        }
      },
      LoginRequest: {
        type: 'object',
        required: ['username', 'password'],
        properties: {
          username: { type: 'string' },
          password: { type: 'string', format: 'password' }
        }
      },
      LoginResponse: {
        type: 'object',
        properties: {
          token: { type: 'string' },
          user: { $ref: '#/components/schemas/UserProfile' }
        }
      },
      UserProfile: {
        type: 'object',
        properties: {
          id: { type: 'string' },
          username: { type: 'string' },
          role: { type: 'string', enum: ['Athlete', 'Trainer', 'Admin', 'SuperAdmin'] },
          is_banned: { type: 'boolean' }
        }
      },
      Athlete: {
        type: 'object',
        properties: {
          id: { type: 'string' },
          full_name: { type: 'string' },
          birth_year: { type: 'integer', nullable: true },
          gender: { type: 'string', enum: ['male', 'female'], nullable: true },
          weight_category: { type: 'string', nullable: true },
          bodyweight: { type: 'number', nullable: true },
          best_snatch_kg: { type: 'number', nullable: true },
          best_clean_jerk_kg: { type: 'number', nullable: true },
          total_kg: { type: 'number', nullable: true },
          is_active: { type: 'boolean' }
        }
      },
      ContactMessage: {
        type: 'object',
        required: ['name', 'email', 'message'],
        properties: {
          name: { type: 'string' },
          email: { type: 'string', format: 'email' },
          message: { type: 'string' }
        }
      },
      AiCoachStatus: {
        type: 'object',
        properties: {
          configured: { type: 'boolean' },
          model: { type: 'string', nullable: true }
        }
      },
      AiPublicStatus: {
        type: 'object',
        properties: {
          available: { type: 'boolean' },
          reason: { type: 'string', nullable: true },
          message: { type: 'string', nullable: true }
        }
      }
    }
  }
}

// Enrich key operations with schema refs
const enrich = [
  ['/api/auth/login', 'post', { requestBody: { content: { 'application/json': { schema: { $ref: '#/components/schemas/LoginRequest' } } } }, responses: { 200: { description: 'OK', content: { 'application/json': { schema: { $ref: '#/components/schemas/LoginResponse' } } } } } }],
  ['/api/auth/me', 'get', { responses: { 200: { description: 'OK', content: { 'application/json': { schema: { $ref: '#/components/schemas/UserProfile' } } } } } }],
  ['/api/contact', 'post', { requestBody: { content: { 'application/json': { schema: { $ref: '#/components/schemas/ContactMessage' } } } }, responses: { 201: { description: 'Created' }, 429: { description: 'Too many requests' } } }],
  ['/api/ai/coach/status', 'get', { responses: { 200: { description: 'OK', content: { 'application/json': { schema: { $ref: '#/components/schemas/AiCoachStatus' } } } } } }],
  ['/api/ai/coach/public/status', 'get', { responses: { 200: { description: 'OK', content: { 'application/json': { schema: { $ref: '#/components/schemas/AiPublicStatus' } } } } } }]
]
for (const [path, method, patch] of enrich) {
  if (paths[path]?.[method]) Object.assign(paths[path][method], patch)
}

writeFileSync(outPath, `${JSON.stringify(spec, null, 2)}\n`, 'utf8')
const count = Object.keys(paths).length
console.log(`OK: ${outPath} — ${count} paths, version ${version}`)
