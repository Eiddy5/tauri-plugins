export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue }
export type JsonSchema = Record<string, unknown>

export interface ToolAnnotations {
  readOnlyHint?: boolean
  destructiveHint?: boolean
  idempotentHint?: boolean
  openWorldHint?: boolean
}

export interface ToolPolicy {
  permissions?: string[]
  timeoutMs?: number
  maxConcurrency?: number
}

export interface ToolOptions {
  name: string
  description: string
  inputSchema: JsonSchema
  outputSchema: JsonSchema
  annotations?: ToolAnnotations
  policy?: ToolPolicy
}

export interface Caller {
  readonly principal: string
  readonly source: 'local' | 'mcp'
}

export interface ToolContext {
  readonly requestId: string
  readonly source: 'local' | 'mcp'
  readonly caller: Caller
  readonly deadline: number
  readonly signal: AbortSignal
}

export interface DefinedTool extends ToolOptions {
  handler: (input: unknown, context: ToolContext) => unknown
}

export interface ErrorData {
  code: string
  message: string
  details?: JsonValue
}

export type BridgeEvent =
  | {
      type: 'call'
      sessionId: string
      requestId: string
      name: string
      arguments: unknown
      deadline: number
      caller: Caller
    }
  | { type: 'cancel'; sessionId: string; requestId: string; error: ErrorData }
  | { type: 'state'; sessionId: string; event: RuntimeEvent; pendingCount: number }

export type Completion =
  | { status: 'success'; result: JsonValue }
  | { status: 'error'; error: ErrorData }

export interface RuntimeTransport {
  readonly mode: 'browser' | 'tauri'
  connect(
    definitions: ToolOptions[],
    receive: (event: BridgeEvent) => void,
  ): Promise<RuntimeSnapshot>
  invoke(
    requestId: string,
    name: string,
    arguments_: JsonValue,
    signal?: AbortSignal,
  ): Promise<JsonValue>
  resolve(
    sessionId: string,
    requestId: string,
    completion: Completion,
  ): Promise<void>
  disconnect(): Promise<void>
}

export interface RuntimeEvent {
  requestId: string
  name: string
  source: 'local' | 'mcp'
  caller: Caller
  state: 'started' | 'completed' | 'failed'
  durationMs: number
  errorCode?: string
}

export interface RuntimeSnapshot {
  tools: ToolOptions[]
  pendingCount: number
}

export interface RuntimeOptions {
  transport?: RuntimeTransport
  beforeCall?: (
    tool: Readonly<ToolOptions>,
    input: JsonValue,
    context: ToolContext,
  ) => void | Promise<void>
}
