import type { ErrorData, JsonValue } from './types'

export class McpError extends Error implements ErrorData {
  constructor(
    public readonly code: string,
    message: string,
    public readonly details?: JsonValue,
  ) {
    super(message)
    this.name = 'McpError'
  }

  toJSON(): ErrorData {
    return {
      code: this.code,
      message: this.message,
      ...(this.details === undefined ? {} : { details: this.details }),
    }
  }
}

export function errorData(error: unknown): McpError {
  return error instanceof McpError
    ? error
    : new McpError('HANDLER_ERROR', 'Tool handler failed')
}
