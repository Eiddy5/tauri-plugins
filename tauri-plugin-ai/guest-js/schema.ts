import { Ajv2020, type ValidateFunction } from 'ajv/dist/2020.js'
import { AiError } from './errors'
import type { JsonSchema, JsonValue, ToolOptions } from './types'

export const LIMITS = {
  maxTools: 128,
  maxPending: 64,
  maxBytes: 1_048_576,
  timeoutMs: 30_000,
  maxTimeoutMs: 300_000,
} as const

export function jsonCopy(value: unknown, code: string): JsonValue {
  const visit = (item: unknown, depth: number): void => {
    if (depth > 64) throw new AiError(code, 'JSON is too deeply nested')
    if (item === null || typeof item === 'string' || typeof item === 'boolean')
      return
    if (typeof item === 'number' && Number.isFinite(item)) return
    if (Array.isArray(item)) {
      for (const entry of item) visit(entry, depth + 1)
      return
    }
    if (
      typeof item === 'object' &&
      (Object.getPrototypeOf(item) === Object.prototype ||
        Object.getPrototypeOf(item) === null)
    ) {
      for (const entry of Object.values(item)) visit(entry, depth + 1)
      return
    }
    throw new AiError(code, 'Only plain JSON values are supported')
  }
  visit(value, 0)
  const text = JSON.stringify(value)
  if (new TextEncoder().encode(text).length > LIMITS.maxBytes)
    throw new AiError(code, 'JSON exceeds 1 MiB')
  return JSON.parse(text) as JsonValue
}

export function compileSchema(schema: JsonSchema): ValidateFunction {
  if (!schema || typeof schema !== 'object' || schema.type !== 'object')
    throw new AiError('INVALID_DEFINITION', 'Root schema must have type object')
  const scan = (value: unknown): void => {
    if (!value || typeof value !== 'object') return
    for (const [key, item] of Object.entries(value)) {
      if (
        (key === '$ref' || key === '$dynamicRef') &&
        (typeof item !== 'string' || !item.startsWith('#'))
      ) {
        throw new AiError(
          'INVALID_DEFINITION',
          'Only local schema references are supported',
        )
      }
      if (
        key === '$schema' &&
        item !== 'https://json-schema.org/draft/2020-12/schema'
      ) {
        throw new AiError(
          'INVALID_DEFINITION',
          'Schema dialect must be JSON Schema 2020-12',
        )
      }
      scan(item)
    }
  }
  scan(schema)
  try {
    return new Ajv2020({
      strict: false,
      allErrors: false,
      validateFormats: false,
    }).compile(schema)
  } catch {
    throw new AiError('INVALID_DEFINITION', 'Invalid JSON Schema')
  }
}

export function validateDefinition(tool: ToolOptions): void {
  const keys = ['name', 'description', 'inputSchema', 'outputSchema', 'annotations', 'policy']
  if (Object.keys(tool).some((key) => !keys.includes(key)))
    throw new AiError('INVALID_DEFINITION', 'Unknown tool option')
  if (
    typeof tool.name !== 'string' ||
    !/^[a-zA-Z0-9_.-]{1,128}$/.test(tool.name) ||
    typeof tool.description !== 'string' ||
    !tool.description.trim() ||
    new TextEncoder().encode(tool.description).length > 4096
  ) {
    throw new AiError('INVALID_DEFINITION', 'Invalid tool name or description')
  }
  if (tool.annotations !== undefined) {
    const hints = ['readOnlyHint', 'destructiveHint', 'idempotentHint', 'openWorldHint']
    if (!tool.annotations || typeof tool.annotations !== 'object' || Array.isArray(tool.annotations) ||
      Object.entries(tool.annotations).some(([key, value]) => !hints.includes(key) || typeof value !== 'boolean'))
      throw new AiError('INVALID_DEFINITION', 'Invalid annotations')
  }
  if (tool.policy !== undefined && (!tool.policy || typeof tool.policy !== 'object' || Array.isArray(tool.policy) ||
    Object.keys(tool.policy).some((key) => !['permissions', 'timeoutMs', 'maxConcurrency'].includes(key))))
    throw new AiError('INVALID_DEFINITION', 'Invalid policy')
  const permissions = tool.policy?.permissions
  if (permissions !== undefined && (!Array.isArray(permissions) ||
    permissions.some((p) => typeof p !== 'string' || !p.trim()) ||
    new Set(permissions).size !== permissions.length))
    throw new AiError('INVALID_DEFINITION', 'Invalid permissions')
  const concurrency = tool.policy?.maxConcurrency === undefined ? LIMITS.maxPending : tool.policy.maxConcurrency
  if (!Number.isInteger(concurrency) || concurrency < 1 || concurrency > LIMITS.maxPending)
    throw new AiError('INVALID_DEFINITION', 'maxConcurrency must be between 1 and 64')
  const timeout = tool.policy?.timeoutMs === undefined ? LIMITS.timeoutMs : tool.policy.timeoutMs
  if (
    !Number.isInteger(timeout) ||
    timeout < 1 ||
    timeout > LIMITS.maxTimeoutMs
  ) {
    throw new AiError(
      'INVALID_DEFINITION',
      'timeoutMs must be an integer between 1 and 300000',
    )
  }
}

export function validateValue(
  validate: ValidateFunction,
  value: unknown,
  code: string,
): JsonValue {
  const copy = jsonCopy(value, code)
  if (!validate(copy)) {
    throw new AiError(code, 'Value does not match schema', {
      path: validate.errors?.[0]?.instancePath ?? '',
      keyword: validate.errors?.[0]?.keyword ?? '',
    })
  }
  return copy
}
