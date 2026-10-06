import type { ErrorData, JsonValue } from './types'

export class AiError extends Error implements ErrorData {
  constructor(
    public readonly code: string,
    message: string,
    public readonly details?: JsonValue,
  ) {
    super(message)
    this.name = 'AiError'
  }

  toJSON(): ErrorData {
    return {
      code: this.code,
      message: this.message,
      ...(this.details === undefined ? {} : { details: this.details }),
    }
  }
}

export function errorData(error: unknown): AiError {
  return error instanceof AiError
    ? error
    : new AiError('HANDLER_ERROR', 'Tool handler failed')
}
