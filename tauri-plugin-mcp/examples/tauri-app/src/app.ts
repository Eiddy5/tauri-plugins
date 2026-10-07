import { Component, computed, DestroyRef, inject, signal } from '@angular/core'
import { FormsModule } from '@angular/forms'
import { McpError, McpRuntime, type RuntimeEvent } from 'tauri-plugin-mcp-api'

const INPUTS: Record<string, object> = {
  'text.echo': { text: '让 AI 调用你的 TypeScript 方法' },
  'math.add': { a: 21, b: 21 },
  'system.wait': { delayMs: 500 },
  'system.fail': {},
}

@Component({
  selector: 'app-root',
  imports: [FormsModule],
  templateUrl: './app.html',
})
export class AppComponent {
  readonly runtime = inject(McpRuntime)
  readonly tools = this.runtime.tools
  readonly search = signal('')
  readonly selectedName = signal('text.echo')
  readonly selected = computed(
    () => this.tools.find((tool) => tool.name === this.selectedName())!,
  )
  readonly filteredTools = computed(() =>
    this.tools.filter((tool) =>
      (tool.name + tool.description)
        .toLowerCase()
        .includes(this.search().toLowerCase()),
    ),
  )
  readonly running = signal(false)
  readonly result = signal('')
  readonly resultState = signal<'idle' | 'success' | 'error' | 'running'>(
    'idle',
  )
  readonly duration = signal(0)
  readonly history = signal<RuntimeEvent[]>([])
  readonly detailTab = signal<'schema' | 'annotation'>('schema')
  readonly inputSchema = computed(() =>
    JSON.stringify(this.selected().inputSchema, null, 2),
  )
  readonly outputSchema = computed(() =>
    JSON.stringify(this.selected().outputSchema, null, 2),
  )
  readonly annotation = computed(
    () =>
      '@Tool(' +
      JSON.stringify(
        { ...this.selected(), name: this.selectedName().split('.')[1] },
        null,
        2,
      ) +
      ')',
  )
  inputText = JSON.stringify(INPUTS['text.echo'], null, 2)
  private controller?: AbortController

  constructor() {
    const off = this.runtime.onEvent((event) =>
      this.history.update((events) =>
        [
          event,
          ...events.filter((item) => item.requestId !== event.requestId),
        ].slice(0, 12),
      ),
    )
    inject(DestroyRef).onDestroy(off)
  }

  select(name: string): void {
    if (this.running()) return
    this.selectedName.set(name)
    this.resetInput()
    this.result.set('')
    this.resultState.set('idle')
  }

  resetInput(): void {
    this.inputText = JSON.stringify(INPUTS[this.selectedName()], null, 2)
  }
  invalidInput(): void {
    this.inputText = '{"unexpected": true}'
  }
  timeoutInput(): void {
    this.select('system.wait')
    this.inputText = '{"delayMs": 5000}'
  }
  cancel(): void {
    this.controller?.abort()
  }

  async run(): Promise<void> {
    if (this.running()) return
    this.running.set(true)
    this.resultState.set('running')
    this.result.set('')
    this.controller = new AbortController()
    const started = performance.now()
    try {
      let input: unknown
      try {
        input = JSON.parse(this.inputText)
      } catch {
        throw new McpError('INVALID_JSON', '请输入合法的 JSON。')
      }
      const output = await this.runtime.invoke(this.selectedName(), input, {
        signal: this.controller.signal,
      })
      this.result.set(JSON.stringify(output, null, 2))
      this.resultState.set('success')
    } catch (error) {
      const data =
        error instanceof McpError
          ? error.toJSON()
          : { code: 'UNEXPECTED_ERROR', message: String(error) }
      this.result.set(JSON.stringify(data, null, 2))
      this.resultState.set('error')
    } finally {
      this.duration.set(Math.round(performance.now() - started))
      this.running.set(false)
      this.controller = undefined
    }
  }
}
