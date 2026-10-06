import type { ValidateFunction } from 'ajv'
import { AiError } from './errors'
import { compileSchema, jsonCopy, LIMITS, validateDefinition } from './schema'
import type { DefinedTool, ToolOptions } from './types'

export interface Binding {
  definition: ToolOptions
  handler: DefinedTool['handler']
  input: ValidateFunction
  output: ValidateFunction
}

/** Metadata is copied atomically; instances and functions stay in this host. */
export class ToolRegistry {
  private readonly bindings = new Map<string, Binding>()

  register(tools: DefinedTool[]): void {
    if (this.bindings.size + tools.length > LIMITS.maxTools)
      throw new AiError('INVALID_DEFINITION', 'Too many tools')
    const staged = new Map<string, Binding>()
    for (const tool of tools) {
      const { handler, ...metadata } = tool
      if (typeof handler !== 'function')
        throw new AiError('INVALID_DEFINITION', 'Tool handler must be a function')
      const definition = jsonCopy(metadata, 'INVALID_DEFINITION') as unknown as ToolOptions
      validateDefinition(definition)
      if (this.bindings.has(definition.name) || staged.has(definition.name))
        throw new AiError('DUPLICATE_TOOL', 'Duplicate tool: ' + definition.name)
      staged.set(definition.name, {
        definition, handler,
        input: compileSchema(definition.inputSchema),
        output: compileSchema(definition.outputSchema),
      })
    }
    jsonCopy([...this.definitions, ...[...staged.values()].map((b) => b.definition)], 'INVALID_DEFINITION')
    for (const [name, binding] of staged) this.bindings.set(name, binding)
  }

  get(name: string): Binding | undefined { return this.bindings.get(name) }
  get definitions(): ToolOptions[] {
    return JSON.parse(JSON.stringify([...this.bindings.values()].map((b) => b.definition)))
  }
}
